import { screen, fireEvent, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ReviewMode } from "../components/modes/ReviewMode";
import type { ReviewPage } from "../lib/tauri";
import { renderWithTheme } from "./test-utils";
import { ThemeProvider } from "../lib/ThemeContext";

function defaultInvoke(cmd: string) {
  if (cmd === "get_indexing_status") {
    return Promise.resolve({ indexed: 0, pending: 0 });
  }
  if (cmd === "get_proposed_content") {
    return Promise.resolve("# Test Wiki Page\n\nTest content.");
  }
  if (cmd === "read_document") {
    return Promise.resolve("# Project X\n\nExisting wiki content.");
  }
  if (cmd === "approve_wiki_page") return Promise.resolve();
  if (cmd === "reject_wiki_page") return Promise.resolve();
  return Promise.resolve(null);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string) => defaultInvoke(cmd));
});

const PAGE = {
  id: 1,
  path: "wiki/Project-X.md",
  generated_by: "llama3.2:3b",
  source_doc_ids: "[\"documents/notes.md\"]",
} as unknown as ReviewPage;

const OLDER_PAGE = {
  id: 1,
  path: "wiki/Older.md",
  generated_by: "llama3.2:3b",
  source_doc_ids: "[\"documents/alpha.md\"]",
} as unknown as ReviewPage;

const NEWER_PAGE = {
  id: 2,
  path: "wiki/Newer.md",
  generated_by: "gpt-4o",
  source_doc_ids: "[\"documents/beta.md\"]",
} as unknown as ReviewPage;

const VAULT = "/Users/test/Curated-Thoughts";

test("shows the queue-clear empty state when queue is empty", async () => {
  renderWithTheme(<ReviewMode queue={[]} onAction={vi.fn()} vaultPath={VAULT} />);
  expect(screen.getByText(/queue clear/i)).toBeInTheDocument();
  expect(
    await screen.findByText(/librarian watching 0 documents/i),
  ).toBeInTheDocument();
});

test("empty state shows indexed document count from backend", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "get_indexing_status") {
      return Promise.resolve({ indexed: 142, pending: 0 });
    }
    return defaultInvoke(cmd);
  });

  renderWithTheme(<ReviewMode queue={[]} onAction={vi.fn()} vaultPath={VAULT} />);
  expect(
    await screen.findByText(/librarian watching 142 documents/i),
  ).toBeInTheDocument();
});

test("renders queue cards oldest-first with path, model, and source names", async () => {
  renderWithTheme(
    <ReviewMode
      queue={[NEWER_PAGE, OLDER_PAGE]}
      onAction={vi.fn()}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  const items = within(list).getAllByRole("listitem");
  expect(items).toHaveLength(2);
  expect(items[0]).toHaveTextContent("wiki/Older.md");
  expect(items[1]).toHaveTextContent("wiki/Newer.md");
  expect(within(list).getByText("gpt-4o")).toBeInTheDocument();
  expect(within(list).getByText(/alpha\.md/)).toBeInTheDocument();
  expect(within(list).getByText(/beta\.md/)).toBeInTheDocument();
});

test("renders evidence panel with source documents for selected proposal", async () => {
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={vi.fn()} vaultPath={VAULT} />);
  const evidence = await screen.findByRole("complementary", {
    name: /source evidence/i,
  });
  expect(
    within(evidence).getByRole("button", { name: "notes.md" }),
  ).toBeInTheDocument();
  expect(
    within(evidence).getByText(/source chunks not available/i),
  ).toBeInTheDocument();
  expect(within(evidence).getByText("Not recorded")).toBeInTheDocument();
});

test("renders queue items and proposed content", async () => {
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={vi.fn()} vaultPath={VAULT} />);
  expect((await screen.findAllByText("wiki/Project-X.md")).length).toBeGreaterThan(0);
  expect(await screen.findByText(/test wiki page/i)).toBeInTheDocument();
});

test("approve invokes approve_wiki_page and calls onAction", async () => {
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/test wiki page/i);
  fireEvent.click(screen.getByRole("button", { name: /approve/i }));
  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "approve_wiki_page",
    expect.objectContaining({
      id: 1,
      content: "# Test Wiki Page\n\nTest content.",
    }),
  );
});

