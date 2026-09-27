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
        className="dialog-surface"
        role="dialog"
        aria-modal="true"
        aria-labelledby="migration-disclosure-title"
      >
        <header className="dialog-header">
          <span className="dialog-header__icon" aria-hidden="true">
            <svg className="icon icon--sm" viewBox="0 0 24 24" focusable="false">
              <path d="M12 3.5 5 6.2v5.1c0 4.3 2.9 8.1 7 9.2 4.1-1.1 7-4.9 7-9.2V6.2z" />
            </svg>
          </span>
          <div className="dialog-header__text">
            <h2 id="migration-disclosure-title">Connected agent privacy</h2>
            <p>Your posture was set for you during the upgrade.</p>
          </div>
        </header>
        <div className="dialog-body">
          <p>
            You already paired a Cloud Bridge token before privacy modes were
            enforced. Your posture has been set to Connected agent to match
            that reality.
          </p>
          <p>{connected?.summary}</p>
          <p>
            You can change this at any time in{" "}
            <strong>Settings → Privacy</strong>.
          </p>
        </div>
        <footer className="dialog-footer">
          <button type="button" className="btn btn--primary" onClick={handleAcknowledge}>
            I understand
          </button>
        </footer>
      </div>
    </div>
  );
}
