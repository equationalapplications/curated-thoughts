import { render, screen, fireEvent } from "@testing-library/react";
import { ProposalItemRow } from "../components/review/ProposalItemRow";
import type { ProposalItem } from "../lib/tauri";

function makeItem(overrides: Partial<ProposalItem> = {}): ProposalItem {
  return {
    id: "item_a",
    item_type: "fact_add",
    target_id: null,
    payload: { body: "Original body" },
    evidence: [],
    status: "pending",
    edited_payload: null,
    ...overrides,
  };
}

test("shows an Edit affordance for a fact_add item", () => {
  render(
    <ProposalItemRow
      item={makeItem()}
      decision="accept"
      onDecisionChange={vi.fn()}
      onEditSave={vi.fn()}
    />,
  );
  expect(screen.getByRole("button", { name: "Edit" })).toBeInTheDocument();
});

test("editing and saving calls onEditSave with the merged payload and marks accepted", () => {
  const onEditSave = vi.fn();
  const onDecisionChange = vi.fn();
  render(
    <ProposalItemRow
      item={makeItem()}
      decision="reject"
      onDecisionChange={onDecisionChange}
      onEditSave={onEditSave}
    />,
  );

  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  const textarea = screen.getByRole("textbox");
  fireEvent.change(textarea, { target: { value: "Edited body" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));

  expect(onEditSave).toHaveBeenCalledWith("item_a", { body: "Edited body" });
  expect(onDecisionChange).toHaveBeenCalledWith("item_a", "accept");
});

test("shows an edited chip once the item has an edited_payload", () => {
  render(
    <ProposalItemRow
      item={makeItem({ edited_payload: { body: "Edited body" } })}
      decision="accept"
      onDecisionChange={vi.fn()}
      onEditSave={vi.fn()}
    />,
  );
  expect(screen.getByText("edited")).toBeInTheDocument();
});

test("no Edit affordance for edge_add or fact_archive items", () => {
  render(
    <ProposalItemRow
      item={makeItem({ item_type: "edge_add", payload: { edge_type: "related" } })}
      decision="accept"
      onDecisionChange={vi.fn()}
      onEditSave={vi.fn()}
    />,
  );
  expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
});
