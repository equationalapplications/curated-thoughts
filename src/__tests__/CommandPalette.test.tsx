import { screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { registerCommandContext } from "../lib/commands";
import { CommandPalette } from "../components/shell/CommandPalette";
import { renderWithTheme } from "./test-utils";

const ENTITIES = [
  {
    id: "ent_1", name: "Apollo Program", entity_type: "project", summary_snippet: "",
    fact_count: 0, open_task_count: 0, created_at: 1, updated_at: 1,
  },
];
const FILES = [
  { path: "documents/apollo-notes.md", name: "apollo-notes.md", tier: "user_doc" },
  { path: "wiki/Apollo.md", name: "Apollo.md", tier: "wiki" },
];

let navigate: ReturnType<typeof vi.fn>;
let unregister: () => void;

beforeEach(() => {
  navigate = vi.fn();
  unregister = registerCommandContext({ navigate });
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "list_entities_cmd") return Promise.resolve(ENTITIES);
    if (cmd === "list_vault_files") return Promise.resolve(FILES);
    return Promise.resolve(null);
  });
});

afterEach(() => {
  unregister();
});

function renderPalette(scope = "mode:brain" as const) {
  const onClose = vi.fn();
  renderWithTheme(<CommandPalette scope={scope} onClose={onClose} />);
  return { onClose };
}

test("shows registry commands and hides palette-internal ones", async () => {
  renderPalette();
  expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  for (const label of [
    "Go to Brain", "Go to Review", "Go to Library",
    "Go to Timeline", "Go to Tasks", "Go to Settings",
  ]) {
    expect(screen.getByText(label)).toBeInTheDocument();
  }
  expect(screen.queryByText("Close the palette")).not.toBeInTheDocument();
  expect(screen.queryByText("Select the next result")).not.toBeInTheDocument();
  expect(screen.queryByText("Select the previous result")).not.toBeInTheDocument();
});

test("query filters registry commands and surfaces entity and document matches", async () => {
  renderPalette();
  const input = await screen.findByLabelText("Search commands");
  fireEvent.change(input, { target: { value: "apollo" } });
  expect(await screen.findByText("Open entity: Apollo Program")).toBeInTheDocument();
  expect(screen.getByText("Open document: apollo-notes.md")).toBeInTheDocument();
  // Wiki-tier files are not document-open targets; registry commands that
  // don't match the query disappear.
  expect(screen.queryByText(/Apollo\.md/)).not.toBeInTheDocument();
  expect(screen.queryByText("Go to Brain")).not.toBeInTheDocument();
});

test("Enter dispatches the active command and closes", async () => {
  const { onClose } = renderPalette();
  const input = await screen.findByLabelText("Search commands");
  // Empty query: first visible entry is "Go to Brain".
  fireEvent.keyDown(input, { key: "Enter" });
  expect(navigate).toHaveBeenCalledWith({ mode: "brain" });
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("ArrowDown then Enter dispatches the second command", async () => {
  const { onClose } = renderPalette();
  const input = await screen.findByLabelText("Search commands");
  fireEvent.keyDown(input, { key: "ArrowDown" });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(navigate).toHaveBeenCalledWith({ mode: "review" });
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("Enter on an entity result navigates to Brain with the entity id", async () => {
  renderPalette();
  const input = await screen.findByLabelText("Search commands");
  fireEvent.change(input, { target: { value: "apollo" } });
  await screen.findByText("Open entity: Apollo Program");
  fireEvent.keyDown(input, { key: "Enter" });
  expect(navigate).toHaveBeenCalledWith({ mode: "brain", entityId: "ent_1" });
});

test("Enter on a document result navigates to Library with the doc path", async () => {
  renderPalette();
  const input = await screen.findByLabelText("Search commands");
  fireEvent.change(input, { target: { value: "apollo" } });
  await screen.findByText("Open document: apollo-notes.md");
  fireEvent.keyDown(input, { key: "ArrowDown" }); // entity first, document second
  fireEvent.keyDown(input, { key: "Enter" });
  expect(navigate).toHaveBeenCalledWith({ mode: "library", docPath: "documents/apollo-notes.md" });
});

test("Escape closes the palette", async () => {
  const { onClose } = renderPalette();
  await screen.findByRole("dialog", { name: "Command palette" });
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("clicking the backdrop closes the palette", async () => {
  const { onClose } = renderPalette();
  await screen.findByRole("dialog", { name: "Command palette" });
  fireEvent.click(screen.getByRole("button", { name: "Close command palette" }));
  expect(onClose).toHaveBeenCalledTimes(1);
});
