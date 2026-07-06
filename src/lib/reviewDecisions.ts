import type { ItemDecision, ProposalDetail } from "./tauri";

export function allAcceptDecisions(detail: ProposalDetail): ItemDecision[] {
  return detail.items.map((item) => ({
    item_id: item.id,
    decision: "accept",
  }));
}

export function allRejectDecisions(detail: ProposalDetail): ItemDecision[] {
  return detail.items.map((item) => ({
    item_id: item.id,
    decision: "reject",
  }));
}
