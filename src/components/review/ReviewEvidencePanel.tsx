import type { ProposalSummary } from "../../lib/tauri";

function sourceDocLabel(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

interface Props {
  proposal: ProposalSummary;
  reasoning?: string | null;
  onSourceClick?: (path: string) => void;
}

export function ReviewEvidencePanel({
  proposal,
  reasoning,
  onSourceClick,
}: Props) {
  const sources = proposal.source_doc_paths;

  return (
    <aside
      className="sidebar review-evidence-panel"
      aria-label="Source evidence"
    >
      <h3 className="review-evidence-heading">Evidence</h3>

      <section className="review-evidence-section">
        <h4 className="review-evidence-label">Sources</h4>
        {sources.length > 0 ? (
          <ul className="review-evidence-sources">
            {sources.map((path) => (
              <li key={path}>
                <button
                  type="button"
                  className="review-evidence-source"
                  title={path}
                  onClick={() => onSourceClick?.(path)}
                >
                  {sourceDocLabel(path)}
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="review-evidence-placeholder">
            No source documents cited.
          </p>
        )}
      </section>

      <section className="review-evidence-section">
        <h4 className="review-evidence-label">Source chunks</h4>
        <p className="review-evidence-placeholder">
          Source chunks not available for this proposal yet.
        </p>
      </section>

      <section className="review-evidence-section">
        <h4 className="review-evidence-label">Why this proposal</h4>
        {reasoning?.trim() ? (
          <p className="review-evidence-reasoning">{reasoning}</p>
        ) : (
          <p className="review-evidence-placeholder">Not recorded</p>
        )}
      </section>
    </aside>
  );
}
