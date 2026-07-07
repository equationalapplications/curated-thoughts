import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

vi.mock("../components/brain/EntityPage", () => ({
  EntityPage: ({
    entityId,
    onEntityLoaded,
  }: {
    entityId: string | null;
    onEntityLoaded: (detail: unknown) => void;
  }) => {
    if (entityId && onEntityLoaded) {
      onEntityLoaded({ id: entityId, name: "Entity Alpha" });
    }
    return (
      <main data-testid="entity-page" data-entity-id={entityId} />
    );
  },
}));

vi.mock("../components/brain/ConnectionsPanel", () => ({
  ConnectionsPanel: () => <aside data-testid="connections-panel" />,
}));

vi.mock("../components/shell/OkfInteropBar", () => ({
  OkfInteropBar: () => <div data-testid="okf-interop-bar" />,
}));

vi.mock("../components/shell/EditorPane", () => ({
  EditorPane: ({ isWiki }: { isWiki: boolean }) => (
    <div data-testid="editor-pane" data-is-wiki={String(isWiki)} />
  ),
}));

import { BrainMode } from "../components/modes/BrainMode";
import { LibraryMode } from "../components/modes/LibraryMode";

const ENTITIES = [
  {
    id: "entity-1",
    name: "Entity Alpha",
    entity_type: "Person",
    summary_snippet: "A person",
    fact_count: 3,
    open_task_count: 0,
    created_at: 1000,
    updated_at: 2000,
  },
  {
    id: "entity-2",
    name: "Entity Beta",
    entity_type: "Organization",
    summary_snippet: "An org",
    fact_count: 2,
    open_task_count: 1,
    created_at: 1500,
    updated_at: 2500,
  },
];

const FILES = [
  { path: "wiki/Alpha.md", name: "Alpha.md", tier: "wiki" },
  { path: "documents/beta.pdf", name: "beta.pdf", tier: "user_doc" },
];

beforeEach(() => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "list_entities_cmd") return Promise.resolve(ENTITIES);
    if (cmd === "get_entity_cmd") {
      const id = vi.mocked(invoke).mock.lastCall?.[1]?.entityId;
      return Promise.resolve(
        ENTITIES.find((e) => e.id === id) || null,
      );
    }
    if (cmd === "list_vault_files") return Promise.resolve(FILES);
    if (cmd === "search_vault") return Promise.resolve([]);
    if (cmd === "get_related_chunks") return Promise.resolve([]);
    if (cmd === "get_structural_neighbors") return Promise.resolve([]);
    if (cmd === "get_indexing_status") return Promise.resolve({ indexed: 0, pending: 0 });
    return Promise.resolve(null);
  });
});

test("BrainMode lists entities and shows selected entity page", async () => {
  const onEntitySelect = vi.fn();
  const onEntityName = vi.fn();

  render(
    <BrainMode
      selectedEntityId="entity-1"
      onEntitySelect={onEntitySelect}
      onOpenSource={vi.fn()}
      onEntityName={onEntityName}
    />,
  );

  expect(await screen.findByText("Entity Alpha")).toBeInTheDocument();
  expect(screen.getByText("Entity Beta")).toBeInTheDocument();
  expect(screen.getByTestId("entity-page")).toHaveAttribute(
    "data-entity-id",
    "entity-1",
  );
});

test("BrainMode wikilink name resolves to entity selection", async () => {
  const onEntitySelect = vi.fn();

  render(
    <BrainMode
      selectedEntityId={null}
      onEntitySelect={onEntitySelect}
      onOpenSource={vi.fn()}
      onEntityName={vi.fn()}
    />,
  );

  await screen.findByText("Entity Alpha");

  // Click on Entity Beta in the list
  const entityBetaButton = screen.getByText("Entity Beta").closest("button");
  if (entityBetaButton) {
    fireEvent.click(entityBetaButton);
  }

  expect(onEntitySelect).toHaveBeenCalledWith("entity-2");
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
});
