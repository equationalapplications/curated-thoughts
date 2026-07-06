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
): ItemDecision[] {
  return detail.items.map((item) => ({
    item_id: item.id,
    decision: itemDecisions.get(item.id) ?? "accept",
  }));
}

export function hasAcceptedItems(
  itemDecisions: Map<string, ItemDecisionState>,
): boolean {
  for (const decision of itemDecisions.values()) {
    if (decision === "accept") return true;
  }
  return false;
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
