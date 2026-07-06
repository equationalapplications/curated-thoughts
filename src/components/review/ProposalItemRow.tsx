import type { EntityDetail, ProposalItem } from "../../lib/tauri";
import type { ItemDecisionState } from "../../lib/reviewDecisions";
import { describeProposalItem } from "../../lib/proposalEntityPreview";

interface Props {
  item: ProposalItem;
  decision: ItemDecisionState;
  entity?: EntityDetail | null;
  onDecisionChange: (itemId: string, decision: ItemDecisionState) => void;
}

export function ProposalItemRow({
  item,
  decision,
  entity,
  onDecisionChange,
}: Props) {
  const { label, detail } = describeProposalItem(item, entity);
  const accepted = decision === "accept";

  return (
    <div
      className="proposal-item-row"
      data-item-type={item.item_type}
      data-decision={decision}
      data-testid={`proposal-item-${item.id}`}
    >
      <div className="proposal-item-body">
        <span className="proposal-item-label">{label}</span>
        <p className="proposal-item-detail">{detail}</p>
      </div>
      <div
        className="proposal-item-actions"
        role="group"
        aria-label={`${label} decision`}
      >
        <button
          type="button"
          className={`proposal-item-btn proposal-item-btn--accept${accepted ? " proposal-item-btn--active" : ""}`}
          aria-pressed={accepted}
          onClick={() => onDecisionChange(item.id, "accept")}
        >
          Accept
        </button>
        <button
          type="button"
          className={`proposal-item-btn proposal-item-btn--reject${!accepted ? " proposal-item-btn--active" : ""}`}
          aria-pressed={!accepted}
          onClick={() => onDecisionChange(item.id, "reject")}
        >
          Reject
        </button>
      </div>
    </div>
  );
}
