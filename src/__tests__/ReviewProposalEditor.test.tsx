import { screen } from "@testing-library/react";
import { ReviewProposalEditor } from "../components/review/ReviewProposalEditor";
import { renderWithTheme } from "./test-utils";
import { makeProposalDetail, makeProposalSummary } from "./fixtures/proposals";

const SUMMARY = makeProposalSummary({
  id: "prop_1",
  target_name: "Project X",
  created_at: 1,
});

const DETAIL = makeProposalDetail(SUMMARY, {
  reasoning: "Librarian inferred this from meeting notes.",
  items: [
    {
      id: "item_1",
      item_type: "fact_add",
      target_id: null,
      payload: { body: "Test fact body." },
      evidence: [],
      status: "pending",
      edited_payload: null,
    },
  ],
});

test("shows markdown preview for a loaded proposal", () => {
  renderWithTheme(<ReviewProposalEditor detail={DETAIL} />);

  const editor = screen.getByTestId("review-proposal-editor");
  expect(editor).toHaveAttribute("data-variant", "new");
  expect(screen.getByText(/# Project X/)).toBeInTheDocument();
  expect(screen.getByText(/Test fact body/i)).toBeInTheDocument();
});

test("shows loading state while detail is loading", () => {
  renderWithTheme(<ReviewProposalEditor detail={undefined} />);
  expect(screen.getByText(/loading proposal/i)).toBeInTheDocument();
});

test("shows unavailable state when detail is null", () => {
  renderWithTheme(<ReviewProposalEditor detail={null} />);
  expect(screen.getByText(/proposal details unavailable/i)).toBeInTheDocument();
});

test("marks update proposals with the update variant", () => {
  const updateSummary = makeProposalSummary({
    id: "prop_update",
    target_name: "Existing Entity",
    created_at: 2,
    kind: "update_entity",
    entity_id: "ent_existing",
  });
  const updateDetail = makeProposalDetail(updateSummary, {
    kind: "update_entity",
    entity_id: "ent_existing",
  });

  renderWithTheme(<ReviewProposalEditor detail={updateDetail} />);
  expect(screen.getByTestId("review-proposal-editor")).toHaveAttribute(
    "data-variant",
    "update",
  );
});
