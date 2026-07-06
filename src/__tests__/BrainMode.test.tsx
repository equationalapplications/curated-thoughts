import { render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

vi.mock("../components/shell/EditorPane", () => ({
  EditorPane: ({ isWiki }: { isWiki: boolean }) => (
    <div data-testid="editor-pane" data-is-wiki={String(isWiki)} />
  ),
}));

import { BrainMode } from "../components/modes/BrainMode";
import { LibraryMode } from "../components/modes/LibraryMode";

const FILES = [
  { path: "wiki/Alpha.md", name: "Alpha.md", tier: "wiki" },
  { path: "documents/beta.pdf", name: "beta.pdf", tier: "user_doc" },
];

beforeEach(() => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "list_vault_files") return Promise.resolve(FILES);
    if (cmd === "search_vault") return Promise.resolve([]);
    if (cmd === "get_related_chunks") return Promise.resolve([]);
    if (cmd === "get_structural_neighbors") return Promise.resolve([]);
    if (cmd === "get_indexing_status") return Promise.resolve({ indexed: 0, pending: 0 });
    return Promise.resolve(null);
  });
});

test("BrainMode lists only wiki files and renders an editable editor", async () => {
  render(
    <BrainMode
      vaultPath="/Users/test/Curated-Thoughts"
      selectedDoc="wiki/Alpha.md"
      onDocSelect={vi.fn()}
    />,
  );
  expect(await screen.findByText("Alpha.md")).toBeInTheDocument();
  expect(screen.queryByText("beta.pdf")).not.toBeInTheDocument();
  expect(screen.getByTestId("editor-pane")).toHaveAttribute("data-is-wiki", "true");
});

test("LibraryMode lists only user documents and renders a read-only editor", async () => {
  render(
    <LibraryMode
      vaultPath="/Users/test/Curated-Thoughts"
      selectedDoc="documents/beta.pdf"
      onDocSelect={vi.fn()}
    />,
  );
  expect(await screen.findByText("beta.pdf")).toBeInTheDocument();
  await waitFor(() =>
    expect(screen.queryByText("Alpha.md")).not.toBeInTheDocument(),
  );
  expect(screen.getByTestId("editor-pane")).toHaveAttribute("data-is-wiki", "false");
});
