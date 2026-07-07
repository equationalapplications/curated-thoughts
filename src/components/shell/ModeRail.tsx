export type AppMode = "brain" | "review" | "library" | "settings";

interface Props {
  mode: AppMode;
  reviewCount: number;
  onModeChange: (mode: AppMode) => void;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
}

const MAIN_MODES: { id: AppMode; label: string; icon: string }[] = [
  { id: "brain", label: "Brain", icon: "🧠" },
  { id: "review", label: "Review", icon: "📥" },
  { id: "library", label: "Library", icon: "📚" },
];

function RailButton({
  id,
  label,
  icon,
  active,
  badge,
  onClick,
}: {
  id: AppMode;
  label: string;
  icon: string;
  active: boolean;
  badge?: number;
  onClick: (mode: AppMode) => void;
}) {
  return (
    <button
      className={`mode-rail-btn${active ? " mode-rail-btn--active" : ""}`}
      aria-label={label}
      aria-current={active ? "page" : undefined}
      title={label}
      onClick={() => onClick(id)}
    >
      <span aria-hidden="true">{icon}</span>
      {badge !== undefined && badge > 0 && (
        <span className="mode-rail-badge">{badge}</span>
      )}
    </button>
  );
}

export function ModeRail({
  mode,
  reviewCount,
  onModeChange,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
}: Props) {
  return (
    <nav className="mode-rail" aria-label="Primary">
      <button
        className="mode-rail-history-btn"
        aria-label="Go back"
        title="Go back"
        disabled={!canGoBack}
        onClick={onBack}
      >
        ‹
      </button>
      <button
        className="mode-rail-history-btn"
        aria-label="Go forward"
        title="Go forward"
        disabled={!canGoForward}
        onClick={onForward}
      >
        ›
      </button>
      {MAIN_MODES.map((m) => (
        <RailButton
          key={m.id}
          id={m.id}
          label={m.label}
          icon={m.icon}
          active={mode === m.id}
          badge={m.id === "review" ? reviewCount : undefined}
          onClick={onModeChange}
        />
      ))}
      <div className="mode-rail-spacer" />
      <RailButton
        id="settings"
        label="Settings"
        icon="⚙"
        active={mode === "settings"}
        onClick={onModeChange}
      />
    </nav>
  );
}
