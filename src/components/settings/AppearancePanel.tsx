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
            <span className="theme-option-label">{opt.label}</span>
            <span className="theme-option-hint">{opt.hint}</span>
          </label>
        ))}
      </div>
    </div>
  );
}
