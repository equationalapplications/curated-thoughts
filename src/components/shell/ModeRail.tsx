import type { ReactElement } from "react";

export type AppMode = "brain" | "review" | "library" | "timeline" | "tasks" | "settings" | "setup";

interface Props {
  mode: AppMode;
  reviewCount: number;
  errorCount?: number;
  onModeChange: (mode: AppMode) => void;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
  onOpenActivity: () => void;
}

/**
 * Inline line icons rather than emoji. Emoji render at the platform's chosen
 * size and colour (a full-colour glyph, not a stroke), which is what made the
 * rail read as a different app from the rest of the chrome. `stroke` inherits
 * `currentColor`, so an icon is never darker than the label it sits under.
 */
function Icon({ path, className }: { path: ReactElement; className?: string }) {
  return (
    <svg
      className={className ?? "icon icon--lg"}
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      {path}
    </svg>
  );
}

const ICONS: Record<string, ReactElement> = {
  brain: (
    <>
      <path d="M12 5.5a3 3 0 0 0-5.6 1.4A2.8 2.8 0 0 0 4 9.4a3 3 0 0 0 1 2.2 3 3 0 0 0 .6 4.3A3 3 0 0 0 12 18z" />
      <path d="M12 5.5a3 3 0 0 1 5.6 1.4A2.8 2.8 0 0 1 20 9.4a3 3 0 0 1-1 2.2 3 3 0 0 1-.6 4.3A3 3 0 0 1 12 18z" />
      <path d="M12 5.5V18" />
    </>
  ),
  review: (
    <>
      <path d="M3.5 13h4l1.5 3h6l1.5-3h4" />
      <path d="M5.5 5h13l2 8v4.5A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5V13z" />
    </>
  ),
  library: (
    <>
      <path d="M4 5.5A1.5 1.5 0 0 1 5.5 4H9v16H5.5A1.5 1.5 0 0 1 4 18.5z" />
      <path d="M10 4h4v16h-4z" />
      <path d="m15.6 5.2 3.9 1-3.4 14.6-3.9-1z" />
    </>
  ),
  timeline: (
    <>
      <circle cx="12" cy="12" r="7.5" />
      <path d="M12 8v4.3l2.8 1.7" />
    </>
  ),
  tasks: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <path d="m8.4 12 2.3 2.3 4.9-4.9" />
    </>
  ),
  activity: <path d="M3 12h3.5l2-5.5 3.5 11 2.5-8 1.8 2.5H21" />,
  settings: (
    <>
      <circle cx="12" cy="12" r="2.9" />
      <path d="M12 3.5v2.1M12 18.4v2.1M20.5 12h-2.1M5.6 12H3.5M18 6l-1.5 1.5M7.5 16.5 6 18M18 18l-1.5-1.5M7.5 7.5 6 6" />
    </>
  ),
  back: <path d="M14.5 5.5 8 12l6.5 6.5" />,
  forward: <path d="M9.5 5.5 16 12l-6.5 6.5" />,
};

const MAIN_MODES: { id: AppMode; label: string; icon: string }[] = [
  { id: "brain", label: "Brain", icon: "brain" },
  { id: "review", label: "Review", icon: "review" },
  { id: "library", label: "Library", icon: "library" },
  { id: "timeline", label: "Timeline", icon: "timeline" },
  { id: "tasks", label: "Tasks", icon: "tasks" },
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
      <Icon path={ICONS[icon]} />
      {badge !== undefined && badge > 0 && (
        <span className="mode-rail-badge">{badge}</span>
      )}
    </button>
  );
}

export function ModeRail({
  mode,
  reviewCount,
  errorCount,
  onModeChange,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
  onOpenActivity,
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
        <Icon path={ICONS.back} className="icon icon--sm" />
      </button>
      <button
        className="mode-rail-history-btn"
        aria-label="Go forward"
        title="Go forward"
        disabled={!canGoForward}
        onClick={onForward}
      >
        <Icon path={ICONS.forward} className="icon icon--sm" />
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
      <button
        className="mode-rail-btn"
        aria-label="Activity"
        title="Activity"
        onClick={onOpenActivity}
      >
        <Icon path={ICONS.activity} />
        {errorCount !== undefined && errorCount > 0 && (
          <span className="mode-rail-badge">{errorCount}</span>
        )}
      </button>
      <RailButton
        id="settings"
        label="Settings"
        icon="settings"
        active={mode === "settings"}
        onClick={onModeChange}
      />
    </nav>
  );
}
