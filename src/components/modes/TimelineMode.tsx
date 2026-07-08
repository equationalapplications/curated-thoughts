import { useState, useCallback, useMemo } from "react";
import { useTimeline } from "../../hooks/useTimeline";
import { TimelineFeed } from "../timeline/TimelineFeed";
import type { TimelineEvent } from "../../lib/tauri";

const EVENT_KINDS = [
  "approved",
  "rejected",
  "agent_access",
  "ingested",
  "synthesized",
  "healed",
  "imported",
  "exported",
] as const;

export function TimelineMode() {
  const [selectedKinds, setSelectedKinds] = useState<Set<string>>(new Set());
  const [entityFilter, setEntityFilter] = useState("");
  const [sinceMs, setSinceMs] = useState<number | null>(null);
  const [untilMs, setUntilMs] = useState<number | null>(null);

  const filter = useMemo(
    () => ({
      kinds: selectedKinds.size > 0 ? Array.from(selectedKinds) : undefined,
      entity_id: entityFilter.trim() || undefined,
      since_ms: sinceMs,
      until_ms: untilMs,
    }),
    [selectedKinds, entityFilter, sinceMs, untilMs]
  );

  const { events, error, refresh } = useTimeline(filter);

  // Client-side filtering for kinds (already filtered server-side, but keep as fallback)
  const filteredEvents = useMemo(() => {
    let result = events;
    if (selectedKinds.size > 0) {
      result = result.filter((e) => selectedKinds.has(e.kind));
    }
    if (entityFilter.trim()) {
      const lower = entityFilter.toLowerCase();
      result = result.filter(
        (e) =>
          (e.entity_name && e.entity_name.toLowerCase().includes(lower)) ||
          (e.entity_id && e.entity_id.toLowerCase().includes(lower))
      );
    }
    return result;
  }, [events, selectedKinds, entityFilter]);

  // Clear all filters
  const clearFilters = useCallback(() => {
    setSelectedKinds(new Set());
    setEntityFilter("");
    setSinceMs(null);
    setUntilMs(null);
  }, []);

  // Load older events (pagination) — currently a no-op; disable button until implemented
  const loadOlder = useCallback(() => {
    // Not yet implemented
  }, []);

  const hasFilters =
    selectedKinds.size > 0 ||
    entityFilter.trim() !== "" ||
    sinceMs !== null ||
    untilMs !== null;

  return (
    <div className="mode-layout">
      <div className="sidebar">
        <h2>Timeline</h2>
        <div className="search-bar">
          <input
            type="text"
            placeholder="Filter by entity…"
            value={entityFilter}
            onChange={(e) => setEntityFilter(e.target.value)}
          />
        </div>
        <div className="folder-tree">
          <div className="tree-section">
            <span className="tree-section-label">Event types</span>
            {EVENT_KINDS.map((kind) => (
              <label key={kind} className="tree-file-row">
                <input
                  type="checkbox"
                  checked={selectedKinds.has(kind)}
                  onChange={() => {
                    setSelectedKinds((prev) => {
                      const next = new Set(prev);
                      if (next.has(kind)) next.delete(kind);
                      else next.add(kind);
                      return next;
                    });
                  }}
                />
                <span className="tree-file">{kind}</span>
              </label>
            ))}
          </div>
          <div className="tree-section">
            <span className="tree-section-label">Date range</span>
            <input
              type="date"
              value={
                sinceMs
                  ? new Date(sinceMs).toISOString().slice(0, 10)
                  : ""
              }
              onChange={(e) =>
                setSinceMs(
                  e.target.value
                    ? new Date(e.target.value).getTime()
                    : null
                )
              }
              className="rule-input"
              placeholder="From"
            />
            <input
              type="date"
              value={
                untilMs
                  ? new Date(untilMs).toISOString().slice(0, 10)
                  : ""
              }
              onChange={(e) =>
                setUntilMs(
                  e.target.value
                    ? new Date(e.target.value).getTime()
                    : null
                )
              }
              className="rule-input"
              placeholder="To"
            />
          </div>
          {hasFilters && (
            <button
              className="tree-file"
              onClick={clearFilters}
              style={{ marginTop: 8 }}
            >
              Clear filters
            </button>
          )}
        </div>
      </div>
      <main className="editor-pane editor-pane--active">
        {error && <p className="editor-error">{error}</p>}
        <TimelineFeed events={filteredEvents} />
        {filteredEvents.length > 0 && (
          <button
            onClick={loadOlder}
            className="load-older-btn"
            disabled
            title="Pagination coming soon"
          >
            Load older
          </button>
        )}
      </main>
    </div>
  );
}
