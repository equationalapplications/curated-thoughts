import type { TimelineEvent, TimelineKind } from "./tauri";

export interface SummarySegment {
  text: string;
  em?: boolean;
}

/**
 * Parse *text* into em segments and plain text.
 * Example: "Approved *Project X*" → [{ text: "Approved " }, { text: "Project X", em: true }]
 */
export function parseSummary(summary: string): SummarySegment[] {
  const out: SummarySegment[] = [];
  const re = /\*([^*]+)\*/g;
  let last = 0;
  for (let m = re.exec(summary); m; m = re.exec(summary)) {
    if (m.index > last) out.push({ text: summary.slice(last, m.index) });
    out.push({ text: m[1], em: true });
    last = re.lastIndex;
  }
  if (last < summary.length) out.push({ text: summary.slice(last) });
  return out;
}

/**
 * Shorten an absolute path for display, keeping the last two components.
 *
 * An ingested event's summary embeds the document's *absolute* path, which on
 * a real vault is 90+ characters of identical prefix — and the only part that
 * tells two `Ingested` rows apart is the tail. The feed clips each row to one
 * line, and clipping the tail left every row reading
 * "Ingested /private/tmp/…/Demo…" — seven identical lines. Eliding the
 * directory prefix instead keeps the distinguishing part on screen:
 * "Ingested …/People/Maya Chen.md".
 *
 * Purely presentational: the untruncated text stays in the event's `summary`
 * and is exposed as the row's `title`, so nothing is lost.
 *
 * Only a "/" that begins a token is treated as a path start, so a sentence
 * containing a ratio or a date ("approved 3/4", "seen 9/12") is left alone.
 * The path runs to the end of the string rather than to the next space,
 * because the last component of a real path can contain spaces
 * ("Maya Chen.md") and stopping at the space would drop the filename.
 */
export function shortenPathForDisplay(text: string): string {
  const start = /(^|\s)\//.exec(text);
  if (!start) return text;
  const from = start.index + start[0].length - 1;
  const path = text.slice(from);
  const segments = path.split("/").filter(Boolean);
  if (segments.length <= 2) return text;
  return text.slice(0, from) + `…/${segments.slice(-2).join("/")}`;
}

/**
 * Group events by local day, with most recent first.
 */
export function groupByDay(events: TimelineEvent[]): { day: string; events: TimelineEvent[] }[] {
  const groups = new Map<string, TimelineEvent[]>();
  for (const e of events) {
    const day = new Date(e.created_at_ms).toLocaleDateString(undefined, {
      year: "numeric",
      month: "long",
      day: "numeric",
    });
    (groups.get(day) ?? groups.set(day, []).get(day)!).push(e);
  }
  // Groups are in insertion order (newest first when events are chronologically sorted)
  return [...groups.entries()].map(([day, evts]) => ({ day, events: evts }));
}

/**
 * Inline SVG path data per timeline kind, drawn on a 24x24 grid with a
 * 1.5px stroke so they match the rest of the line-icon set. Emoji were used
 * here before: they render as full-colour glyphs at the platform's chosen
 * size, which is why the feed looked like it came from a different app.
 * `KIND_ICON_PATHS.other` is a dot rather than a bullet character, so it
 * scales with the icon box instead of sitting on the text baseline.
 */
export const KIND_ICON_PATHS: Record<TimelineKind, string[]> = {
  ingested: ["M6 4.5A1.5 1.5 0 0 1 7.5 3h5.2L18 8.3v11.2a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 19.5z"],
  synthesized: [
    "M12 4.2v2.1M12 17.7v2.1M4.2 12h2.1M17.7 12h2.1",
    "M6.6 6.6 8.1 8.1M15.9 15.9l1.5 1.5M17.4 6.6 15.9 8.1M8.1 15.9l-1.5 1.5",
  ],
  approved: ["m5.5 12.5 4.2 4.2 8.8-9.4"],
  rejected: ["M6 6l12 12M18 6 6 18"],
  healed: [
    "M4.5 12h3.2l1.4-2.8 2.6 5.6 1.6-3.4 1 1.4h5.2",
  ],
  imported: [
    "M12 3.5v9.5M8.6 9.6 12 13l3.4-3.4",
    "M4.5 15.5v3A2 2 0 0 0 6.5 20.5h11a2 2 0 0 0 2-2v-3",
  ],
  exported: [
    "M12 13.5V4M8.6 7.4 12 4l3.4 3.4",
    "M4.5 15.5v3A2 2 0 0 0 6.5 20.5h11a2 2 0 0 0 2-2v-3",
  ],
  agent_access: [
    "M12 3.5a2 2 0 0 1 2 2v1.2a2 2 0 0 1-1.2 1.9V10a2 2 0 0 1-1.2 1.9v1.4a2 2 0 0 1-1.2 1.9v.6a2 2 0 0 1-2 2v-2.4l-1.2-1.2V12l-1.2-1.2V9.6a2 2 0 0 1-1.2-1.9V8a2 2 0 0 1 2-2",
  ],
  other: ["M12 12h.01"],
};

export const KIND_LABELS: Record<TimelineKind, string> = {
  ingested: "Ingested",
  synthesized: "Synthesized",
  approved: "Approved",
  rejected: "Rejected",
  healed: "Healed",
  imported: "Imported",
  exported: "Exported",
  agent_access: "Agent access",
  other: "Other",
};
