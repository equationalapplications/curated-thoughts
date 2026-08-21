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
 *
 * **Known limitation:** tight lists (no blank lines between items),
 * tables, and multi-paragraph blockquotes parse into different
 * block counts in BlockNote than this walker produces. The
 * alignment breaks for those shapes and the overlay will land on
 * the wrong region. The hook falls back to the source-moved badge
 * when no blocks cover the resolved range, so the failure is
 * visible rather than silent.
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
 * markdown ranges with the BlockNote blocks array. When the counts
 * disagree, we stop at the shorter array so any extra BlockNote
 * blocks get no range (the hook then surfaces source-moved). */
function buildBlockLineMap(
  content: string,
  blocks: Array<{ id: string }>,
): BlockLineMap {
  const ranges = computeMarkdownLineRanges(content);
  const map: BlockLineMap = new Map();
  const len = Math.min(blocks.length, ranges.length);
  for (let i = 0; i < len; i++) {
    const range: BlockLineRange = [
      ranges[i].startLine,
      ranges[i].endLine,
    ];
    map.set(blocks[i].id, range);
  }
  return map;
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
  const overlayDismissTimerRef = useRef<number | undefined>(undefined);
  // Local dismissal flag: when true, the line overlay is hidden even if
  // `useChunkOverlay` still reports `"visible"`. The hook's status is a
  // pure data signal (IPC + rect); presentation/dismissal is a concern
  // local to the pane and lives here so the hook's contract stays narrow.
  const [dismissed, setDismissed] = useState(false);

  const { status: overlayStatus, overlay } = useChunkOverlay(
    selectedDoc,
    anchorChunkId,
    container,
    lineMap,
  );

  // Doc-load effect: re-runs only on `selectedDoc` change. After the
  // markdown is parsed into BlockNote blocks, we compute the line map
  // once and hand it to the overlay hook.
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
        // Compute the line map from the markdown source. The hook reads
        // line ranges from this map (not from DOM attributes) so no DOM
        // stamping is needed.
        setLineMap(buildBlockLineMap(content, blocks));
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

  // Auto-dismiss the overlay 1.5s after it becomes visible. Any
  // non-visible status is treated as a fresh presentation decision —
  // we clear the previous auto-dismissal so the source-moved notice
  // can render even after a visible overlay has timed out.
  useEffect(() => {
    if (overlayStatus !== "visible") {
      if (overlayDismissTimerRef.current !== undefined) {
        window.clearTimeout(overlayDismissTimerRef.current);
        overlayDismissTimerRef.current = undefined;
      }
      setDismissed(false);
      return;
    }
    // Fresh visible overlay → ensure not pre-dismissed by a prior render.
    setDismissed(false);
    overlayDismissTimerRef.current = window.setTimeout(() => {
      overlayDismissTimerRef.current = undefined;
      setDismissed(true);
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
      {overlayStatus === "source-moved" && !dismissed && (
        <div className="editor-pane-source-moved-notice" role="status">
          <span>The source may have moved since this fact was created.</span>
          <button
            type="button"
            className="editor-pane-source-moved-notice__dismiss"
            onClick={() => {
              // Dismissing hides the badge for this navigation only; a
              // fresh navigation will re-resolve and may show it again.
              setDismissed(true);
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
      {overlayStatus === "visible" && overlay && !dismissed && (
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
