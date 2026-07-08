import { useTimeline } from "../../hooks/useTimeline";
import { TimelineFeed } from "../timeline/TimelineFeed";
import { useErrorFeed } from "../../hooks/useErrorFeed";
import type { NavTarget } from "../../lib/navigation";
import type { BackgroundError } from "../../lib/errorFeed";

interface Props {
  isOpen: boolean;
  onClose: () => void;
  onNavigate: (target: NavTarget) => void;
  errors?: BackgroundError[];
}

export function ActivityFeedPanel({
  isOpen,
  onClose,
  onNavigate,
  errors: errorsProp,
}: Props) {
  const { events, error } = useTimeline({ limit: 50 });
  const { errors: errorsFromHook, dismiss, retry } = useErrorFeed();
  const errors = errorsProp ?? errorsFromHook;

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
          {errors.length > 0 && (
            <div className="activity-errors">
              {errors.map((err) => (
                <div key={err.id} className="activity-error">
                  <p>{err.message}</p>
                  <div className="activity-error-actions">
                    {err.retry && (
                      <button
                        onClick={() => retry(err.id)}
                        className="activity-error-btn"
                      >
                        Retry
                      </button>
                    )}
                    <button
                      onClick={() => dismiss(err.id)}
                      className="activity-error-btn"
                    >
                      Dismiss
                    </button>
                  </div>
                </div>
              ))}
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
