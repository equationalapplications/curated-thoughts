import type { ItemDecision, ProposalDetail } from "./tauri";

export type ItemDecisionState = "accept" | "reject";

export function defaultItemDecisions(
  detail: ProposalDetail,
): Map<string, ItemDecisionState> {
  return new Map(detail.items.map((item) => [item.id, "accept"]));
}

export function buildDecisions(
  detail: ProposalDetail,
  itemDecisions: Map<string, ItemDecisionState>,
  itemEdits?: Map<string, Record<string, unknown>>,
): ItemDecision[] {
  return detail.items.map((item) => {
    const decision: ItemDecision = {
      item_id: item.id,
      decision: itemDecisions.get(item.id) ?? "accept",
    };
    const editedPayload = itemEdits?.get(item.id);
    if (editedPayload) decision.edited_payload = editedPayload;
    return decision;
  });
}

export function hasAcceptedItems(
  detail: ProposalDetail,
  itemDecisions: Map<string, ItemDecisionState>,
): boolean {
  return detail.items.some(
    (item) => (itemDecisions.get(item.id) ?? "accept") === "accept",
  );
}

export function allAcceptDecisions(detail: ProposalDetail): ItemDecision[] {
  return buildDecisions(detail, defaultItemDecisions(detail));
}

export function allRejectDecisions(detail: ProposalDetail): ItemDecision[] {
  return detail.items.map((item) => ({
    item_id: item.id,
    decision: "reject",
  }));
}
