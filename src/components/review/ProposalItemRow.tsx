import { useState, useEffect } from "react";
import type { EntityDetail, ProposalItem } from "../../lib/tauri";
import type { ItemDecisionState } from "../../lib/reviewDecisions";
import { describeProposalItem } from "../../lib/proposalEntityPreview";

interface Props {
  item: ProposalItem;
  decision: ItemDecisionState;
  entity?: EntityDetail | null;
  onDecisionChange: (itemId: string, decision: ItemDecisionState) => void;
  onEditSave?: (itemId: string, payload: Record<string, unknown>) => void;
}

function editableFieldForItem(item: ProposalItem): string | null {
  if (item.item_type === "fact_add" || item.item_type === "fact_update") {
    return "body";
  }
  if (item.item_type === "summary_update") {
    return "summary";
  }
  if (item.item_type === "task_add") {
    return "description";
  }
  return null;
}

export function ProposalItemRow({
  item,
  decision,
  entity,
  onDecisionChange,
  onEditSave,
}: Props) {
  const editableField = editableFieldForItem(item);
  const currentPayload = item.edited_payload ?? item.payload;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(
    editableField ? String(currentPayload[editableField] ?? "") : "",
  );

  // Resync state when item changes
  useEffect(() => {
    setEditing(false);
    setDraft(editableField ? String(currentPayload[editableField] ?? "") : "");
  }, [item.id]);

  const startEdit = () => {
    setDraft(editableField ? String(currentPayload[editableField] ?? "") : "");
    setEditing(true);
  };

  const save = () => {
    if (!editableField) return;
    onEditSave?.(item.id, { ...item.payload, [editableField]: draft });
    onDecisionChange(item.id, "accept");
    setEditing(false);
  };

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
        {editing && editableField ? (
          <textarea
            className="proposal-item-detail"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            rows={3}
            style={{ width: "100%" }}
          />
        ) : (
          <p className="proposal-item-detail">{detail}</p>
        )}
      </div>
      <div
        className="proposal-item-actions"
        role="group"
        aria-label={`${label} decision`}
      >
        {editing ? (
          <>
            <button
              type="button"
              className="proposal-item-btn proposal-item-btn--accept"
              onClick={save}
            >
              Save
            </button>
            <button
              type="button"
              className="proposal-item-btn"
              onClick={() => setEditing(false)}
            >
              Cancel
            </button>
          </>
        ) : (
          <>
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
            {editableField && (
              <button
                type="button"
                className="proposal-item-btn"
                onClick={startEdit}
              >
                Edit
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
