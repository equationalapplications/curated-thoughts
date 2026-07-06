import { ReviewPage } from "../../lib/tauri";
import { sortReviewQueue } from "../../lib/reviewQueue";

function parseSourceIds(json: string): string[] {
  try {
    const parsed: unknown = JSON.parse(json || "[]");
    return Array.isArray(parsed) ? parsed.map(String) : [];
  } catch {
    return [];
  }
}

function sourceLabel(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

interface Props {
  queue: ReviewPage[];
  selectedId: number | null;
  checkedIds: ReadonlySet<number>;
  onSelect: (id: number) => void;
  onToggleChecked: (id: number, checked: boolean) => void;
  onBatchApprove?: () => void;
  batchBusy?: boolean;
}

export function ReviewQueueList({
  queue,
  selectedId,
  checkedIds,
  onSelect,
  onToggleChecked,
  onBatchApprove,
  batchBusy = false,
}: Props) {
  const sorted = sortReviewQueue(queue);
  const batchCount = checkedIds.size;

  return (
    <aside className="sidebar review-queue-list">
      <div className="review-queue-header">
        <h3 className="review-queue-heading">Queue ({queue.length})</h3>
        {batchCount > 0 && onBatchApprove && (
          <button
            type="button"
            className="review-batch-approve"
            onClick={onBatchApprove}
            disabled={batchBusy}
          >
            Approve {batchCount} selected
          </button>
        )}
      </div>
      <ul className="review-queue-cards" aria-label="Review queue">
        {sorted.map((page) => {
          const sources = parseSourceIds(page.source_doc_ids).map(sourceLabel);
          const active = selectedId === page.id;
          const checked = checkedIds.has(page.id);

          return (
            <li key={page.id} className="review-queue-row">
              <input
                type="checkbox"
                className="review-queue-check"
                checked={checked}
                aria-label={`Select ${page.path}`}
                onChange={(event) =>
                  onToggleChecked(page.id, event.target.checked)
                }
              />
              <button
                type="button"
                className={`review-queue-item${
                  active ? " review-queue-item--active" : ""
                }`}
                onClick={() => onSelect(page.id)}
                aria-pressed={active}
              >
                <span className="review-queue-item-path">{page.path}</span>
                <span className="review-queue-item-model">
                  {page.generated_by}
                </span>
                {sources.length > 0 && (
                  <span className="review-queue-item-sources">
                    {sources.join(", ")}
                  </span>
                )}
              </button>
            </li>
          );
        })}
      </ul>
    </aside>
  );
}
