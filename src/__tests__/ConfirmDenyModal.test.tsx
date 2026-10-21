import { fireEvent, screen, waitFor } from "@testing-library/react";
import { ConfirmDenyModal } from "../components/queue/ConfirmDenyModal";
import { renderWithTheme } from "./test-utils";
import type { QueueRow } from "../lib/tauri";

const ROW: QueueRow = {
  fact_id: "fact_1",
  title: "Some fact",
  body: "Some fact body.",
  provenance: "librarian_inferred",
  entity_id: "ent_1",
  evidence_quote: "…some fact body…",
  source_document: "documents/note.md",
  attributed_speaker: null,
  instruction_source: null,
  age_days: 2,
};

// The modal is callback-wired (QueueMode owns the invoke calls — covered in
// QueueMode.test.tsx); these tests pin the modal's own contract.
function render(onError = (m: string) => {}, onConfirm = () => {}, onDeny = () => {}) {
  return renderWithTheme(
    <ConfirmDenyModal
      row={ROW}
      onClose={() => {}}
      onConfirm={onConfirm}
      onDeny={onDeny}
      onError={onError}
    />,
  );
}

test("shows the fact body and its evidence quote from the brain row", () => {
  render();
  expect(screen.getByText("Some fact body.")).toBeInTheDocument();
  expect(screen.getByText(/…some fact body…/)).toBeInTheDocument();
  expect(screen.getByText(/documents\/note\.md/)).toBeInTheDocument();
});

test("Confirm hands the fact id to onConfirm", async () => {
  const onConfirm = vi.fn().mockResolvedValue(undefined);
  render(undefined, onConfirm);
  fireEvent.click(screen.getByRole("button", { name: /^confirm$/i }));
  await waitFor(() => {
    expect(onConfirm).toHaveBeenCalledWith("fact_1");
  });
});

test("Deny hands the fact id to onDeny", async () => {
  const onDeny = vi.fn().mockResolvedValue(undefined);
  render(undefined, () => {}, onDeny);
  fireEvent.click(screen.getByRole("button", { name: /^deny$/i }));
  await waitFor(() => {
    expect(onDeny).toHaveBeenCalledWith("fact_1");
  });
});

test("a denial error is surfaced via onError, never swallowed (Task 8 review note)", async () => {
  const onDeny = vi
    .fn()
    .mockRejectedValue(new Error("denial ledger write failed"));
  const onError = vi.fn();
  render(onError, () => {}, onDeny);
  fireEvent.click(screen.getByRole("button", { name: /^deny$/i }));
  await waitFor(() => {
    expect(onError).toHaveBeenCalledWith("denial ledger write failed");
  });
  // The modal stays open so the action can be retried.
  expect(screen.getByRole("dialog")).toBeInTheDocument();
});

test("Escape closes the modal", () => {
  const onClose = vi.fn();
  renderWithTheme(
    <ConfirmDenyModal
      row={ROW}
      onClose={onClose}
      onConfirm={() => Promise.resolve()}
      onDeny={() => Promise.resolve()}
      onError={() => {}}
    />,
  );
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onClose).toHaveBeenCalled();
});
