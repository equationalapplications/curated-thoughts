import type { ProposalDetail, ProposalSummary } from "../lib/tauri";

export function makeProposalSummary(
  overrides: Partial<ProposalSummary> & Pick<ProposalSummary, "id" | "target_name" | "created_at">,
): ProposalSummary {
  return {
    kind: "new_entity",
    entity_id: null,
    source_doc_paths: [],
    item_counts: {
      total: 1,
      facts: 1,
      edges: 0,
      tasks: 0,
      summary_updates: 0,
    },
    age_secs: 60,
    model: "llama3.2:3b",
    ...overrides,
  };
}

export function makeProposalDetail(
  summary: ProposalSummary,
  overrides: Partial<ProposalDetail> = {},
): ProposalDetail {
  return {
    id: summary.id,
    kind: summary.kind,
    entity_id: summary.entity_id,
    proposed_name: summary.kind === "new_entity" ? summary.target_name : null,
    proposed_type: summary.kind === "new_entity" ? "concept" : null,
    target_name: summary.target_name,
    reasoning: null,
    model: summary.model,
    status: "pending",
    created_at: summary.created_at,
    source_doc_paths: summary.source_doc_paths,
    items: [
      {
        id: "item_fact_1",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "Test fact body." },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
    ],
    ...overrides,
  };
}
