interface Props {
  open: boolean;
  onClose: () => void;
}

export function ActivityFeedPanel({ open, onClose }: Props) {
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
          <p className="settings-hint">
            Live librarian events will appear here. Full Timeline mode ships in
            a later phase.
          </p>
          <p className="settings-hint">
            For now, check the status bar for indexing and synthesis state.
          </p>
        </div>
      </aside>
    </>
  );
}
