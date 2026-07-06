import { usePrivacyMode, type PrivacyMode } from "../../hooks/usePrivacyMode";

const MODES: {
  id: PrivacyMode;
  label: string;
  summary: string;
}[] = [
  {
    id: "strict",
    label: "Strict (default)",
    summary: "Fully local. Nothing ever leaves this machine.",
  },
  {
    id: "ephemeral",
    label: "Ephemeral cloud inference",
    summary:
      "Local storage and embeddings; generation may use an external API with transient context.",
  },
  {
    id: "full",
    label: "Full cloud sync",
    summary:
      "Ephemeral inference plus Cloud Bridge sync of brain state (Clanker interop).",
  },
];

export function PrivacyPanel() {
  const { mode, setMode } = usePrivacyMode();

  return (
    <div className="settings-section">
      <h3>Privacy</h3>
      <p className="settings-hint">
        Choose your data posture. UI enforcement (hiding cloud fields, Cloud
        Bridge gating) ships in Phase 6 — this tab records your preference
        today.
      </p>
      <div className="privacy-options" role="radiogroup" aria-label="Privacy mode">
        {MODES.map((m) => (
          <label
            key={m.id}
            className={`privacy-option${
              mode === m.id ? " privacy-option--active" : ""
            }`}
          >
            <input
              type="radio"
              name="privacy-mode"
              value={m.id}
              checked={mode === m.id}
              onChange={() => setMode(m.id)}
            />
            <span className="privacy-option-label">{m.label}</span>
            <span className="privacy-option-summary">{m.summary}</span>
          </label>
        ))}
      </div>
    </div>
  );
}
