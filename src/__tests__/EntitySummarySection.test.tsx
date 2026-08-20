import { screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { describe, it, expect, vi, beforeEach } from "vitest";

const editorMock = {
  document: [],
  tryParseMarkdownToBlocks: vi.fn(async () => []),
  replaceBlocks: vi.fn(),
  blocksToMarkdownLossy: vi.fn(async () => "Edited summary."),
  insertInlineContent: vi.fn(),
};

vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => editorMock,
  SuggestionMenuController: () => null,
}));

vi.mock("@blocknote/mantine", () => ({
  BlockNoteView: () => <div data-testid="blocknote" />,
}));

import { EntitySummarySection, searchEntitiesByQuery } from "../components/brain/EntitySummarySection";
import {
  EntityWikilinkSuggestion,
  filterEntitySuggestions,
} from "../components/brain/EntityWikilinkSuggestion";
import {
  __resetWikilinkResolverForTests,
  refreshWikilinkResolver,
} from "../components/brain/WikilinkText";
import { renderWithTheme } from "./test-utils";
import type { EntitySummary } from "../lib/tauri";

function entity(overrides: Partial<EntitySummary>): EntitySummary {
  return {
    id: "ent_x",
    name: "X",
    entity_type: "concept",
    summary_snippet: "",
    fact_count: 0,
    open_task_count: 0,
    created_at: 100,
    updated_at: 100,
    ...overrides,
  };
}

const ENTITIES = [
  entity({ id: "ent_alpha", name: "Alpha", entity_type: "project" }),
  entity({ id: "ent_beta", name: "Beta", entity_type: "project" }),
  entity({ id: "ent_charlie", name: "Charlie", entity_type: "person" }),
];

beforeEach(() => {
  editorMock.tryParseMarkdownToBlocks.mockClear();
  editorMock.replaceBlocks.mockClear();
  editorMock.blocksToMarkdownLossy.mockClear();
  editorMock.insertInlineContent.mockClear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "list_entities_cmd") return Promise.resolve(ENTITIES);
    if (cmd === "update_entity_summary_cmd") return Promise.resolve();
    return Promise.resolve(null);
  });
});

describe("searchEntitiesByQuery", () => {
  beforeEach(async () => {
    __resetWikilinkResolverForTests();
    await refreshWikilinkResolver();
  });

  it("filters the cached entity list by case-insensitive prefix", () => {
    const items = searchEntitiesByQuery("be");
    expect(items.map((i) => i.entity.name)).toEqual(["Beta"]);
  });

  it("returns all entities for an empty query", () => {
    const items = searchEntitiesByQuery("");
    expect(items).toHaveLength(3);
  });

  it("returns an empty list for a non-matching query", () => {
    const items = searchEntitiesByQuery("zz");
    expect(items).toEqual([]);
  });
});

test("renders summary prose with wikilink chips in view mode", () => {
  const onNavigate = vi.fn();
  renderWithTheme(
    <EntitySummarySection
      entityId="ent_1"
      summary="Owns [[Project X]]."
      onChanged={vi.fn()}
      onNavigateEntity={onNavigate}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Project X" }));
  expect(onNavigate).toHaveBeenCalledWith("Project X");
  expect(screen.queryByTestId("blocknote")).not.toBeInTheDocument();
});

test("edit loads markdown into BlockNote; save round-trips and persists", async () => {
  const onChanged = vi.fn();
  renderWithTheme(
    <EntitySummarySection
      entityId="ent_1"
      summary="Original."
      onChanged={onChanged}
      onNavigateEntity={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Edit summary" }));
  await screen.findByTestId("blocknote");
  expect(editorMock.tryParseMarkdownToBlocks).toHaveBeenCalledWith("Original.");

  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(onChanged).toHaveBeenCalled());
  expect(invoke).toHaveBeenCalledWith("update_entity_summary_cmd", {
    entityId: "ent_1",
    summary: "Edited summary.",
  });
});

describe("EntityWikilinkSuggestion", () => {
  it("filters entities by case-insensitive prefix match on name", () => {
    const matches = filterEntitySuggestions(ENTITIES, "al");
    expect(matches.map((e) => e.name)).toEqual(["Alpha"]);
  });

  it("returns all entities when query is empty", () => {
    const matches = filterEntitySuggestions(ENTITIES, "");
    expect(matches).toHaveLength(3);
  });

  it("returns empty list when no match", () => {
    const matches = filterEntitySuggestions(ENTITIES, "zz");
    expect(matches).toEqual([]);
  });

  it("renders matching entities and calls onSelect on click", () => {
    const onSelect = vi.fn();
    renderWithTheme(
      <EntityWikilinkSuggestion entities={ENTITIES} query="be" onSelect={onSelect} />,
    );
    expect(screen.getByRole("option", { name: /Beta/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Alpha/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Charlie/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /Beta/ }));
    expect(onSelect).toHaveBeenCalledWith(ENTITIES[1]);
  });

  it("renders all entities when query is empty", () => {
    renderWithTheme(
      <EntityWikilinkSuggestion entities={ENTITIES} query="" onSelect={vi.fn()} />,
    );
    expect(screen.getByRole("option", { name: /Alpha/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Beta/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Charlie/ })).toBeInTheDocument();
  });

  it("shows empty placeholder when no matches", () => {
    renderWithTheme(
      <EntityWikilinkSuggestion entities={ENTITIES} query="zz" onSelect={vi.fn()} />,
    );
    expect(screen.getByText(/No entities match\./)).toBeInTheDocument();
  });
});
