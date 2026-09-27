import type { PrivacyMode } from "../../hooks/usePrivacyMode";

export const PRIVACY_MODES: {
  id: PrivacyMode;
  label: string;
  summary: string;
}[] = [
  {
    id: "strict",
    label: "Strict (default)",
    summary:
      "Fully local. Inference, embeddings, and storage all on-device. Cloud Bridge and external API fields disabled. Nothing ever leaves this machine.",
  },
  {
    id: "ephemeral",
    label: "Ephemeral cloud inference",
    summary:
      "Local storage and embeddings; generation may route to an external OpenAI-compatible API. Sent context is transient and never stored remotely.",
  },
  {
    id: "connected",
    label: "Connected agent (Cloud Bridge)",
    summary:
      "Ephemeral inference plus the Clanker Cloud Bridge: your Clanker agent may query the vault on demand over a read-only channel. Individual query results leave the machine when the agent asks; nothing syncs, nothing is stored remotely as a copy of the brain, and nothing can be written back over this channel.",
  },
];

interface Props {
  mode: PrivacyMode;
  onChange: (mode: PrivacyMode) => void;
  disabled?: boolean;
}

export function PrivacyModeCards({ mode, onChange, disabled = false }: Props) {
  return (
    <div className="privacy-options" role="radiogroup" aria-label="Privacy mode">
      {PRIVACY_MODES.map((m) => (
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
            disabled={disabled}
            onChange={() => onChange(m.id)}
          />
          {/* A drawn radio mark, not the platform's. The real input is
              opacity:0 (it stays focusable and screen-reader-visible), so
              without this the three cards had no radio at all — the only
              difference between selected and unselected was a fill change,
              which is a weak signal for the one control in Settings that
              governs whether data leaves the machine. */}
          <span className="privacy-option__mark" aria-hidden="true">
            <svg className="icon icon--sm" viewBox="0 0 24 24" focusable="false">
              <circle cx="12" cy="12" r="8" />
              {mode === m.id && <circle cx="12" cy="12" r="3.4" fill="currentColor" stroke="none" />}
            </svg>
          </span>
          <span className="privacy-option__text">
            <span className="privacy-option-label">{m.label}</span>
            <span className="privacy-option-summary">{m.summary}</span>
          </span>
        </label>
      ))}
    </div>
  );
}