test("keyboard a approves the selected proposal", async () => {
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/test wiki page/i);
  fireEvent.keyDown(window, { key: "a" });
  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "approve_wiki_page",
    expect.objectContaining({ id: 1 }),
  );
});

test("keyboard r rejects after optional prompt", async () => {
  const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("Too noisy");
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/test wiki page/i);
  fireEvent.keyDown(window, { key: "r" });
  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(promptSpy).toHaveBeenCalled();
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("reject_wiki_page", { id: 1 });
  promptSpy.mockRestore();
});

test("keyboard r does nothing when prompt is cancelled", async () => {
  const promptSpy = vi.spyOn(window, "prompt").mockReturnValue(null);
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/test wiki page/i);
  fireEvent.keyDown(window, { key: "r" });
  await new Promise((r) => setTimeout(r, 50));
  expect(onAction).not.toHaveBeenCalled();
  promptSpy.mockRestore();
});

test("keyboard j and k move queue selection", async () => {
  renderWithTheme(
    <ReviewMode
      queue={[NEWER_PAGE, OLDER_PAGE]}
      onAction={vi.fn()}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  const buttons = within(list).getAllByRole("button", { pressed: true });
  expect(buttons[0]).toHaveTextContent("wiki/Older.md");

  fireEvent.keyDown(window, { key: "j" });
  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /wiki\/Newer\.md/i }),
    ).toHaveAttribute("aria-pressed", "true"),
  );

  fireEvent.keyDown(window, { key: "k" });
  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /wiki\/Older\.md/i }),
    ).toHaveAttribute("aria-pressed", "true"),
  );
});

test("keyboard space advances to the next queue item", async () => {
  renderWithTheme(
    <ReviewMode
      queue={[NEWER_PAGE, OLDER_PAGE]}
      onAction={vi.fn()}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  fireEvent.keyDown(window, { key: " " });
  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /wiki\/Newer\.md/i }),
    ).toHaveAttribute("aria-pressed", "true"),
  );
});

test("keyboard e focuses the proposal editor container", async () => {
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={vi.fn()} vaultPath={VAULT} />);
  const editor = await screen.findByTestId("review-proposal-editor");
  const focusSpy = vi.spyOn(editor, "focus");
  fireEvent.keyDown(window, { key: "e" });
  expect(focusSpy).toHaveBeenCalled();
  focusSpy.mockRestore();
});

test("keyboard shortcuts are ignored inside text inputs", async () => {
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/test wiki page/i);
  const input = document.createElement("input");
  document.body.appendChild(input);
  input.focus();
  fireEvent.keyDown(input, { key: "a" });
  await new Promise((r) => setTimeout(r, 50));
  expect(onAction).not.toHaveBeenCalled();
  input.remove();
});

test("approve advances selection to the next queue item", async () => {
  const onAction = vi.fn();
  const { rerender } = renderWithTheme(
    <ReviewMode
      queue={[NEWER_PAGE, OLDER_PAGE]}
      onAction={onAction}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  fireEvent.click(
    within(list).getByRole("button", { name: /wiki\/Older\.md/i }),
  );
  fireEvent.click(screen.getByRole("button", { name: /approve/i }));
  await waitFor(() => expect(onAction).toHaveBeenCalled());

  rerender(
    <ThemeProvider>
      <ReviewMode queue={[NEWER_PAGE]} onAction={onAction} vaultPath={VAULT} />
    </ThemeProvider>,
  );

  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /wiki\/Newer\.md/i }),
    ).toHaveAttribute("aria-pressed", "true"),
  );
});

test("batch approve approves all checked queue items", async () => {
  const onAction = vi.fn();
  renderWithTheme(
    <ReviewMode
      queue={[NEWER_PAGE, OLDER_PAGE]}
      onAction={onAction}
      vaultPath={VAULT}
    />,
  );

  fireEvent.click(screen.getByRole("checkbox", { name: /select wiki\/Older/i }));
  fireEvent.click(screen.getByRole("checkbox", { name: /select wiki\/Newer/i }));
  fireEvent.click(screen.getByRole("button", { name: /approve 2 selected/i }));

  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "approve_wiki_page",
    expect.objectContaining({ id: 1 }),
  );
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "approve_wiki_page",
    expect.objectContaining({ id: 2 }),
  );
});
