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
  // Return with newest first
  return [...groups.entries()]
    .reverse()
    .map(([day, evts]) => ({ day, events: evts }));
}

export const KIND_ICONS: Record<TimelineKind, string> = {
  ingested: "📄",
  synthesized: "✨",
  approved: "✅",
  rejected: "🚫",
  healed: "🩹",
  imported: "📦",
  exported: "📤",
  agent_access: "🤖",
  other: "•",
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
