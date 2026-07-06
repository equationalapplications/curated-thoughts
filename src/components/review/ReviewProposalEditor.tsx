import { useEffect, useState, type RefObject } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument, ReviewPage } from "../../lib/tauri";
import { useTheme } from "../../lib/ThemeContext";
import { ProposalDiff } from "./ProposalDiff";

interface Props {
  page: ReviewPage;
  proposedContent: string | null;
  onEditedContentChange: (content: string) => void;
  containerRef?: RefObject<HTMLDivElement | null>;
}

export function ReviewProposalEditor({
  page,
  proposedContent,
  onEditedContentChange,
  containerRef,
}: Props) {
  const editor = useCreateBlockNote();
  const { resolved: theme } = useTheme();
  const [currentContent, setCurrentContent] = useState<
    string | null | undefined
  >(undefined);

  useEffect(() => {
    let cancelled = false;
    setCurrentContent(undefined);
    readDocument(page.path)
      .then((content) => {
        if (!cancelled) setCurrentContent(content);
      })
      .catch(() => {
        if (!cancelled) setCurrentContent(null);
      });
    return () => {
      cancelled = true;
    };
  }, [page.id, page.path]);

  useEffect(() => {
    if (proposedContent === null || currentContent !== null) return;

    let cancelled = false;
    void (async () => {
      try {
        const blocks = await editor.tryParseMarkdownToBlocks(proposedContent);
        if (cancelled) return;
        editor.replaceBlocks(editor.document, blocks);
        onEditedContentChange(proposedContent);
      } catch {
        if (!cancelled) onEditedContentChange(proposedContent);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [proposedContent, currentContent, editor, onEditedContentChange]);

  useEffect(() => {
    if (currentContent !== null) return;

    return editor.onChange(async () => {
      const markdown = await editor.blocksToMarkdownLossy(editor.document);
      onEditedContentChange(markdown);
    });
  }, [currentContent, editor, onEditedContentChange]);

  useEffect(() => {
    if (currentContent === null || currentContent === undefined) return;
    if (proposedContent === null) return;
    onEditedContentChange(proposedContent);
  }, [currentContent, proposedContent, onEditedContentChange]);

  if (proposedContent === null || currentContent === undefined) {
    return <p className="review-hint">Loading proposal…</p>;
  }

  if (currentContent === null) {
    return (
      <div
        className="review-proposal-editor"
        data-variant="new"
        data-testid="review-proposal-editor"
        ref={containerRef}
        tabIndex={-1}
      >
        <BlockNoteView editor={editor} editable={false} theme={theme} />
      </div>
    );
  }

  return (
    <div
      className="review-proposal-editor"
      data-variant="update"
      data-testid="review-proposal-editor"
      ref={containerRef}
      tabIndex={-1}
    >
      <ProposalDiff oldText={currentContent} newText={proposedContent} />
    </div>
  );
}
