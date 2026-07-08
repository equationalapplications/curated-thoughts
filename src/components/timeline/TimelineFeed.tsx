import type { TimelineEvent } from "../../lib/tauri";
import type { NavTarget } from "../../lib/navigation";
import { groupByDay, parseSummary, KIND_ICONS } from "../../lib/timelineFormat";

interface Props {
  events: TimelineEvent[];
  powerLayer: boolean;
  onNavigate: (target: NavTarget) => void;
}

export function TimelineFeed({ events, powerLayer, onNavigate }: Props) {
  const groups = groupByDay(events);

  if (groups.length === 0) {
    return <p className="placeholder">No activity.</p>;
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
                    <span className="icon">{KIND_ICONS[event.kind]}</span>
                    <span className="summary">
                      {segments.map((seg, idx) =>
                        seg.em ? (
                          <em key={idx}>{seg.text}</em>
                        ) : (
                          <span key={idx}>{seg.text}</span>
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
