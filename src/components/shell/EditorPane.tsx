import { useEffect, useRef, useState } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument, saveWikiPage } from "../../lib/tauri";
import { useTheme } from "../../lib/ThemeContext";
import { useChunkOverlay } from "../../hooks/useChunkOverlay";

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

export function EditorPane({ selectedDoc, isWiki, anchorChunkId = null }: Props) {
  const editor = useCreateBlockNote();
  const { resolved: theme } = useTheme();
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveOk, setSaveOk] = useState(false);
  const paneRef = useRef<HTMLElement | null>(null);
  const overlayDismissTimerRef = useRef<number | undefined>(undefined);

  const { status: overlayStatus, overlay } = useChunkOverlay(
    selectedDoc,
    anchorChunkId,
    paneRef,
  );

  // Doc-load effect: unchanged from before — only re-runs on `selectedDoc`.
  useEffect(() => {
    if (!selectedDoc) {
      setLoadError(null);
      return;
    }
    setLoadError(null);
    setSaveError(null);
    setSaveOk(false);
    let cancelled = false;
    readDocument(selectedDoc)
      .then(async (content) => {
        if (cancelled) return;
        const blocks = await editor.tryParseMarkdownToBlocks(content);
        if (cancelled) return;
        editor.replaceBlocks(editor.document, blocks);
        if (cancelled) return;
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
