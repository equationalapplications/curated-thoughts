import { describe, it, expect } from "vitest";
import { parseSummary, groupByDay, KIND_ICONS, KIND_LABELS } from "../lib/timelineFormat";
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
      expect(segments).toHaveLength(5);
      expect(segments[0]).toEqual({ text: "Updated " });
      expect(segments[1]).toEqual({ text: "field A", em: true });
      expect(segments[2]).toEqual({ text: " and " });
      expect(segments[3]).toEqual({ text: "field B", em: true });
      expect(segments[4]).toEqual({ text: "" });
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

  describe("KIND_ICONS and KIND_LABELS", () => {
    it("has all kinds covered", () => {
      const kinds = ["ingested", "synthesized", "approved", "rejected", "healed", "imported", "exported", "agent_access", "other"] as const;
      for (const kind of kinds) {
        expect(KIND_ICONS[kind]).toBeDefined();
        expect(KIND_LABELS[kind]).toBeDefined();
      }
    });
  });
});
