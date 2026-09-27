import { useTheme, type ThemePreference } from "../../lib/ThemeContext";

const OPTIONS: { id: ThemePreference; label: string; hint: string }[] = [
  {
    id: "light",
    label: "Light",
    hint: "Warm paper tones for daytime reading.",
  },
  {
    id: "dark",
    label: "Dark",
    hint: "Dim surfaces for low-light environments.",
  },
  {
    id: "system",
    label: "System",
    hint: "Follow your operating system appearance.",
  },
];

export function AppearancePanel() {
  const { preference, setPreference } = useTheme();

  return (
    <div className="settings-section">
      <h3>Appearance</h3>
      <p className="settings-hint">
        Theme applies to the shell and the BlockNote editor.
      </p>
      <div className="theme-options" role="radiogroup" aria-label="Theme">
        {OPTIONS.map((opt) => (
          <label
            key={opt.id}
            className={`theme-option${
              preference === opt.id ? " theme-option--active" : ""
            }`}
          >
            <input
              type="radio"
              name="theme"
              value={opt.id}
              checked={preference === opt.id}
              onChange={() => setPreference(opt.id)}
            />
            {/* Same drawn mark as the privacy cards. The radio is opacity:0,
                so without it the three theme options were distinguished only
                by a fill change — and the panel whose whole job is showing you
                which theme is active had no indicator at all. */}
            <span className="privacy-option__mark" aria-hidden="true">
              <svg className="icon icon--sm" viewBox="0 0 24 24" focusable="false">
                <circle cx="12" cy="12" r="8" />
                {preference === opt.id && (
                  <circle cx="12" cy="12" r="3.4" fill="currentColor" stroke="none" />
                )}
              </svg>
            </span>
            <span className="privacy-option__text">
              <span className="theme-option-label">{opt.label}</span>
              <span className="theme-option-hint">{opt.hint}</span>
            </span>
          </label>
        ))}
      </div>
    </div>
  );
}
