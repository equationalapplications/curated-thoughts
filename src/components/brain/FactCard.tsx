import { useState } from "react";
import {
  archiveEntityFact,
  updateEntityFact,
  type EntityFact,
} from "../../lib/tauri";
import { FactPowerMenu } from "./FactPowerMenu";
import { WikilinkText } from "./WikilinkText";

function docLabel(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

interface Props {
  entityId: string;
  fact: EntityFact;
  onChanged: () => void;
  onNavigateEntity: (name: string) => void;
  /**
   * Called when the user clicks a source-document chip. The chunk id is the
   * optional anchor within `path` that Library will scroll/highlight on
   * open. v1: `source_docs` does not yet carry chunk ids so callers pass
   * `null` here; the click path stays unchanged when chunk id is absent.
   */
  onOpenSource: (path: string, chunkId?: string | null) => void;
}

export function FactCard({
  entityId,
  fact,
  onChanged,
  onNavigateEntity,
  onOpenSource,
}: Props) {
  const [editing, setEditing] = useState(false);
  const [powerOpen, setPowerOpen] = useState(false);
  const [draft, setDraft] = useState(fact.body);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    setError(null);
    try {
      await updateEntityFact(entityId, fact.id, draft);
      setEditing(false);
      onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function archive() {
    setError(null);
    try {
      await archiveEntityFact(entityId, fact.id);
      onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <article className="fact-card">
      {editing ? (
        <div className="fact-card-editor">
          <textarea
            aria-label="Fact body"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <div className="fact-card-actions">
            <button type="button" onClick={save}>
              Save
            </button>
            <button
              type="button"
              onClick={() => {
                setDraft(fact.body);
                setEditing(false);
              }}
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <>
          <p className="fact-card-body">
            <WikilinkText text={fact.body} onNavigate={onNavigateEntity} />
          </p>
          <div className="fact-card-meta">
            <span className="fact-chip fact-chip--confidence">{fact.confidence}</span>
            <span className="fact-chip">{fact.source_type}</span>
            {fact.source_docs.map((path) => (
              <button
                key={path}
                type="button"
                className="fact-chip fact-chip--source"
                title={path}
                onClick={() => onOpenSource(path, null)}
              >
                {docLabel(path)}
              </button>
            ))}
            <span className="fact-card-date">
              {new Date(fact.updated_at).toLocaleDateString()}
            </span>
          </div>
          <div className="fact-card-actions">
            <button type="button" onClick={() => setEditing(true)}>
              Edit
            </button>
            <button type="button" onClick={archive}>
              Archive
            </button>
            <button
              type="button"
              onClick={() => setPowerOpen((v) => !v)}
              aria-label="Fact details"
              aria-expanded={powerOpen}
            >
              …
            </button>
          </div>
        </>
      )}
      {error && (
        <p className="fact-card-error" role="alert">
          {error}
        </p>
      )}
      <FactPowerMenu
        fact={fact}
        open={powerOpen}
        onClose={() => setPowerOpen(false)}
      />
    </article>
  );
}
