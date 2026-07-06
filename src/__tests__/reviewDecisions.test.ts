import {
  buildDecisions,
  defaultItemDecisions,
  hasAcceptedItems,
} from "../lib/reviewDecisions";
import { makeProposalDetail, makeProposalSummary } from "./fixtures/proposals";

const SUMMARY = makeProposalSummary({
  id: "prop_1",
  target_name: "Project X",
  created_at: 1,
});

test("defaultItemDecisions accepts every item", () => {
  const detail = makeProposalDetail(SUMMARY, {
    items: [
      {
        id: "item_a",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "A" },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
      {
        id: "item_b",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "B" },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
    ],
  });

  const decisions = defaultItemDecisions(detail);
  expect(decisions.get("item_a")).toBe("accept");
  expect(decisions.get("item_b")).toBe("accept");
});

test("buildDecisions respects per-item overrides", () => {
  const detail = makeProposalDetail(SUMMARY, {
    items: [
      {
        id: "item_a",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "A" },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
      {
        id: "item_b",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "B" },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
    ],
  });

  const overrides = new Map([
    ["item_a", "accept"],
    ["item_b", "reject"],
  ] as const);

  expect(buildDecisions(detail, overrides)).toEqual([
    { item_id: "item_a", decision: "accept" },
    { item_id: "item_b", decision: "reject" },
  ]);
});

test("hasAcceptedItems is false when every item is rejected", () => {
  const detail = makeProposalDetail(SUMMARY, {
    items: [
      {
        id: "item_a",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "A" },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
      {
        id: "item_b",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "B" },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
    ],
  });
  const decisions = new Map([
    ["item_a", "reject"],
    ["item_b", "reject"],
  ] as const);
  expect(hasAcceptedItems(detail, decisions)).toBe(false);
});

test("hasAcceptedItems defaults missing map entries to accept", () => {
  const detail = makeProposalDetail(SUMMARY, {
    items: [
      {
        id: "item_a",
        item_type: "fact_add",
        target_id: null,
        payload: { body: "A" },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
    ],
  });
  expect(hasAcceptedItems(detail, new Map())).toBe(true);
});
