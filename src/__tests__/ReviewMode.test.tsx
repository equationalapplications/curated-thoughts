import { screen, fireEvent, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ReviewMode } from "../components/modes/ReviewMode";
import { renderWithTheme } from "./test-utils";
import { ThemeProvider } from "../lib/ThemeContext";
import {
  makeProposalDetail,
  makeProposalSummary,
} from "./fixtures/proposals";

const VAULT = "/Users/test/Curated-Thoughts";

const PAGE = makeProposalSummary({
  id: "prop_project_x",
  target_name: "Project X",
  created_at: 200,
  source_doc_paths: ["documents/notes.md"],
});

const OLDER = makeProposalSummary({
  id: "prop_older",
  target_name: "Older Entity",
  created_at: 100,
  source_doc_paths: ["documents/alpha.md"],
});

const NEWER = makeProposalSummary({
  id: "prop_newer",
  target_name: "Newer Entity",
  created_at: 300,
  model: "gpt-4o",
  source_doc_paths: ["documents/beta.md"],
});

function detailFor(summary: typeof PAGE) {
  return makeProposalDetail(summary, {
    reasoning: null,
    items: [
      {
        id: `item_${summary.id}`,
        item_type: "fact_add",
        target_id: null,
        payload: { body: "Test fact for preview." },
        evidence: [],
        status: "pending",
        edited_payload: null,
      },
    ],
  });
}

function defaultInvoke(cmd: string, args?: Record<string, unknown>) {
  if (cmd === "get_indexing_status") {
    return Promise.resolve({ indexed: 0, pending: 0 });
  }
  if (cmd === "get_proposal_detail_cmd") {
    const proposalId = args?.proposalId as string;
    const summary = [PAGE, OLDER, NEWER].find((p) => p.id === proposalId) ?? PAGE;
    return Promise.resolve(detailFor(summary));
  }
  if (cmd === "resolve_proposal_cmd") return Promise.resolve({
    committed: [],
    conflicts: [],
    dropped_edges: [],
    proposal_status: "approved",
  });
  return Promise.resolve(null);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string, args?: Record<string, unknown>) =>
    defaultInvoke(cmd, args),
  );
});

test("shows the queue-clear empty state when queue is empty", async () => {
  renderWithTheme(<ReviewMode queue={[]} onAction={vi.fn()} vaultPath={VAULT} />);
  expect(screen.getByText(/queue clear/i)).toBeInTheDocument();
  expect(
    await screen.findByText(/librarian watching 0 documents/i),
  ).toBeInTheDocument();
});

test("empty state shows indexed document count from backend", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "get_indexing_status") {
      return Promise.resolve({ indexed: 142, pending: 0 });
    }
    return defaultInvoke(cmd, args);
  });

  renderWithTheme(<ReviewMode queue={[]} onAction={vi.fn()} vaultPath={VAULT} />);
  expect(
    await screen.findByText(/librarian watching 142 documents/i),
  ).toBeInTheDocument();
});

test("renders queue cards oldest-first with target name, model, and source names", async () => {
  renderWithTheme(
    <ReviewMode
      queue={[NEWER, OLDER]}
      onAction={vi.fn()}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  const items = within(list).getAllByRole("listitem");
  expect(items).toHaveLength(2);
  expect(items[0]).toHaveTextContent("Older Entity");
  expect(items[1]).toHaveTextContent("Newer Entity");
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

test("renders queue items and proposal preview", async () => {
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={vi.fn()} vaultPath={VAULT} />);
  expect((await screen.findAllByText("Project X")).length).toBeGreaterThan(0);
  expect(await screen.findByText(/Test fact for preview/i)).toBeInTheDocument();
});

test("approve invokes resolve_proposal_cmd and calls onAction", async () => {
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/Test fact for preview/i);
  fireEvent.click(screen.getByRole("button", { name: /approve/i }));
  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "resolve_proposal_cmd",
    expect.objectContaining({
      proposalId: "prop_project_x",
      decisions: [{ item_id: `item_${PAGE.id}`, decision: "accept" }],
    }),
  );
});

