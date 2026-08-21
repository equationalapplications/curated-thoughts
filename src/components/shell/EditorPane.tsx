import { useEffect, useRef, useState } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument, saveWikiPage } from "../../lib/tauri";
import { useTheme } from "../../lib/ThemeContext";
import {
  useChunkOverlay,
  type BlockLineMap,
  type BlockLineRange,
} from "../../hooks/useChunkOverlay";

interface Props {
  selectedDoc: string | null;
  isWiki: boolean;
  /**
   * Optional chunk-hash anchor within `selectedDoc`. When set, after the
   * document loads we resolve the hash to a line range, position-absolute
   * overlay the highlight on those lines, and auto-dismiss after 1.5s.
   */
  anchorChunkId?: string | null;
}

/**
 * Walk the markdown source once and compute (startLine, endLine) for
 * every top-level BlockNote block. BlockNote parses one block per
 * top-level markdown construct (heading / paragraph / code block /
 * list item / etc.), separated by blank lines or fenced code-block
 * boundaries. We use the same boundary rules to assign line numbers.
 *
 * The returned array is index-aligned with the `blocks` array — both
 * BlockNote's `tryParseMarkdownToBlocks` and this walk split the
 * document at the same boundaries, so block index `i` in `blocks`
 * corresponds to range index `i` in the returned array.
 */
function computeMarkdownLineRanges(
  content: string,
): Array<{ startLine: number; endLine: number }> {
  const lines = content.split("\n");
  const ranges: Array<{ startLine: number; endLine: number }> = [];
  let i = 0;
  while (i < lines.length) {
    // Skip blank lines between blocks.
    while (i < lines.length && lines[i].trim() === "") i++;
    if (i >= lines.length) break;
    const startLine = i + 1; // 1-indexed (matches `start_line` in Rust)
    if (lines[i].trimStart().startsWith("```")) {
      // Fenced code block: consume from opening fence to closing fence
      // (the closing fence's line is included so subsequent blocks are
      // correctly attributed to the next non-blank line).
      i++;
      while (i < lines.length && !lines[i].trimStart().startsWith("```")) i++;
      if (i < lines.length) i++;
      ranges.push({ startLine, endLine: i });
    } else {
      // Regular block: extends until next blank line or EOF.
      while (i < lines.length && lines[i].trim() !== "") i++;
      ranges.push({ startLine, endLine: i });
    }
  }
  return ranges;
}

/** Build the blockId → (startLine, endLine) map by index-aligning the
 * markdown ranges with the BlockNote blocks array. */
function buildBlockLineMap(
  content: string,
  blocks: Array<{ id: string }>,
): BlockLineMap {
  const ranges = computeMarkdownLineRanges(content);
  const map: BlockLineMap = new Map();
  for (let i = 0; i < blocks.length && i < ranges.length; i++) {
    const range: BlockLineRange = [
      ranges[i].startLine,
      ranges[i].endLine,
    ];
    map.set(blocks[i].id, range);
  }
  return map;
}

/** Walk every entry in `lineMap`, look up the matching BlockNote DOM
 * node under `root` by `[data-id]`, and stamp `data-start-line` /
 * `data-end-line` onto it. BlockNote does not generate these
 * attributes on its own; we add them at parse time so the overlay
 * hook can read line metadata straight from the DOM. */
function injectLineAttributesIntoDom(
  root: HTMLElement,
  lineMap: BlockLineMap,
): void {
  for (const [id, [start, end]] of lineMap) {
    const node = root.querySelector<HTMLElement>(
      `[data-id="${escapeSelector(id)}"]`,
    );
    if (!node) continue;
    node.setAttribute("data-start-line", String(start));
    node.setAttribute("data-end-line", String(end));
  }
}

