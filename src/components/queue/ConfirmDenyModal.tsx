import { useEffect, useRef } from "react";
import type { QueueRow } from "../../lib/tauri";

/**
 * Confirm / deny / deny-with-suppression modal for one queue fact (spec
 * §3.3). Denial errors are surfaced, never swallowed (Task 8 review note).
 */
interface Props {
  row: QueueRow;
  busy?: boolean;
  onClose: () => void;
  onConfirm: (factId: string) => Promise<void>;
  onDeny: (factId: string) => Promise<void>;
  onError: (message: string) => void;
}

export function ConfirmDenyModal({
  row,
  busy = false,
  onClose,
  onConfirm,
  onDeny,
  onError,
}: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function act(fn: (factId: string) => Promise<void>) {
    try {
      await fn(row.fact_id);
      onClose();
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div
      className="queue-modal-backdrop"
      role="presentation"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="queue-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Confirm or deny fact"
      >
        <h3 className="queue-modal-title">{row.title}</h3>
        <p className="queue-modal-body">{row.body}</p>
        {row.evidence_quote && (
          <blockquote className="queue-modal-evidence">
            “{row.evidence_quote}”
            {row.source_document && (
              <cite> — {row.source_document}</cite>
            )}
          </blockquote>
        )}
        <p className="queue-modal-hint">
          Confirming stamps this fact as verified by you. Denying archives it
          and suppresses re-extraction of the same statement.
        </p>
        <div className="queue-modal-actions">
          <button
            className="queue-modal-confirm"
            disabled={busy}
            onClick={() => void act(onConfirm)}
          >
            Confirm
          </button>
          <button
            className="queue-modal-deny"
            disabled={busy}
            onClick={() => void act(onDeny)}
          >
            Deny
          </button>
          <button className="queue-modal-cancel" onClick={onClose}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
