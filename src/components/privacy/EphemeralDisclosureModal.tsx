import { acknowledgeEphemeralDisclosure } from "../../lib/tauri";

interface Props {
  onAcknowledged: () => void;
  onCancel: () => void;
}

const EPHEMERAL_OUTLINE =
  "The librarian sends your synthesis prompt plus retrieved document chunks to the configured external API. Chunks are quoted in context; nothing is stored on the remote service beyond the transient request.";

export function EphemeralDisclosureModal({ onAcknowledged, onCancel }: Props) {
  const handleAcknowledge = async () => {
    await acknowledgeEphemeralDisclosure();
    onAcknowledged();
  };

  return (
    <div className="privacy-modal-backdrop" role="presentation">
      <div
        className="dialog-surface"
        role="dialog"
        aria-modal="true"
        aria-labelledby="ephemeral-disclosure-title"
      >
        <header className="dialog-header">
          <span className="dialog-header__icon" aria-hidden="true">
            <svg className="icon icon--sm" viewBox="0 0 24 24" focusable="false">
              <path d="M12 3.5V10M8.2 6.2 12 3.5l3.8 2.7" />
              <path d="M5 12v6.5a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V12" />
            </svg>
          </span>
          <div className="dialog-header__text">
            <h2 id="ephemeral-disclosure-title">What leaves your machine</h2>
            <p>Read this before switching to Ephemeral cloud inference.</p>
          </div>
        </header>
        <div className="dialog-body">
          <p>{EPHEMERAL_OUTLINE}</p>
          <p>
            Embeddings and vault storage remain local. Only generation requests
            use the external API.
          </p>
        </div>
        {/* The confirming action is the only primary button in the dialog.
            Previously "Continue" and "Cancel" were two identical secondary
            buttons, so the acknowledgement read as dismissible. */}
        <footer className="dialog-footer">
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="btn btn--primary" onClick={handleAcknowledge}>
            Continue
          </button>
        </footer>
      </div>
    </div>
  );
}