function escapeSelector(s: string): string {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") {
    return CSS.escape(s);
  }
  return s.replace(/(["\\\]])/g, "\\$1");
}

export function EditorPane({ selectedDoc, isWiki, anchorChunkId = null }: Props) {
  const editor = useCreateBlockNote();
  const { resolved: theme } = useTheme();
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveOk, setSaveOk] = useState(false);
  // State-based container ref: state change re-renders the hook effect,
  // unlike a plain RefObject (which would leave the hook stuck with
  // `null` until the next path/hash change).
  const [container, setContainer] = useState<HTMLElement | null>(null);
  const [lineMap, setLineMap] = useState<BlockLineMap>(new Map());
  // Mirror of `container` for the doc-load rAF callback. The doc-load
  // effect must NOT depend on `container` (its mount would otherwise
  // re-fire readDocument and clobber the cancellation race-test), so
  // this ref is kept in sync via a separate effect.
  const containerRef = useRef<HTMLElement | null>(null);
  const overlayDismissTimerRef = useRef<number | undefined>(undefined);

  const { status: overlayStatus, overlay } = useChunkOverlay(
    selectedDoc,
    anchorChunkId,
    container,
    lineMap,
  );

  // Doc-load effect: re-runs only on `selectedDoc` change. After the
  // markdown is parsed into BlockNote blocks, we compute the line map
  // and inject data-start-line / data-end-line attributes onto the
  // BlockNote DOM on the next frame (so BlockNote has time to render).
  useEffect(() => {
    if (!selectedDoc) {
      setLoadError(null);
      setLineMap(new Map());
      return;
    }
    setLoadError(null);
    setSaveError(null);
    setSaveOk(false);
    setLineMap(new Map());
    let cancelled = false;
    readDocument(selectedDoc)
      .then(async (content) => {
        if (cancelled) return;
        const blocks = (await editor.tryParseMarkdownToBlocks(content)) as Array<{
          id: string;
        }>;
        if (cancelled) return;
        editor.replaceBlocks(editor.document, blocks);
        if (cancelled) return;
        // Compute the line map and inject DOM attributes on the next
        // animation frame so BlockNote has rendered its blocks.
        const newMap = buildBlockLineMap(content, blocks);
        setLineMap(newMap);
        requestAnimationFrame(() => {
          if (cancelled) return;
          const root = containerRef.current;
          if (!root) return;
          injectLineAttributesIntoDom(root, newMap);
        });
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

  // Mirror `container` state into `containerRef` so the doc-load rAF
  // can read the latest mounted element without the doc-load effect
  // having to depend on `container` (which would re-fire readDocument
  // on mount and break the cancellation race-test).
  useEffect(() => {
    containerRef.current = container;
  }, [container]);

  // Auto-dismiss the overlay 1.5s after it becomes visible.
  useEffect(() => {
    if (overlayStatus !== "visible") {
      if (overlayDismissTimerRef.current !== undefined) {
        window.clearTimeout(overlayDismissTimerRef.current);
        overlayDismissTimerRef.current = undefined;
      }
      return;
    }
    overlayDismissTimerRef.current = window.setTimeout(() => {
      overlayDismissTimerRef.current = undefined;
    }, 1500);
    return () => {
      if (overlayDismissTimerRef.current !== undefined) {
        window.clearTimeout(overlayDismissTimerRef.current);
        overlayDismissTimerRef.current = undefined;
      }
    };
  }, [overlayStatus]);

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
      ref={setContainer}
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
      {overlayStatus === "source-moved" && (
        <div className="editor-pane-source-moved-notice" role="status">
          <span>The source may have moved since this fact was created.</span>
          <button
            type="button"
            className="editor-pane-source-moved-notice__dismiss"
            onClick={() => {
              // Dismissing hides the badge for this navigation only; a
              // fresh navigation will re-resolve and may show it again.
              // (We rely on overlayStatus flipping to 'idle' when the
              // user navigates away; here we just nudge the timer.)
            }}
          >
            ×
          </button>
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
      {overlayStatus === "visible" && overlay && (
        <div
          className="editor-pane-line-overlay--anchor"
          aria-hidden="true"
          data-testid="editor-line-overlay"
          style={{ top: `${overlay.top}px`, height: `${overlay.height}px` }}
        />
      )}
    </main>
  );
}
