import { type BackgroundError } from "../../lib/errorFeed";

interface Props {
  open: boolean;
  onClose: () => void;
  errors?: BackgroundError[];
  onNavigate?: (mode: string) => void;
  onDismiss?: (id: number) => void;
}

export function ActivityFeedPanel({
  open,
  onClose,
  errors = [],
  onNavigate,
  onDismiss,
}: Props) {
  if (!open) return null;

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
          {errors.length === 0 && (
            <p className="settings-hint">
              Live librarian events will appear here. Full Timeline mode ships in
              a later phase.
            </p>
          )}
          {errors.length > 0 && (
            <div className="activity-error-list">
              {errors.map((err) => (
                <div key={err.id} className="activity-error-item">
                  <div className="activity-error-message">
                    <span>{err.message}</span>
                    <small>{new Date(err.at).toLocaleTimeString()}</small>
                  </div>
                  <div className="activity-error-actions">
                    {err.retry && (
                      <button
                        onClick={() => {
                          retry(err.id).catch(() => {});
                        }}
                        className="activity-error-btn"
                      >
                        Retry
                      </button>
                    )}
                    {onDismiss && (
                      <button
                        aria-label="Dismiss"
                        onClick={() => onDismiss(err.id)}
                        className="activity-error-btn"
                      >
                        Dismiss
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
          <p className="settings-hint">
            For now, check the status bar for indexing and synthesis state.
          </p>
          {onNavigate && (
            <button
              className="tree-file"
              onClick={() => onNavigate("timeline")}
              style={{ marginTop: 8 }}
            >
              Open full Timeline
            </button>
          )}
        </div>
      </aside>
    </>
  );
}

function retry(id: number): Promise<void> {
  // rethrow so the caller can catch
  return import("../../lib/errorFeed").then(({ retryError }) => retryError(id));
}
