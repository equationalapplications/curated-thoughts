import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ReviewMode } from "../components/modes/ReviewMode";
import type { ReviewPage } from "../lib/tauri";

const PAGE = {
  id: 1,
  path: "wiki/Project-X.md",
  generated_by: "llama3.2:3b",
  source_doc_ids: "[\"documents/notes.md\"]",
} as unknown as ReviewPage;

test("shows the queue-clear empty state when queue is empty", () => {
  render(<ReviewMode queue={[]} onAction={vi.fn()} />);
  expect(screen.getByText(/queue clear/i)).toBeInTheDocument();
});

test("renders queue items and proposed content", async () => {
  render(<ReviewMode queue={[PAGE]} onAction={vi.fn()} />);
  expect((await screen.findAllByText("wiki/Project-X.md")).length).toBeGreaterThan(0);
  expect(await screen.findByText(/test wiki page/i)).toBeInTheDocument();
});

test("approve invokes approve_wiki_page and calls onAction", async () => {
  const onAction = vi.fn();
  render(<ReviewMode queue={[PAGE]} onAction={onAction} />);
  await screen.findByText(/test wiki page/i);
  fireEvent.click(screen.getByRole("button", { name: /approve/i }));
  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "approve_wiki_page",
    expect.objectContaining({ id: 1 }),
  );
});
