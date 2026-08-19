import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EntityList } from "../components/brain/EntityList";
import type { EntitySummary } from "../lib/tauri";
import { describe, it, expect, vi } from "vitest";

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
  entity({ id: "ent_1", name: "Alpha", entity_type: "project", fact_count: 3 }),
  entity({ id: "ent_2", name: "Bob", entity_type: "person" }),
  entity({ id: "ent_3", name: "Beta", entity_type: "project" }),
];

describe("EntityList", () => {
  it("groups entities by type and selects on click", () => {
    const onSelect = vi.fn();
    render(
      <EntityList entities={ENTITIES} selectedId={null} onSelect={onSelect} onCreate={vi.fn()} />,
    );
    expect(screen.getByRole("heading", { name: "person" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "project" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Alpha/ }));
    expect(onSelect).toHaveBeenCalledWith("ent_1");
  });

  it("filter narrows the list by name", () => {
    render(
      <EntityList entities={ENTITIES} selectedId={null} onSelect={vi.fn()} onCreate={vi.fn()} />,
    );
    fireEvent.change(screen.getByPlaceholderText("Filter entities..."), {
      target: { value: "be" },
    });
    expect(screen.getByRole("button", { name: /Beta/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Alpha/ })).not.toBeInTheDocument();
  });

  it("new entity form submits trimmed name", () => {
    const onCreate = vi.fn();
    render(<EntityList entities={[]} selectedId={null} onSelect={vi.fn()} onCreate={onCreate} />);
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
        <EntityList entities={ENTITIES} selectedId={null} onSelect={vi.fn()} onCreate={vi.fn()} />,
      );
      const select = screen.getByRole("combobox", { name: "Sort entities" });
      expect(select).toBeInTheDocument();
      expect(select).toHaveValue("updated_desc");
      expect(screen.getByRole("option", { name: "Recently updated" })).toBeInTheDocument();
      expect(screen.getByRole("option", { name: "Name (A → Z)" })).toBeInTheDocument();
      expect(screen.getByRole("option", { name: "Name (Z → A)" })).toBeInTheDocument();
      expect(screen.getByRole("option", { name: "Recently created" })).toBeInTheDocument();
    });

    it("allows changing the sort selection", async () => {
      const user = userEvent.setup();
      render(
        <EntityList entities={ENTITIES} selectedId={null} onSelect={vi.fn()} onCreate={vi.fn()} />,
      );
      const select = screen.getByRole("combobox", { name: "Sort entities" });
      await user.selectOptions(select, "name_asc");
      expect(select).toHaveValue("name_asc");
      await user.selectOptions(select, "created_desc");
      expect(select).toHaveValue("created_desc");
    });
  });
});
