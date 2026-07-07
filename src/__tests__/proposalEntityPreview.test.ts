import { describeProposalItem, proposedSummaryText } from "../lib/proposalEntityPreview";
import { makeProposalDetail, makeProposalSummary } from "./fixtures/proposals";
import type { EntityDetail } from "../lib/tauri";

const SUMMARY = makeProposalSummary({
  id: "prop_1",
  target_name: "Project X",
  created_at: 1,
});

const ENTITY: EntityDetail = {
  id: "ent_1",
  name: "Project X",
  entity_type: "project",
  summary: "Existing summary prose.",
  created_at: 1,
  updated_at: 2,
  facts: [
    {
      id: "fact_old",
      title: "",
      body: "Old fact body.",
      tags: [],
      confidence: "certain",
      source_type: "user_confirmed",
      source_docs: [],
      updated_at: 2,
    },
  ],
  tasks: [],
  events: [],
};

test("proposedSummaryText reads summary_update payload", () => {
  const detail = makeProposalDetail(SUMMARY, {
    items: [
      {
        id: "item_summary",
        item_type: "summary_update",
        target_id: null,
        payload: { summary: "Proposed summary." },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
    ],
  });

  expect(proposedSummaryText(detail)).toBe("Proposed summary.");
});

test("describeProposalItem shows empty-string fact body as previous value", () => {
  const entityWithEmptyFact: EntityDetail = {
    ...ENTITY,
    facts: [
      {
        id: "fact_empty",
        title: "",
        body: "",
        tags: [],
        confidence: "certain",
        source_type: "user_confirmed",
        source_docs: [],
        updated_at: 2,
      },
    ],
  };
  const item = {
    id: "item_update",
    item_type: "fact_update",
    target_id: "fact_empty",
    payload: { body: "New body." },
    evidence: [],
    status: "pending",
    edited_payload: null,
  };

  expect(describeProposalItem(item, entityWithEmptyFact)).toEqual({
    label: "Update fact",
    detail: " → New body.",
  });
});

test("describeProposalItem shows fact update with previous body", () => {
  const item = {
    id: "item_update",
    item_type: "fact_update",
    target_id: "fact_old",
    payload: { body: "Updated fact body." },
    evidence: [],
    status: "pending",
    edited_payload: null,
  };

  expect(describeProposalItem(item, ENTITY)).toEqual({
    label: "Update fact",
    detail: "Old fact body. → Updated fact body.",
  });
});
