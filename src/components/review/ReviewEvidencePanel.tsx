import { ReviewPage } from "../../lib/tauri";

function parseSourceDocIds(json: string): string[] {
  try {
    const parsed: unknown = JSON.parse(json || "[]");
    return Array.isArray(parsed) ? parsed.map(String) : [];
  } catch {
    return [];
  }
}

function sourceDocLabel(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

interface Props {
  page: ReviewPage;
  onSourceClick?: (path: string) => void;
}

export function ReviewEvidencePanel({ page, onSourceClick }: Props) {
  const sources = parseSourceDocIds(page.source_doc_ids);

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
        {page.reasoning_summary ? (
          <p className="review-evidence-reasoning">
            {page.reasoning_summary}
          </p>
        ) : (
          <p className="review-evidence-placeholder">Not recorded</p>
        )}
      </section>
    </aside>
  );
}
