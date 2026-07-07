import { useState } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { updateEntitySummary } from "../../lib/tauri";
import { useTheme } from "../../lib/ThemeContext";
import { WikilinkText } from "./WikilinkText";

interface Props {
  entityId: string;
  summary: string;
  onChanged: () => void;
  onNavigateEntity: (name: string) => void;
}

export function EntitySummarySection({
  entityId,
  summary,
  onChanged,
  onNavigateEntity,
}: Props) {
  const editor = useCreateBlockNote();
  const { resolved: theme } = useTheme();
  const [editing, setEditing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function startEdit() {
    setError(null);
    const blocks = await editor.tryParseMarkdownToBlocks(summary);
    editor.replaceBlocks(editor.document, blocks);
    setEditing(true);
  }

  async function save() {
    setError(null);
    try {
      const markdown = await editor.blocksToMarkdownLossy(editor.document);
      await updateEntitySummary(entityId, markdown);
      setEditing(false);
      onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <section className="entity-summary">
      <div className="entity-section-header">
        <h3>Summary</h3>
        {editing ? (
          <div className="entity-summary-actions">
            <button type="button" onClick={save}>
              Save
            </button>
            <button type="button" onClick={() => setEditing(false)}>
              Cancel
            </button>
          </div>
        ) : (
          <button type="button" onClick={startEdit}>
            Edit summary
          </button>
        )}
      </div>
      {editing ? (
        <BlockNoteView editor={editor} editable theme={theme} />
      ) : summary.trim() ? (
        <p className="entity-summary-prose">
          <WikilinkText text={summary} onNavigate={onNavigateEntity} />
        </p>
      ) : (
        <p className="placeholder">No summary yet.</p>
      )}
      {error && (
        <p className="entity-summary-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
