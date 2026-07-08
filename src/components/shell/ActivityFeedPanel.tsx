import { useTimeline } from "../../hooks/useTimeline";
import { TimelineFeed } from "../timeline/TimelineFeed";
import type { NavTarget } from "../../lib/navigation";

interface Props {
  isOpen: boolean;
  onClose: () => void;
  onNavigate: (target: NavTarget) => void;
}

export function ActivityFeedPanel({ isOpen, onClose, onNavigate }: Props) {
  const { events, error } = useTimeline({ limit: 50 });

  if (!isOpen) return null;

  return (
    <>
      <button
        type="button"
        className="activity-backdrop"
        aria-label="Close activity feed"
        onClick={onClose}
      />
      <aside
        className="activity-panel"
        role="dialog"
        aria-label="Activity feed"
        aria-modal="true"
      >
        <header className="activity-panel-header">
          <h2>Activity</h2>
          <button
            type="button"
            className="activity-panel-close"
            aria-label="Close"
            onClick={onClose}
          >
            ×
          </button>
        </header>
        <div className="activity-panel-body">
          {error && (
            <div className="error-banner" role="alert">
              {error}
            </div>
          )}
          <TimelineFeed
            events={events}
            powerLayer={false}
            onNavigate={onNavigate}
          />
        </div>
        <footer className="activity-panel-footer">
          <button
            className="activity-open-timeline"
            onClick={() => {
              onNavigate({ mode: "timeline" });
              onClose();
            }}
          >
            Open full Timeline
          </button>
        </footer>
      </aside>
    </>
  );
}
