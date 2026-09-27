import { describe, it, expect } from "vitest";
import { parseSummary, groupByDay, shortenPathForDisplay, KIND_ICON_PATHS, KIND_LABELS } from "../lib/timelineFormat";
import type { TimelineEvent } from "../lib/tauri";

describe("timelineFormat", () => {
  describe("parseSummary", () => {
    it("renders_*name*_spans_as_emphasis", () => {
      const segments = parseSummary("Approved *Project X*");
      expect(segments).toHaveLength(2);
      expect(segments[0]).toEqual({ text: "Approved " });
      expect(segments[1]).toEqual({ text: "Project X", em: true });
    });

    it("handles multiple emphasized segments", () => {
      const segments = parseSummary("Updated *field A* and *field B*");
      expect(segments).toHaveLength(4);
      expect(segments[0]).toEqual({ text: "Updated " });
      expect(segments[1]).toEqual({ text: "field A", em: true });
      expect(segments[2]).toEqual({ text: " and " });
      expect(segments[3]).toEqual({ text: "field B", em: true });
    });

    it("handles no emphasis", () => {
      const segments = parseSummary("Just plain text");
      expect(segments).toHaveLength(1);
      expect(segments[0]).toEqual({ text: "Just plain text" });
    });

    it("handles leading emphasis", () => {
      const segments = parseSummary("*Emphasized* start");
      expect(segments).toHaveLength(2);
      expect(segments[0]).toEqual({ text: "Emphasized", em: true });
      expect(segments[1]).toEqual({ text: " start" });
    });
  });

  describe("shortenPathForDisplay", () => {
    it("shortens Windows drive paths and keeps their separator", () => {
      expect(
        shortenPathForDisplay("Ingested C:\\Users\\Maya\\Vault\\People\\Maya Chen.md"),
      ).toBe("Ingested …\\People\\Maya Chen.md");
      expect(shortenPathForDisplay("Ingested D:/Vault/immutable-source-files/People/Note.md")).toBe(
        "Ingested …/People/Note.md",
      );
    });

    it("shortens Windows UNC paths", () => {
      expect(shortenPathForDisplay("Ingested \\\\server\\share\\Vault\\People\\Note.md")).toBe(
        "Ingested …\\People\\Note.md",
      );
    });

    it("keeps the last two path components and elides the directory prefix", () => {
      expect(
        shortenPathForDisplay(
          "Ingested /private/tmp/claude-502/DemoVault/immutable-source-files/People/Maya Chen.md",
        ),
      ).toBe("Ingested …/People/Maya Chen.md");
    });

    it("leaves a short path alone — no elision for two components or fewer", () => {
      expect(shortenPathForDisplay("Ingested /vault/notes.md")).toBe(
        "Ingested /vault/notes.md",
      );
    });

    it("leaves text with no path alone", () => {
      expect(shortenPathForDisplay("Librarian is idle.")).toBe("Librarian is idle.");
    });

    it("leaves a bare ratio alone — the slash does not start a token", () => {
      expect(shortenPathForDisplay("Approved 3/4 facts")).toBe("Approved 3/4 facts");
    });

    it("keeps a spaced filename intact", () => {
      expect(
        shortenPathForDisplay(
          "/private/tmp/DemoVault/immutable-source-files/People/Maya Chen.md",
        ),
      ).toBe("…/People/Maya Chen.md");
    });

    it("leaves an emphasised entity name alone — no slash at all", () => {
      expect(shortenPathForDisplay("Project X")).toBe("Project X");
    });
  });

  describe("groupByDay", () => {
    it("groups_events_by_local_day", () => {
      const now = Date.now();
      const yesterday = now - 24 * 60 * 60 * 1000;

      const events: TimelineEvent[] = [
        {
          id: "e1",
          kind: "synthesized",
          summary: "Event 1",
          created_at_ms: now,
          raw_type: "test",
        },
        {
          id: "e2",
          kind: "approved",
          summary: "Event 2",
          created_at_ms: now - 1000,
          raw_type: "test",
        },
        {
          id: "e3",
          kind: "ingested",
          summary: "Event 3",
          created_at_ms: yesterday,
          raw_type: "test",
        },
      ];

      const groups = groupByDay(events);
      expect(groups).toHaveLength(2);
      // Newest day first
      expect(groups[0].events).toHaveLength(2);
      expect(groups[1].events).toHaveLength(1);
    });

    it("returns empty array for empty input", () => {
      const groups = groupByDay([]);
      expect(groups).toHaveLength(0);
    });
  });

  describe("KIND_ICON_PATHS and KIND_LABELS", () => {
    it("has all kinds covered", () => {
      const kinds = ["ingested", "synthesized", "approved", "rejected", "healed", "imported", "exported", "agent_access", "other"] as const;
      for (const kind of kinds) {
        expect(KIND_ICON_PATHS[kind]).toBeDefined();
        expect(KIND_LABELS[kind]).toBeDefined();
      }
    });

    // The icons are inline SVG path data, not emoji: each entry must be a
    // non-empty list of path commands, and none may contain a non-ASCII
    // character (which is how an emoji sneaks back in).
    it("every kind has at least one drawable, ASCII-only path", () => {
      for (const [kind, paths] of Object.entries(KIND_ICON_PATHS)) {
        expect(paths.length, `${kind} has paths`).toBeGreaterThan(0);
        for (const d of paths) {
          expect(d.length, `${kind} path is non-empty`).toBeGreaterThan(0);
          expect(/^[\x20-\x7E]+$/.test(d), `${kind} path is ASCII: ${d}`).toBe(true);
        }
      }
    });
  });
});
