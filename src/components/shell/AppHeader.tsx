interface Props {
  onSettingsOpen: () => void;
}

export function AppHeader({ onSettingsOpen }: Props) {
  return (
    <header className="app-header">
      <span className="app-header-title">Curated Thoughts</span>
      <button
        className="app-header-settings"
        onClick={onSettingsOpen}
        aria-label="Settings"
        title="Settings"
      >
        ⚙
      </button>
    </header>
  );
}
