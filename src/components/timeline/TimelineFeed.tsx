import type { TimelineEvent } from "../../lib/tauri";
import type { NavTarget } from "../../lib/navigation";
import { groupByDay, parseSummary, shortenPathForDisplay, KIND_ICON_PATHS } from "../../lib/timelineFormat";

interface Props {
  events: TimelineEvent[];
  powerLayer: boolean;
  onNavigate: (target: NavTarget) => void;
}

export function TimelineFeed({ events, powerLayer, onNavigate }: Props) {
  const groups = groupByDay(events);

  if (groups.length === 0) {
    // Rendered *inside* .timeline-feed so the empty state inherits the feed's
    // padding. Bailing out early put a bare <p> flush against the column's
    // left divider.
    return (
      <div className="timeline-feed">
        <p className="placeholder">No activity.</p>
      </div>
    );
  }

  return (
    <div className="timeline-feed">
      {groups.map((group) => (
        <div key={group.day} className="day-group">
          <h3 className="day-header">{group.day}</h3>
          <div className="events-list">
            {group.events.map((event) => {
              const isClickable = !!event.entity_id || !!event.doc_path;
              const handleClick = () => {
                if (event.entity_id) {
                  onNavigate({ mode: "brain", entityId: event.entity_id });
                } else if (event.doc_path) {
                  onNavigate({ mode: "library", docPath: event.doc_path });
                }
              };

              const timestamp = new Date(event.created_at_ms).toLocaleTimeString(undefined, {
                hour: "2-digit",
                minute: "2-digit",
              });

              const segments = parseSummary(event.summary);

              return (
                <div
                  key={event.id}
                  className={`event-row ${isClickable ? "clickable" : ""}`}
                  title={event.summary.replace(/\*/g, "")}
                  onClick={isClickable ? handleClick : undefined}
                  role={isClickable ? "button" : undefined}
                  tabIndex={isClickable ? 0 : -1}
                  onKeyDown={
                    isClickable
                      ? (e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.preventDefault();
                            handleClick();
                          }
                        }
                      : undefined
                  }
                >
                  <div className="event-main">
                    <svg
                      className="icon"
                      viewBox="0 0 24 24"
                      aria-hidden="true"
                      focusable="false"
                    >
                      {KIND_ICON_PATHS[event.kind].map((d) => (
                        <path key={d} d={d} />
                      ))}
                    </svg>
                    <span className="summary">
                      {/* The elision runs on BOTH branches, not just the
                          plain one. The Rust event summary formats an
                          ingest as `Ingested *<path>*` — the path is the
                          *emphasised* segment — so a helper applied only to
                          plain text left every ingested row showing its full
                          absolute path. For genuinely emphasised names
                          ("Approved *Project X*") the helper is a no-op:
                          there is no "/" in the text. */}
                      {segments.map((seg, idx) =>
                        seg.em ? (
                          <em key={idx}>{shortenPathForDisplay(seg.text)}</em>
                        ) : (
                          <span key={idx}>{shortenPathForDisplay(seg.text)}</span>
                        ),
                      )}
                    </span>
                    <span className="timestamp">{timestamp}</span>
                  </div>
                  {powerLayer && (
                    <div className="power-layer">
                      <code>
                        {event.raw_type} · {event.id}
                        {event.client ? ` · ${event.client}` : ""}
                      </code>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </div>
  );
}
