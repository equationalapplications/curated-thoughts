import type { ProposalSummary } from "../../lib/tauri";
import { sortReviewQueue } from "../../lib/reviewQueue";

function sourceLabel(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

function kindLabel(kind: ProposalSummary["kind"]): string {
  return kind === "new_entity" ? "New entity" : "Update";
}

function itemCountLabel(counts: ProposalSummary["item_counts"]): string {
  const parts: string[] = [];
  if (counts.facts > 0) {
    parts.push(`${counts.facts} fact${counts.facts === 1 ? "" : "s"}`);
  }
  if (counts.summary_updates > 0) {
    parts.push(
      `${counts.summary_updates} summary update${counts.summary_updates === 1 ? "" : "s"}`,
    );
  }
  if (counts.edges > 0) {
    parts.push(`${counts.edges} edge${counts.edges === 1 ? "" : "s"}`);
  }
  if (counts.tasks > 0) {
    parts.push(`${counts.tasks} task${counts.tasks === 1 ? "" : "s"}`);
  }
  return parts.length > 0 ? parts.join(", ") : "No items";
}

interface Props {
  queue: ProposalSummary[];
  selectedId: string | null;
  checkedIds: ReadonlySet<string>;
  onSelect: (id: string) => void;
  onToggleChecked: (id: string, checked: boolean) => void;
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
        {sorted.map((proposal) => {
          const sources = proposal.source_doc_paths.map(sourceLabel);
          const active = selectedId === proposal.id;
          const checked = checkedIds.has(proposal.id);

          return (
            <li key={proposal.id} className="review-queue-row">
              <input
                type="checkbox"
                className="review-queue-check"
                checked={checked}
                aria-label={`Select ${proposal.target_name}`}
                onChange={(event) =>
                  onToggleChecked(proposal.id, event.target.checked)
                }
              />
              <button
                type="button"
                className={`review-queue-item${
                  active ? " review-queue-item--active" : ""
                }`}
                onClick={() => onSelect(proposal.id)}
                aria-pressed={active}
              >
                <span className="review-queue-item-path">
                  {proposal.target_name}
                </span>
                <span className="review-queue-item-kind">{kindLabel(proposal.kind)}</span>
                <span className="review-queue-item-model">{proposal.model}</span>
                <span className="review-queue-item-counts">
                  {itemCountLabel(proposal.item_counts)}
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
