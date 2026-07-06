import type { ProposalItem, ProposalSummary } from "../../lib/tauri";

function sourceDocLabel(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

interface Props {
  proposal: ProposalSummary;
  reasoning?: string | null;
  items?: ProposalItem[] | null;
  onSourceClick?: (path: string) => void;
}

export function ReviewEvidencePanel({
  proposal,
  reasoning,
  items,
  onSourceClick,
}: Props) {
  const sources = proposal.source_doc_paths;
  const evidence = (items ?? []).flatMap((item) => item.evidence);

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
        {evidence.length === 0 ? (
          <p className="review-evidence-placeholder">
            Source chunks not available for this proposal yet.
          </p>
        ) : (
          <ul className="review-evidence-sources">
            {evidence.map((chunk, index) => {
              const lineRange =
                chunk.start_line === chunk.end_line
                  ? `L${chunk.start_line}`
                  : `L${chunk.start_line}-${chunk.end_line}`;
              const sourceName = chunk.doc_path ? sourceDocLabel(chunk.doc_path) : "Unknown source";
              const sourceMeta = chunk.source_deleted
                ? `${sourceName} (deleted source) · ${lineRange}`
                : `${sourceName} · ${lineRange}`;
              return (
                <li key={`${chunk.chunk_id}-${chunk.start_line}-${chunk.end_line}-${index}`}>
                  <p className="review-evidence-reasoning">{chunk.quote}</p>
                  <p className="review-evidence-placeholder">{sourceMeta}</p>
                </li>
              );
            })}
          </ul>
        )}
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
