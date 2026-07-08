import { useCallback, useMemo, useState } from "react";
import type { TimelineKind, TimelineFilter } from "../../lib/tauri";
import { useTimeline } from "../../hooks/useTimeline";
import type { NavTarget } from "../../lib/navigation";
import { TimelineFeed } from "../timeline/TimelineFeed";
import { KIND_LABELS } from "../../lib/timelineFormat";

interface Props {
  onNavigate: (target: NavTarget) => void;
}

export function TimelineMode({ onNavigate }: Props) {
  const [selectedKinds, setSelectedKinds] = useState<Set<TimelineKind>>(new Set());
  const [entityFilter, setEntityFilter] = useState<string>("");
  const [sinceMs, setSinceMs] = useState<number | null>(null);
  const [untilMs, setUntilMs] = useState<number | null>(null);
  const [powerLayer, setPowerLayer] = useState(false);

  // Build filter for the hook
  const filter = useMemo<TimelineFilter>(() => {
    const f: TimelineFilter = {};
    if (selectedKinds.size > 0) {
      f.kinds = Array.from(selectedKinds);
    }
    if (sinceMs) {
      f.since_ms = sinceMs;
    }
    if (untilMs) {
      f.until_ms = untilMs;
    }
    return f;
  }, [selectedKinds, sinceMs, untilMs]);

  const { events, error, refresh } = useTimeline(filter);

  // Client-side entity name filter
  const filteredEvents = useMemo(() => {
    if (!entityFilter.trim()) return events;
    const needle = entityFilter.toLowerCase();
    return events.filter((e) => e.entity_name?.toLowerCase().includes(needle));
  }, [events, entityFilter]);

  // Handle kind checkbox toggle
  const toggleKind = useCallback((kind: TimelineKind) => {
    setSelectedKinds((prev) => {
      const next = new Set(prev);
      if (next.has(kind)) {
        next.delete(kind);
      } else {
        next.add(kind);
      }
      return next;
    });
  }, []);

  // Clear all filters
  const clearFilters = useCallback(() => {
    setSelectedKinds(new Set());
    setEntityFilter("");
    setSinceMs(null);
    setUntilMs(null);
  }, []);

  // Load older events (pagination)
  const loadOlder = useCallback(() => {
    if (filteredEvents.length === 0) return;
    const lastEvent = filteredEvents[filteredEvents.length - 1];
    // Create a new filter with before_ms set to the last event's timestamp
    const olderFilter: TimelineFilter = { ...filter, before_ms: lastEvent.created_at_ms, limit: 50 };
    useTimeline(olderFilter);
  }, [filteredEvents, filter]);

  const hasFilters = selectedKinds.size > 0 || entityFilter.trim() !== "" || sinceMs || untilMs;

  return (
    <div className="mode-layout">
      <aside className="mode-sidebar">
        <div className="filters-section">
          <h3>Kind</h3>
          <div className="kind-filters">
            {(Object.keys(KIND_LABELS) as TimelineKind[]).map((kind) => (
              <label key={kind}>
                <input
                  type="checkbox"
                  checked={selectedKinds.has(kind)}
                  onChange={() => toggleKind(kind)}
                />
                {KIND_LABELS[kind]}
              </label>
            ))}
          </div>

          <h3>Entity</h3>
          <input
            type="text"
            placeholder="Filter by entity name…"
            value={entityFilter}
            onChange={(e) => setEntityFilter(e.target.value)}
            className="entity-filter-input"
          />

          <h3>Date Range</h3>
          <div className="date-inputs">
            <label>
              Since (ms)
              <input
                type="number"
                value={sinceMs ?? ""}
                onChange={(e) => setSinceMs(e.target.value ? parseInt(e.target.value) : null)}
                placeholder="Leave empty for any"
              />
            </label>
            <label>
              Until (ms)
              <input
                type="number"
                value={untilMs ?? ""}
                onChange={(e) => setUntilMs(e.target.value ? parseInt(e.target.value) : null)}
                placeholder="Leave empty for any"
              />
            </label>
          </div>

          {hasFilters && (
            <button onClick={clearFilters} className="clear-filters-btn">
              Clear filters
            </button>
          )}
        </div>
      </aside>

      <main className="mode-main">
        <div className="timeline-controls">
          <label className="power-layer-toggle">
            <input
              type="checkbox"
              checked={powerLayer}
              onChange={(e) => setPowerLayer(e.target.checked)}
            />
            Power layer
          </label>
        </div>

        {error && (
          <div className="error-banner" role="alert">
            {error}
          </div>
        )}

        <TimelineFeed events={filteredEvents} powerLayer={powerLayer} onNavigate={onNavigate} />

        {filteredEvents.length > 0 && (
          <button onClick={loadOlder} className="load-older-btn">
            Load older
          </button>
        )}
      </main>
    </div>
  );
}
