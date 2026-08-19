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
  const paneRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!selectedDoc) {
      setLoadError(null);
      return;
    }
    setLoadError(null);
    setSaveError(null);
    setSaveOk(false);
    readDocument(selectedDoc)
      .then(async (content) => {
        const blocks = await editor.tryParseMarkdownToBlocks(content);
        editor.replaceBlocks(editor.document, blocks);

        // Anchor highlight: find the first block whose text matches
        // `anchorChunkId` (case-sensitive heading match) and scroll to it.
        if (anchorChunkId) {
          // Defer to next frame so BlockNote has rendered the new blocks.
          requestAnimationFrame(() => {
            const target = blocks.find((b) => {
              const text = blockText(b);
              return text === anchorChunkId;
            });
            if (!target) return;
            try {
              editor.setTextCursorPosition(target.id, "end");
            } catch {
              // Block may not be addressable until rendered; skip silently.
              return;
            }
            const root = paneRef.current;
            if (!root) return;
            const node = root.querySelector(
              `[data-id="${target.id}"]`,
            ) as HTMLElement | null;
            if (node) {
              node.classList.add("editor-pane-block--anchor-highlight");
              window.setTimeout(() => {
                node.classList.remove(
                  "editor-pane-block--anchor-highlight",
                );
              }, 1500);
            }
          });
        }
      })
      .catch((err) => {
        setLoadError(
          err instanceof Error ? err.message : String(err) || "Failed to load document",
        );
      });
  }, [selectedDoc, anchorChunkId, editor]);

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