import { acknowledgeMigrationDisclosure } from "../../lib/tauri";
import { PRIVACY_MODES } from "./PrivacyModeCards";

interface Props {
  onAcknowledged: () => void;
}

export function MigrationDisclosureModal({ onAcknowledged }: Props) {
  const connected = PRIVACY_MODES.find((m) => m.id === "connected");

  const handleAcknowledge = async () => {
    await acknowledgeMigrationDisclosure();
    onAcknowledged();
  };

  return (
    <div className="privacy-modal-backdrop" role="presentation">
      <div
        className="privacy-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="migration-disclosure-title"
      >
        <h2 id="migration-disclosure-title">Connected agent privacy</h2>
        <p className="settings-hint">
          You already paired a Cloud Bridge token before privacy modes were enforced.
          Your posture has been set to Connected agent to match that reality.
        </p>
        <p className="privacy-option-summary">{connected?.summary}</p>
        <button type="button" onClick={handleAcknowledge}>
          I understand
        </button>
      </div>
    </div>
  );
}
