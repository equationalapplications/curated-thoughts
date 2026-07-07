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
          <span className="privacy-option-label">{m.label}</span>
          <span className="privacy-option-summary">{m.summary}</span>
        </label>
      ))}
    </div>
  );
}
