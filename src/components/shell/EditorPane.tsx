import { useEffect, useRef, useState } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument, saveWikiPage } from "../../lib/tauri";
import { useTheme } from "../../lib/ThemeContext";

interface Props {
  selectedDoc: string | null;
  isWiki: boolean;
  /**
   * Optional chunk id within `selectedDoc`. When set, after the document
   * loads we locate the matching block (a heading whose text equals the
   * chunk id) and scroll to it with a transient 1.5s highlight.
   */
  anchorChunkId?: string | null;
}

/**
 * Pull the plain-text content of a BlockNote block. Supports the default
 * `paragraph`, `heading`, and `bulletListItem` types we use in this app.
 */
function blockText(block: { type: string; content?: unknown }): string {
  const parts: string[] = [];
  const content = block.content as
    | Array<{ type: string; text?: string; content?: unknown }>
    | undefined;
  if (!Array.isArray(content)) return "";
  for (const inline of content) {
    if (inline.type === "text" && typeof inline.text === "string") {
      parts.push(inline.text);
    }
  }
  return parts.join("").trim();
}

export function EditorPane({ selectedDoc, isWiki, anchorChunkId = null }: Props) {
  const editor = useCreateBlockNote();
  const { resolved: theme } = useTheme();
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveOk, setSaveOk] = useState(false);
  const [loadedDoc, setLoadedDoc] = useState<string | null>(null);
  const paneRef = useRef<HTMLElement | null>(null);
  // Hold the most recently parsed blocks so the anchor-highlight effect can
  // find the target without re-reading the file or relying on BlockNote's
  // internal `editor.document` (which the tests mock as an empty array).
  const lastBlocksRef = useRef<Array<{ type: string; content?: unknown; id: string }>>([]);

  // Doc-load effect: only re-runs when `selectedDoc` changes. Critically, it
  // does NOT depend on `anchorChunkId` — re-running on anchor change would
  // overwrite the user's in-progress wiki edits with the freshly-read file.
  useEffect(() => {
    if (!selectedDoc) {
      setLoadError(null);
      setLoadedDoc(null);
      return;
    }
    setLoadError(null);
    setSaveError(null);
    setSaveOk(false);
    // Reset load identity so the anchor effect doesn't reuse blocks from
    // a prior selection. Without this, a rapid A → B → A switch can leave
    // `loadedDoc === "A.md"` while `lastBlocksRef` still points at the
    // earlier A's blocks — the anchor effect would then "succeed" without
    // actually resolving the new A's chunk anchor.
    setLoadedDoc(null);
    lastBlocksRef.current = [];
    // Effect-local cancellation flag: if `selectedDoc` changes while
    // `readDocument` / `tryParseMarkdownToBlocks` is in flight, the older
    // resolution must NOT replace the newer editor blocks. The cleanup
    // marks this effect as cancelled; the async chain checks the flag
    // before every post-await mutation.
    let cancelled = false;
    readDocument(selectedDoc)
      .then(async (content) => {
        if (cancelled) return;
        const blocks = await editor.tryParseMarkdownToBlocks(content);
        if (cancelled) return;
        editor.replaceBlocks(editor.document, blocks);
        if (cancelled) return;
        lastBlocksRef.current = (blocks as Array<{ type: string; content?: unknown; id?: string }>)
          .map((b) => ({ ...b, id: b.id ?? "" }));
        setLoadedDoc(selectedDoc);
      })
      .catch((err) => {
        if (cancelled) return;
        setLoadError(
          err instanceof Error ? err.message : String(err) || "Failed to load document",
        );
      });
    return () => {
      cancelled = true;
    };
  }, [selectedDoc, editor]);

  // Anchor-highlight effect: runs on doc-load (loadedDoc change) and on
  // anchorChunkId change. Does NOT mutate editor blocks — only scrolls +
  // transiently highlights a node.
  useEffect(() => {
    if (!selectedDoc || !anchorChunkId || loadedDoc !== selectedDoc) return;
    let highlightTimer: number | undefined;
    // Track the node we added the highlight class to so we can remove it
    // during cleanup. The timer may be cancelled mid-flight (selectedDoc
    // or anchorChunkId changed before 1.5s elapsed) — without this the
    // old node would stay highlighted until the next anchor resolution.
    let highlightedNode: HTMLElement | null = null;
    const cleanupHighlight = () => {
      if (highlightTimer !== undefined) {
        window.clearTimeout(highlightTimer);
        highlightTimer = undefined;
      }
      if (highlightedNode) {
        highlightedNode.classList.remove("editor-pane-block--anchor-highlight");
        highlightedNode = null;
      }
    };
    // Defer to next frame so BlockNote has rendered the new blocks.
    const raf = requestAnimationFrame(() => {
      const blocks = lastBlocksRef.current;
      const target = blocks.find((b) => {
        // Anchor chunk ids only ever identify heading blocks; rejecting
        // paragraphs that happen to share the text prevents a duplicate-
        // text earlier block from being matched.
        if (b.type !== "heading") return false;
        const text = blockText(b);
        return text === anchorChunkId;
      });
      if (!target) return;
      try {
        editor.setTextCursorPosition(target.id, "end");
      } catch {
        return;
      }
      const root = paneRef.current;
      if (!root) return;
      // Escape the id for CSS attribute matching — BlockNote generates
      // hyphenated ids today, but attribute selectors interpret `"`, `\`,
      // and `]` specially, so interpolating raw breaks for any id containing
      // those characters.
      const safeSelector =
        typeof CSS !== "undefined" && CSS.escape
          ? `[data-id="${CSS.escape(target.id)}"]`
          : `[data-id="${target.id.replace(/(["\\\]])/g, "\\$1")}"]`;
      const node = root.querySelector(safeSelector) as HTMLElement | null;
      if (!node) return; // block not yet painted; skip rather than fight
      // setTextCursorPosition only moves the editor selection; scroll the
      // resolved DOM node into view so the user can see the anchor.
      node.scrollIntoView({ block: "nearest", behavior: "auto" });
      cleanupHighlight();
      node.classList.add("editor-pane-block--anchor-highlight");
      highlightedNode = node;
      highlightTimer = window.setTimeout(() => {
        node.classList.remove("editor-pane-block--anchor-highlight");
        highlightTimer = undefined;
        highlightedNode = null;
      }, 1500);
    });
    return () => {
      cancelAnimationFrame(raf);
      cleanupHighlight();
    };
  }, [selectedDoc, anchorChunkId, loadedDoc, editor]);

  async function handleSave() {
    if (!selectedDoc || !isWiki) return;
    setSaveError(null);
    setSaveOk(false);
    try {
      const md = await editor.blocksToMarkdownLossy(editor.document);
      const filename = selectedDoc.split("/").at(-1) ?? "page.md";
      await saveWikiPage(filename, md);
      setSaveOk(true);
    } catch (err) {
      setSaveError(
        err instanceof Error ? err.message : String(err) || "Failed to save",
      );
    }
  }

  if (!selectedDoc) {
    return (
      <main className="editor-pane">
        <p className="placeholder">Drop your first document</p>
      </main>
    );
  }

  return (
    <main
      ref={paneRef}
      className="editor-pane editor-pane--active"
    >
      {!isWiki && (
        <div className="editor-protected-badge">User Document — protected</div>
      )}
      {isWiki && (
        <div className="editor-toolbar">
          <button className="editor-save-btn" onClick={handleSave}>
            Save
          </button>
          {saveOk && (
            <span className="editor-status editor-status--ok">Saved</span>
          )}
          {saveError && (
            <span className="editor-status editor-status--error" role="alert">
              {saveError}
            </span>
          )}
        </div>
      )}
      {loadError ? (
        <div className="editor-error" role="alert">
          <p>Could not open this document.</p>
          <p className="editor-error-detail">{loadError}</p>
        </div>
      ) : (
        <BlockNoteView editor={editor} editable={isWiki} theme={theme} />
      )}
    </main>
  );
}
