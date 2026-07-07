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
        className="privacy-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="ephemeral-disclosure-title"
      >
        <h2 id="ephemeral-disclosure-title">What leaves your machine</h2>
        <p className="settings-hint">{EPHEMERAL_OUTLINE}</p>
        <p className="settings-hint">
          Embeddings and vault storage remain local. Only generation requests use
          the external API.
        </p>
        <div className="privacy-modal-actions">
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
          <button type="button" onClick={handleAcknowledge}>
            Continue
          </button>
        </div>
      </div>
    </div>
  );
}
