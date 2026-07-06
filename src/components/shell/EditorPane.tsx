import { useEffect, useState } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument, saveWikiPage } from "../../lib/tauri";
import { useTheme } from "../../lib/ThemeContext";

interface Props {
  selectedDoc: string | null;
  isWiki: boolean;
}

export function EditorPane({ selectedDoc, isWiki }: Props) {
  const editor = useCreateBlockNote();
  const { resolved: theme } = useTheme();
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveOk, setSaveOk] = useState(false);

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
      })
      .catch((err) => {
        setLoadError(
          err instanceof Error ? err.message : String(err) || "Failed to load document",
        );
      });
  }, [selectedDoc, editor]);

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
        <p className="placeholder">Select a document to read it</p>
      </main>
    );
  }

  return (
    <main className="editor-pane editor-pane--active">
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
