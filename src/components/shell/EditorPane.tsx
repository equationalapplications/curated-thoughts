import { useEffect } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument } from "../../lib/tauri";

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
      <BlockNoteView editor={editor} editable={isWiki} theme="light" />
    </main>
  );
}
