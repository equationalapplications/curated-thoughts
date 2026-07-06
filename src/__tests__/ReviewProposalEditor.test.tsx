import { screen, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ReviewProposalEditor } from "../components/review/ReviewProposalEditor";
import { defaultItemDecisions } from "../lib/reviewDecisions";
import { renderWithTheme } from "./test-utils";
import { makeProposalDetail, makeProposalSummary } from "./fixtures/proposals";
import type { EntityDetail } from "../lib/tauri";

const NEW_SUMMARY = makeProposalSummary({
  id: "prop_new",
  target_name: "New Entity",
  created_at: 1,
});

const NEW_DETAIL = makeProposalDetail(NEW_SUMMARY, {
  items: [
    {
      id: "item_fact",
      item_type: "fact_add",
      target_id: null,
      payload: { body: "Test fact body." },
      evidence: [],
      status: "pending",
      edited_payload: null,
    },
  ],
});

const UPDATE_SUMMARY = makeProposalSummary({
  id: "prop_update",
  target_name: "Existing Entity",
  created_at: 2,
  kind: "update_entity",
  entity_id: "ent_existing",
});

const ENTITY: EntityDetail = {
  id: "ent_existing",
  name: "Existing Entity",
  entity_type: "concept",
  summary: "Current summary text.",
  created_at: 1,
  updated_at: 2,
  facts: [],
  tasks: [],
  events: [],
};

const UPDATE_DETAIL = makeProposalDetail(UPDATE_SUMMARY, {
  kind: "update_entity",
  entity_id: "ent_existing",
  items: [
    {
      id: "item_summary",
      item_type: "summary_update",
      target_id: null,
      payload: { summary: "Proposed summary text." },
      evidence: [],
      status: "pending",
      edited_payload: null,
    },
    {
      id: "item_fact",
      item_type: "fact_add",
      target_id: null,
      payload: { body: "Another fact." },
      evidence: [],
      status: "pending",
      edited_payload: null,
    },
  ],
});

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("shows per-item toggles for new entity proposals", () => {
  const onItemDecisionChange = vi.fn();
  renderWithTheme(
    <ReviewProposalEditor
      detail={NEW_DETAIL}
      itemDecisions={defaultItemDecisions(NEW_DETAIL)}
      onItemDecisionChange={onItemDecisionChange}
    />,
  );

  const editor = screen.getByTestId("review-proposal-editor");
  expect(editor).toHaveAttribute("data-variant", "new");
  expect(screen.getByText(/Test fact body/i)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /^Reject$/i }));
  expect(onItemDecisionChange).toHaveBeenCalledWith("item_fact", "reject");
});

test("shows loading state while detail is loading", () => {
  renderWithTheme(
    <ReviewProposalEditor
      detail={undefined}
      itemDecisions={new Map()}
      onItemDecisionChange={vi.fn()}
    />,
  );
  expect(screen.getByText(/loading proposal/i)).toBeInTheDocument();
});

test("shows unavailable state when detail is null", () => {
  renderWithTheme(
    <ReviewProposalEditor
      detail={null}
      itemDecisions={new Map()}
      onItemDecisionChange={vi.fn()}
    />,
  );
  expect(screen.getByText(/proposal details unavailable/i)).toBeInTheDocument();
});

test("shows ProposalDiff for update_entity proposals via getEntity", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "get_entity_cmd") return Promise.resolve(ENTITY);
    return Promise.resolve(null);
  });

  renderWithTheme(
    <ReviewProposalEditor
      detail={UPDATE_DETAIL}
      itemDecisions={defaultItemDecisions(UPDATE_DETAIL)}
      onItemDecisionChange={vi.fn()}
    />,
  );

  const diff = await screen.findByTestId("proposal-diff");
  expect(diff).toBeInTheDocument();
  expect(screen.getByTestId("review-proposal-editor")).toHaveAttribute(
    "data-variant",
    "update",
  );
  expect(diff.querySelector(".proposal-diff-removed")).toBeTruthy();
  expect(diff.querySelector(".proposal-diff-added")).toBeTruthy();
  expect(screen.getByText(/Proposed summary text/i)).toBeInTheDocument();
  expect(screen.getByText(/Another fact/i)).toBeInTheDocument();
});
