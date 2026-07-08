import { type AppMode } from "./AppShell";

interface Props {
  mode: AppMode;
  reviewCount: number;
  canGoBack: boolean;
  canGoForward: boolean;
  onModeChange: (mode: AppMode) => void;
  onBack: () => void;
  onForward: () => void;
  onOpenActivity?: () => void;
  errorCount?: number;
}

const MODE_LABELS: Record<AppMode, string> = {
  brain: "Brain",
  review: "Review",
  library: "Library",
  settings: "Settings",
};

export function ModeRail({
  mode,
  reviewCount,
  canGoBack,
  canGoForward,
  onModeChange,
  onBack,
  onForward,
  onOpenActivity,
  errorCount = 0,
}: Props) {
  return (
    <nav className="mode-rail" aria-label="Mode navigation">
      <button
        className={`mode-rail-btn mode-rail-btn--back${canGoBack ? "" : " mode-rail-btn--disabled"}`}
        aria-label="Go back"
        disabled={!canGoBack}
        onClick={onBack}
      >
        ←
      </button>
      <button
        className={`mode-rail-btn mode-rail-btn--forward${canGoForward ? "" : " mode-rail-btn--disabled"}`}
        aria-label="Go forward"
        disabled={!canGoForward}
        onClick={onForward}
      >
        →
      </button>

      <div className="mode-rail-spacer" />

      {(["brain", "review", "library", "settings"] as const).map((m) => (
        <button
          key={m}
          className={`mode-rail-btn${mode === m ? " mode-rail-btn--active" : ""}`}
          aria-current={mode === m ? "page" : undefined}
          aria-label={MODE_LABELS[m]}
          onClick={() => onModeChange(m)}
        >
          {m === "review" && reviewCount > 0 && (
            <span className="mode-rail-badge">{reviewCount}</span>
          )}
          <span className="mode-rail-icon">{m === "brain" ? "🧠" : m === "review" ? "📋" : m === "library" ? "📚" : "⚙"}</span>
        </button>
      ))}

      <div className="mode-rail-spacer" />

      {onOpenActivity && (
        <button
          className={`mode-rail-btn${errorCount > 0 ? " mode-rail-btn--active" : ""}`}
          aria-label="Activity"
          onClick={onOpenActivity}
        >
          {errorCount > 0 && (
            <span className="mode-rail-badge">{errorCount}</span>
          )}
          <span className="mode-rail-icon">🔔</span>
        </button>
      )}
    </nav>
  );
}
