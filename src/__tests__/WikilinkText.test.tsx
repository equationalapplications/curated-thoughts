import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, test, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  parseWikilinks,
  WikilinkText,
  refreshWikilinkResolver,
  getWikilinkResolverEntities,
  __resetWikilinkResolverForTests,
} from "../components/brain/WikilinkText";
import type { EntitySummary } from "../lib/tauri";

function entity(name: string): EntitySummary {
  return {
    id: `ent_${name.toLowerCase()}`,
    name,
    entity_type: "concept",
    summary_snippet: "",
    fact_count: 0,
    open_task_count: 0,
    created_at: 0,
    updated_at: 0,
  };
}

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

describe("WikilinkText resolver coalescing + test reset", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    __resetWikilinkResolverForTests();
  });

  test("refreshWikilinkResolver coalesces concurrent calls into one IPC round-trip", async () => {
    let listCallCount = 0;
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_entities_cmd") {
        listCallCount += 1;
        return Promise.resolve([]);
      }
      return Promise.resolve(null);
    });

    // Two concurrent refreshes share one promise.
    const p1 = refreshWikilinkResolver();
    const p2 = refreshWikilinkResolver();
    await Promise.all([p1, p2]);

    expect(listCallCount).toBe(1);
  });

  test("refreshWikilinkResolver launches a fresh round-trip after the previous one settles", async () => {
    let listCallCount = 0;
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_entities_cmd") {
        listCallCount += 1;
        return Promise.resolve([]);
      }
      return Promise.resolve(null);
    });

    await refreshWikilinkResolver();
    expect(listCallCount).toBe(1);

    await refreshWikilinkResolver();
    expect(listCallCount).toBe(2);
  });

  test("__resetWikilinkResolverForTests clears state without firing an IPC round-trip", () => {
    vi.mocked(invoke).mockReset();
    __resetWikilinkResolverForTests();
    // getWikilinkResolverEntities returns [] on a freshly-reset cache (loading state).
    expect(getWikilinkResolverEntities()).toEqual([]);
    expect(invoke).not.toHaveBeenCalled();
  });

  test("a refresh started before the reset cannot repopulate the cache after it", async () => {
    let releaseFirst: (entities: EntitySummary[]) => void = () => {};
    let listCallCount = 0;
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd !== "list_entities_cmd") return Promise.resolve(null);
      listCallCount += 1;
      if (listCallCount === 1) {
        return new Promise<EntitySummary[]>((resolve) => {
          releaseFirst = resolve;
        });
      }
      return Promise.resolve([]);
    });

    const stale = refreshWikilinkResolver();

    // Tear the module state down while the first fetch is still in flight,
    // then let it land. Without the generation guard it would write its
    // entities into the post-reset cache.
    __resetWikilinkResolverForTests();
    releaseFirst([entity("Stale")]);
    await stale;

    expect(getWikilinkResolverEntities()).toEqual([]);
  });

  test("a pre-reset refresh settling later does not clear a newer refreshInFlight", async () => {
    const releases: Array<(entities: EntitySummary[]) => void> = [];
    let listCallCount = 0;
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd !== "list_entities_cmd") return Promise.resolve(null);
      listCallCount += 1;
      return new Promise<EntitySummary[]>((resolve) => {
        releases.push(resolve);
      });
    });

    const stale = refreshWikilinkResolver();
    __resetWikilinkResolverForTests();

    // Newer refresh starts and stays in flight; then the stale one settles.
    // Its `finally` must not null out the newer promise, or `alsoFresh` below
    // would start a third round-trip instead of coalescing.
    const fresh = refreshWikilinkResolver();
    releases[0]([entity("Stale")]);
    await stale;

    const alsoFresh = refreshWikilinkResolver();
    releases[1]([entity("Fresh")]);
    await Promise.all([fresh, alsoFresh]);

    // 1 stale + 1 fresh; `alsoFresh` coalesced into `fresh`.
    expect(listCallCount).toBe(2);
  });
});
