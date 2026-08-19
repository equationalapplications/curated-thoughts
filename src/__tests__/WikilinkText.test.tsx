import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, test, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  parseWikilinks,
  WikilinkText,
  refreshWikilinkResolver,
  getWikilinkResolverEntities,
} from "../components/brain/WikilinkText";

describe("parseWikilinks", () => {
  test("splits text and link segments", () => {
    expect(parseWikilinks("Works with [[Alpha]] and [[Beta Team]].")).toEqual([
      { type: "text", value: "Works with " },
      { type: "link", value: "Alpha" },
      { type: "text", value: " and " },
      { type: "link", value: "Beta Team" },
      { type: "text", value: "." },
    ]);
  });

  test("handles text with no links", () => {
    expect(parseWikilinks("No links here")).toEqual([
      { type: "text", value: "No links here" },
    ]);
  });

  test("handles unclosed wikilinks as text", () => {
    expect(parseWikilinks("Unclosed [[Alpha")).toEqual([
      { type: "text", value: "Unclosed [[Alpha" },
    ]);
  });
});

describe("WikilinkText component", () => {
  test("clicking a chip fires onNavigate with the entity name", () => {
    const onNavigate = vi.fn();
    render(<WikilinkText text="See [[Project X]] for details." onNavigate={onNavigate} />);
    fireEvent.click(screen.getByRole("button", { name: "Project X" }));
    expect(onNavigate).toHaveBeenCalledWith("Project X");
  });

  test("renders multiple wikilinks as separate chips", () => {
    const onNavigate = vi.fn();
    render(<WikilinkText text="[[First]] and [[Second]]" onNavigate={onNavigate} />);
    const buttons = screen.getAllByRole("button");
    expect(buttons).toHaveLength(2);
    expect(buttons[0]).toHaveTextContent("First");
    expect(buttons[1]).toHaveTextContent("Second");
  });

  test("renders plain text without any buttons", () => {
    const onNavigate = vi.fn();
    render(<WikilinkText text="Just plain text" onNavigate={onNavigate} />);
    const buttons = screen.queryAllByRole("button");
    expect(buttons).toHaveLength(0);
  });
});

describe("WikilinkText resolver cache", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  test("renders unresolved chips for entities created after the initial fetch until refreshWikilinkResolver is called", async () => {
    // Initial fetch: only "Old" exists.
    let currentEntities = [
      { id: "ent_old", name: "Old", entity_type: "concept", summary_snippet: "", fact_count: 0, open_task_count: 0, created_at: 0, updated_at: 0 },
    ];
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_entities_cmd") return Promise.resolve(currentEntities);
      return Promise.resolve(null);
    });
    // Reset the module-level cache (shared across tests) and prime it with
    // the initial fetch. Without this, previous tests' resolver state would
    // shadow our mock.
    await refreshWikilinkResolver();

    const onNavigate = vi.fn();
    const { rerender } = render(
      <WikilinkText text="[[Old]] and [[NewEntity]]" onNavigate={onNavigate} />,
    );

    // Wait for the initial fetch to settle.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Old" })).toHaveClass(
        "wikilink-chip--resolved",
      );
    });
    expect(screen.getByRole("button", { name: "NewEntity" })).toHaveClass(
      "wikilink-chip--unresolved",
    );

    // Mutate the entity list (simulate user creating a new entity).
    currentEntities = [
      ...currentEntities,
      { id: "ent_new", name: "NewEntity", entity_type: "concept", summary_snippet: "", fact_count: 0, open_task_count: 0, created_at: 0, updated_at: 0 },
    ];

    // Refresh the cache (this is what useEntityList.refresh() does now).
    await refreshWikilinkResolver();

    // Re-render and verify the chip flipped to resolved.
    rerender(
      <WikilinkText text="[[Old]] and [[NewEntity]]" onNavigate={onNavigate} />,
    );
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "NewEntity" })).toHaveClass(
        "wikilink-chip--resolved",
      );
    });
  });

  test("getWikilinkResolverEntities returns the cached list (used by the [[Entity]] autocomplete to avoid per-keystroke IPC)", async () => {
    const entities = [
      { id: "ent_a", name: "Alpha", entity_type: "concept", summary_snippet: "", fact_count: 0, open_task_count: 0, created_at: 0, updated_at: 0 },
      { id: "ent_b", name: "Beta", entity_type: "concept", summary_snippet: "", fact_count: 0, open_task_count: 0, created_at: 0, updated_at: 0 },
    ];
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_entities_cmd") return Promise.resolve(entities);
      return Promise.resolve(null);
    });

    await refreshWikilinkResolver();
    const cached = getWikilinkResolverEntities();
    expect(cached.map((e) => e.name)).toEqual(["Alpha", "Beta"]);
  });
});
