import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EntityList } from "../components/brain/EntityList";
import type { EntitySummary } from "../lib/tauri";
import { invoke } from "@tauri-apps/api/core";
import { describe, it, expect, vi, beforeEach } from "vitest";

const ENTITIES: EntitySummary[] = [
  {
    id: "ent_1",
    name: "Alpha",
    entity_type: "project",
    summary_snippet: "",
    fact_count: 3,
    open_task_count: 0,
    created_at: 100,
    updated_at: 100,
  },
  {
    id: "ent_2",
    name: "Bob",
    entity_type: "person",
    summary_snippet: "",
    fact_count: 0,
    open_task_count: 0,
    created_at: 100,
    updated_at: 100,
  },
  {
    id: "ent_3",
    name: "Beta",
    entity_type: "project",
    summary_snippet: "",
    fact_count: 0,
    open_task_count: 0,
    created_at: 100,
    updated_at: 100,
  },
];

vi.mock("../components/brain/EntityPage", () => ({
  EntityPage: () => <main data-testid="entity-page" />,
}));

vi.mock("../components/brain/ConnectionsPanel", () => ({
  ConnectionsPanel: () => <aside data-testid="connections-panel" />,
}));

vi.mock("../components/shell/OkfInteropBar", () => ({
  OkfInteropBar: () => <div data-testid="okf-interop-bar" />,
}));

vi.mock("../components/shell/EditorPane", () => ({
  EditorPane: () => <div data-testid="editor-pane" />,
}));

describe("EntityList", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_entities_cmd") return Promise.resolve(ENTITIES);
      if (cmd === "get_entity_cmd") return Promise.resolve(null);
      if (cmd === "list_vault_files") return Promise.resolve([]);
      if (cmd === "search_vault") return Promise.resolve([]);
      if (cmd === "get_related_chunks") return Promise.resolve([]);
      if (cmd === "get_structural_neighbors") return Promise.resolve([]);
      if (cmd === "get_indexing_status") return Promise.resolve({ indexed: 0, pending: 0 });
      return Promise.resolve(null);
    });
  });

  it("groups entities by type and selects on click", () => {
    const onSelect = vi.fn();
    render(
      <EntityList entities={ENTITIES} selectedId={null} onSelect={onSelect} onCreate={vi.fn()} sort="updated_desc" onSortChange={vi.fn()} />,
    );
    expect(screen.getByRole("heading", { name: "person" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "project" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Alpha/ }));
    expect(onSelect).toHaveBeenCalledWith("ent_1");
  });

  it("filter narrows the list by name", () => {
    render(
      <EntityList entities={ENTITIES} selectedId={null} onSelect={vi.fn()} onCreate={vi.fn()} sort="updated_desc" onSortChange={vi.fn()} />,
    );
    fireEvent.change(screen.getByPlaceholderText("Filter entities..."), {
      target: { value: "be" },
    });
    expect(screen.getByRole("button", { name: /Beta/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Alpha/ })).not.toBeInTheDocument();
  });

  it("new entity form submits trimmed name", () => {
    const onCreate = vi.fn();
    render(<EntityList entities={[]} selectedId={null} onSelect={vi.fn()} onCreate={onCreate} sort="updated_desc" onSortChange={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "+ New entity" }));
    fireEvent.change(screen.getByLabelText("New entity name"), {
      target: { value: "  Project X  " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(onCreate).toHaveBeenCalledWith("Project X");
  });

  describe("sort picker", () => {
    it("renders sort picker with all four options", () => {
      render(
        <EntityList entities={ENTITIES} selectedId={null} onSelect={vi.fn()} onCreate={vi.fn()} sort="updated_desc" onSortChange={vi.fn()} />,
      );
      const select = screen.getByRole("combobox", { name: "Sort entities" });
      expect(select).toBeInTheDocument();
      expect(select).toHaveValue("updated_desc");
      expect(screen.getByRole("option", { name: "Recently updated" })).toBeInTheDocument();
      expect(screen.getByRole("option", { name: "Name (A → Z)" })).toBeInTheDocument();
      expect(screen.getByRole("option", { name: "Name (Z → A)" })).toBeInTheDocument();
      expect(screen.getByRole("option", { name: "Recently created" })).toBeInTheDocument();
    });

    it("uses sort as a controlled prop — parent updates flow back to the picker", () => {
      // Discriminator for the deferred #2 refactor: today the picker is driven
      // by local state and ignores any `sort` prop, so a rerender with a new
      // value leaves the picker on "updated_desc". After lifting sort to a prop,
      // the rerender flips the picker to "name_desc".
      const onSortChange = vi.fn();
      const { rerender } = render(
        <EntityList
          entities={ENTITIES}
          selectedId={null}
          onSelect={vi.fn()}
          onCreate={vi.fn()}
          sort="updated_desc"
          onSortChange={onSortChange}
        />,
      );
      const select = screen.getByRole("combobox", { name: "Sort entities" }) as HTMLSelectElement;
      expect(select).toHaveValue("updated_desc");

      rerender(
        <EntityList
          entities={ENTITIES}
          selectedId={null}
          onSelect={vi.fn()}
          onCreate={vi.fn()}
          sort="name_desc"
          onSortChange={onSortChange}
        />,
      );
      expect(select).toHaveValue("name_desc");
      // onSortChange is fired only by user interaction, not by the rerender itself.
      expect(onSortChange).not.toHaveBeenCalled();
    });

    it("calls listEntities via invoke with new sort value when selection changes", async () => {
      const user = userEvent.setup();
      const { BrainMode } = await import("../components/modes/BrainMode");
      render(
        <BrainMode
          selectedEntityId={null}
          onEntitySelect={vi.fn()}
          onOpenSource={vi.fn()}
          onEntityName={vi.fn()}
        />,
      );
      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith(
          "list_entities_cmd",
          expect.objectContaining({ sort: "updated_desc" }),
        ),
      );

      const select = screen.getByRole("combobox", { name: "Sort entities" });
      await user.selectOptions(select, "name_asc");
      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith(
          "list_entities_cmd",
          expect.objectContaining({ sort: "name_asc" }),
        ),
      );

      await user.selectOptions(select, "created_desc");
      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith(
          "list_entities_cmd",
          expect.objectContaining({ sort: "created_desc" }),
        ),
      );
    });
  });
});
