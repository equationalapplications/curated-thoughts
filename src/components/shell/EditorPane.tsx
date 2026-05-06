import { useEffect } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument, saveWikiPage } from "../../lib/tauri";

interface Props {
  selectedDoc: string | null;
  isWiki: boolean;
}

export function EditorPane({ selectedDoc, isWiki }: Props) {
  const editor = useCreateBlockNote();

  useEffect(() => {
    if (!selectedDoc) return;
    readDocument(selectedDoc)
      .then(async (content) => {
        const blocks = await editor.tryParseMarkdownToBlocks(content);
        editor.replaceBlocks(editor.document, blocks);
      })
      .catch(() => {});
  }, [selectedDoc, editor]);

  async function handleSave() {
    if (!selectedDoc || !isWiki) return;
    try {
      const md = await editor.blocksToMarkdownLossy(editor.document);
      const filename = selectedDoc.split("/").at(-1) ?? "page.md";
      await saveWikiPage(filename, md);
    } catch {}
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
          <button className="editor-save-btn" onClick={handleSave}>Save</button>
        </div>
      )}
      <BlockNoteView editor={editor} editable={isWiki} theme="light" />
    </main>
  );
}