test("keyboard a approves the selected proposal", async () => {
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/Test fact for preview/i);
  fireEvent.keyDown(window, { key: "a" });
  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "resolve_proposal_cmd",
    expect.objectContaining({ proposalId: "prop_project_x" }),
  );
});

test("keyboard r rejects after optional prompt", async () => {
  const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("Too noisy");
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/Test fact for preview/i);
  fireEvent.keyDown(window, { key: "r" });
  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(promptSpy).toHaveBeenCalled();
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "resolve_proposal_cmd",
    expect.objectContaining({
      proposalId: "prop_project_x",
      rejectReason: "Too noisy",
      decisions: [{ item_id: `item_${PAGE.id}`, decision: "reject" }],
    }),
  );
  promptSpy.mockRestore();
});

test("keyboard r does nothing when prompt is cancelled", async () => {
  const promptSpy = vi.spyOn(window, "prompt").mockReturnValue(null);
  const onAction = vi.fn();
  renderWithTheme(<ReviewMode queue={[PAGE]} onAction={onAction} vaultPath={VAULT} />);
  await screen.findByText(/Test fact for preview/i);
  fireEvent.keyDown(window, { key: "r" });
  await new Promise((r) => setTimeout(r, 50));
  expect(onAction).not.toHaveBeenCalled();
  promptSpy.mockRestore();
});

test("keyboard j and k move queue selection", async () => {
  renderWithTheme(
    <ReviewMode
      queue={[NEWER, OLDER]}
      onAction={vi.fn()}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  const buttons = within(list).getAllByRole("button", { pressed: true });
  expect(buttons[0]).toHaveTextContent("Older Entity");

  fireEvent.keyDown(window, { key: "j" });
  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /Newer Entity/i }),
    ).toHaveAttribute("aria-pressed", "true"),
  );

  fireEvent.keyDown(window, { key: "k" });
  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /Older Entity/i }),
    ).toHaveAttribute("aria-pressed", "true"),
  );
});

test("keyboard space advances to the next queue item", async () => {
  renderWithTheme(
    <ReviewMode
      queue={[NEWER, OLDER]}
      onAction={vi.fn()}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  fireEvent.keyDown(window, { key: " " });
  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /Newer Entity/i }),
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
  await screen.findByText(/Test fact for preview/i);
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
      queue={[NEWER, OLDER]}
      onAction={onAction}
      vaultPath={VAULT}
    />,
  );

  const list = screen.getByRole("list", { name: /review queue/i });
  fireEvent.click(
    within(list).getByRole("button", { name: /Older Entity/i }),
  );
  await screen.findByText(/Test fact for preview/i);
  fireEvent.click(screen.getByRole("button", { name: /approve/i }));
  await waitFor(() => expect(onAction).toHaveBeenCalled());

  rerender(
    <ThemeProvider>
      <ReviewMode queue={[NEWER]} onAction={onAction} vaultPath={VAULT} />
    </ThemeProvider>,
  );

  await waitFor(() =>
    expect(
      within(list).getByRole("button", { name: /Newer Entity/i }),
    ).toHaveAttribute("aria-pressed", "true"),
  );
});

test("batch approve approves all checked queue items", async () => {
  const onAction = vi.fn();
  renderWithTheme(
    <ReviewMode
      queue={[NEWER, OLDER]}
      onAction={onAction}
      vaultPath={VAULT}
    />,
  );

  fireEvent.click(screen.getByRole("checkbox", { name: /select Older Entity/i }));
  fireEvent.click(screen.getByRole("checkbox", { name: /select Newer Entity/i }));
  fireEvent.click(screen.getByRole("button", { name: /approve 2 selected/i }));

  await waitFor(() => expect(onAction).toHaveBeenCalled());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "resolve_proposal_cmd",
    expect.objectContaining({ proposalId: "prop_older" }),
  );
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "resolve_proposal_cmd",
    expect.objectContaining({ proposalId: "prop_newer" }),
  );
});
