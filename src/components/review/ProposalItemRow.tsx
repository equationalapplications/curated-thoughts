import { useEffect, useState } from "react";
import type { EntityDetail, ProposalItem } from "../../lib/tauri";
import type { ItemDecisionState } from "../../lib/reviewDecisions";
import {
  describeProposalItem,
  editableFieldForItem,
} from "../../lib/proposalEntityPreview";

interface Props {
  item: ProposalItem;
  decision: ItemDecisionState;
  entity?: EntityDetail | null;
  onDecisionChange: (itemId: string, decision: ItemDecisionState) => void;
  onEditSave: (itemId: string, editedPayload: Record<string, unknown>) => void;
}

export function ProposalItemRow({
  item,
  decision,
  entity,
  onDecisionChange,
  onEditSave,
}: Props) {
  const { label, detail } = describeProposalItem(item, entity);
  const accepted = decision === "accept";
  const editableField = editableFieldForItem(item);
  const [editing, setEditing] = useState(false);
  const currentPayload = item.edited_payload ?? item.payload;
  const [draft, setDraft] = useState(
    editableField ? String(currentPayload[editableField] ?? "") : "",
  );

  // When the parent reuses this row for a different item, reset any in-flight
  // edit so the form/draft don't bleed across items.
  useEffect(() => {
    setEditing(false);
    setDraft(
      editableField ? String(currentPayload[editableField] ?? "") : "",
    );
    // item.id is the stable identity for the proposal row.
  }, [item.id]);

  const startEdit = () => {
    setDraft(editableField ? String(currentPayload[editableField] ?? "") : "");
    setEditing(true);
  };

  const save = () => {
    if (!editableField) return;
    onEditSave(item.id, { ...item.payload, [editableField]: draft });
    onDecisionChange(item.id, "accept");
    setEditing(false);
  };

  return (
    <div
      className="proposal-item-row"
      data-item-type={item.item_type}
      data-decision={decision}
      data-testid={`proposal-item-${item.id}`}
    >
      <div className="proposal-item-body">
        <span className="proposal-item-label">
          {label}
          {item.edited_payload ? (
            <span className="proposal-item-edited-chip">edited</span>
          ) : null}
        </span>
        {editing ? (
          <div className="proposal-item-edit">
            <textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
            <div className="proposal-item-edit-actions">
              <button type="button" onClick={save}>
                Save
              </button>
              <button type="button" onClick={() => setEditing(false)}>
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <p className="proposal-item-detail">{detail}</p>
        )}
      </div>
      <div
        className="proposal-item-actions"
        role="group"
        aria-label={`${label} decision`}
      >
        {editableField && !editing ? (
          <button type="button" className="proposal-item-btn" onClick={startEdit}>
            Edit
          </button>
        ) : null}
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
