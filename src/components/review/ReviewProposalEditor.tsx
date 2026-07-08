import { useEffect, useRef, useState, type RefObject } from "react";
import { getEntity, type EntityDetail, type ProposalDetail } from "../../lib/tauri";
import type { ItemDecisionState } from "../../lib/reviewDecisions";
import {
  findSummaryUpdateItem,
  nonSummaryItems,
  proposedSummaryText,
  summaryTextFromItem,
} from "../../lib/proposalEntityPreview";
import { ProposalDiff } from "./ProposalDiff";
import { ProposalItemRow } from "./ProposalItemRow";

interface Props {
  detail: ProposalDetail | null | undefined;
  itemDecisions: Map<string, ItemDecisionState>;
  onItemDecisionChange: (itemId: string, decision: ItemDecisionState) => void;
  onItemEditSave: (itemId: string, editedPayload: Record<string, unknown>) => void;
  containerRef?: RefObject<HTMLDivElement | null>;
}

export function ReviewProposalEditor({
  detail,
  itemDecisions,
  onItemDecisionChange,
  onItemEditSave,
  containerRef,
}: Props) {
  const [entity, setEntity] = useState<EntityDetail | null | undefined>(
    undefined,
  );
  const entityRequestSeq = useRef(0);

  useEffect(() => {
    setEntity(undefined);
    if (!detail || detail.kind !== "update_entity" || !detail.entity_id) {
      setEntity(null);
      return;
    }

    entityRequestSeq.current += 1;
    const requestSeq = entityRequestSeq.current;
    getEntity(detail.entity_id)
      .then((loaded) => {
        if (entityRequestSeq.current !== requestSeq) return;
        setEntity(loaded);
      })
      .catch(() => {
        if (entityRequestSeq.current === requestSeq) setEntity(null);
      });
  }, [detail?.id, detail?.kind, detail?.entity_id]);

  if (detail === undefined) {
    return <p className="review-hint">Loading proposal…</p>;
  }
  if (detail === null) {
    return <p className="review-hint">Proposal details unavailable.</p>;
  }

  const variant = detail.kind === "new_entity" ? "new" : "update";
  const summaryItem = findSummaryUpdateItem(detail);
  const otherItems = nonSummaryItems(detail);
  const entityLoading =
    detail.kind === "update_entity" && detail.entity_id && entity === undefined;
  const proposedSummary = proposedSummaryText(detail);

  return (
    <div
      className="review-proposal-editor"
      data-variant={variant}
      data-testid="review-proposal-editor"
      ref={containerRef}
      tabIndex={-1}
    >
      <header className="review-proposal-header">
        <h2 className="review-proposal-title">{detail.target_name}</h2>
        {detail.kind === "new_entity" && detail.proposed_type && (
          <span className="review-proposal-type">{detail.proposed_type}</span>
        )}
      </header>

      {entityLoading && (
        <p className="review-hint">Loading current entity…</p>
      )}

      {detail.kind === "update_entity" && summaryItem && !entityLoading && (
        <section className="review-proposal-section">
          <h3 className="review-proposal-section-title">Summary</h3>
          {entity ? (
            <ProposalDiff
              oldText={entity.summary}
              newText={proposedSummary || summaryTextFromItem(summaryItem)}
            />
          ) : (
            <>
              <p className="review-hint">
                Current entity unavailable — showing proposed summary only.
              </p>
              <pre className="review-proposal-preview">
                {proposedSummary || summaryTextFromItem(summaryItem)}
              </pre>
            </>
          )}
          <ProposalItemRow
            item={summaryItem}
            decision={itemDecisions.get(summaryItem.id) ?? "accept"}
            entity={entity}
            onDecisionChange={onItemDecisionChange}
            onEditSave={onItemEditSave}
          />
        </section>
      )}

      {detail.kind === "new_entity" && summaryItem && proposedSummary && (
        <section className="review-proposal-section">
          <h3 className="review-proposal-section-title">Summary</h3>
          <pre className="review-proposal-preview">{proposedSummary}</pre>
          <ProposalItemRow
            item={summaryItem}
            decision={itemDecisions.get(summaryItem.id) ?? "accept"}
            entity={entity}
            onDecisionChange={onItemDecisionChange}
            onEditSave={onItemEditSave}
          />
        </section>
      )}

      {(detail.kind === "new_entity"
        ? detail.items.filter(
            (item) =>
              item.item_type !== "summary_update" || !proposedSummary,
          )
        : otherItems
      ).length > 0 ? (
        <section className="review-proposal-section">
          <h3 className="review-proposal-section-title">Proposed changes</h3>
          <div className="proposal-item-list">
            {(detail.kind === "new_entity"
              ? detail.items.filter(
                  (item) =>
                    item.item_type !== "summary_update" || !proposedSummary,
                )
              : otherItems
            ).map((item) => (
              <ProposalItemRow
                key={item.id}
                item={item}
                decision={itemDecisions.get(item.id) ?? "accept"}
                entity={entity}
                onDecisionChange={onItemDecisionChange}
                onEditSave={onItemEditSave}
              />
            ))}
          </div>
        </section>
      ) : (
        detail.items.length === 0 && (
          <p className="review-hint">No proposed items.</p>
        )
      )}
    </div>
  );
}
