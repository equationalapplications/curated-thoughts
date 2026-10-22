import { useCallback, useEffect, useState } from "react";
import {
  captureStatement,
  confirmQueueFact,
  denyQueueFact,
  listDenied,
  listQueue,
  unDenyQueueFact,
  type DenialRow,
  type QueueRow,
} from "../../lib/tauri";
import { ConfirmDenyModal } from "../queue/ConfirmDenyModal";

/**
 * The human confirmation queue (spec §3.2): live facts whose provenance is
 * not human-tier, oldest first, each with its evidence quote and an aging
 * indicator. Bulk select + bulk confirm; a fresh-statement capture box (Rule
 * 15 / spec §9: typed statements go through Path B, never a direct insert);
 * a denied section with undeny. Queue state is worded "unconfirmed" — never
 * "pending" (Rule 3 reserves pending for un-processed deposits).
 */
interface Props {
  onQueueChanged?: () => void;
}

function provenanceLabel(row: QueueRow): string {
  const base = row.provenance ?? "unclassified";
  return base === "immutable_document" ? "immutable document" : base.replaceAll("_", " ");
}

function ageLabel(days: number): string {
  if (days <= 0) return "today";
  if (days === 1) return "1 day old";
  return `${days} days old`;
}

export function QueueMode({ onQueueChanged }: Props) {
  const [rows, setRows] = useState<QueueRow[] | null>(null);
  const [denied, setDenied] = useState<DenialRow[]>([]);
  const [checked, setChecked] = useState<Set<string>>(() => new Set());
  const [actionRow, setActionRow] = useState<QueueRow | null>(null);
  const [captureText, setCaptureText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      // Null-safe: a backend that fails to answer leaves the queue empty
      // rather than crashing the screen.
      const [nextRows, nextDenied] = await Promise.all([
        listQueue(),
        listDenied(),
      ]);
      setRows(Array.isArray(nextRows) ? nextRows : []);
      setDenied(Array.isArray(nextDenied) ? nextDenied : []);
    } catch (err) {
      setRows([]);
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const afterAction = useCallback(() => {
    setChecked(() => new Set());
    void refresh();
    onQueueChanged?.();
  }, [refresh, onQueueChanged]);

  async function runBatch(fn: (factId: string) => Promise<void>) {
    if (checked.size === 0 || busy) return;
    setBusy(true);
    setError(null);
    let completed = 0;
    try {
      for (const factId of checked) {
        await fn(factId);
        completed += 1;
      }
      setNotice(`${completed} confirmed`);
      afterAction();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      // Facts confirmed before the throw ARE confirmed in the ledger —
      // refresh so the list, checked set and rail badge converge instead of
      // offering already-confirmed rows (confirm is an idempotent UPDATE per
      // spec §4, so the state was never wrong, just stale).
      afterAction();
    } finally {
      setBusy(false);
    }
  }

  async function handleCapture() {
    const text = captureText.trim();
    if (!text || busy) return;
    setBusy(true);
    setError(null);
    try {
      const rel = await captureStatement(text);
      setCaptureText("");
      setNotice(`Captured to ${rel}`);
      afterAction();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  const loading = rows === null;
  const queue = rows ?? [];

  return (
    <section className="queue-screen" aria-label="Confirmation queue">
      <header className="queue-header">
        <h2>Confirmation queue ({queue.length})</h2>
        <p className="queue-subtitle">
          Facts the librarian extracted that you have not confirmed. Confirming
          stamps the fact — it never asks you to retype it.
        </p>
      </header>

      <div className="queue-capture">
        <label className="queue-capture-label" htmlFor="queue-capture-input">
          State something new — it is captured as a stated file, attested, and
          the librarian picks it up.
        </label>
        <textarea
          id="queue-capture-input"
          value={captureText}
          onChange={(e) => setCaptureText(e.target.value)}
          rows={2}
          placeholder="Type a statement…"
        />
        <button
          className="queue-capture-btn"
          disabled={busy || captureText.trim() === ""}
          onClick={() => void handleCapture()}
        >
          Capture statement
        </button>
      </div>

      {error && (
        <p role="alert" className="queue-error">
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="queue-notice">
          {notice}
        </p>
      )}

      {checked.size > 0 && (
        <div className="queue-bulkbar">
          <span>{checked.size} selected</span>
          <button
            disabled={busy}
            onClick={() => void runBatch((id) => confirmQueueFact(id))}
          >
            Confirm selected
          </button>
        </div>
      )}

      {loading ? (
        <p className="queue-empty">Loading…</p>
      ) : queue.length === 0 ? (
        <p className="queue-empty">Queue clear — nothing waiting for you.</p>
      ) : (
        <ul className="queue-rows" aria-label="Unconfirmed facts">
          {queue.map((row) => (
            <li key={row.fact_id} className="queue-row">
              <label className="queue-row-check">
                <input
                  type="checkbox"
                  aria-label={`Select ${row.title}`}
                  checked={checked.has(row.fact_id)}
                  onChange={(e) => {
                    setChecked((prev) => {
                      const next = new Set(prev);
                      if (e.target.checked) {
                        next.add(row.fact_id);
                      } else {
                        next.delete(row.fact_id);
                      }
                      return next;
                    });
                  }}
                />
              </label>
              <div className="queue-row-main">
                <div className="queue-row-meta">
                  <span className="queue-row-provenance">
                    {provenanceLabel(row)}
                  </span>
                  <span className="queue-row-age">{ageLabel(row.age_days)}</span>
                  {row.attributed_speaker && (
                    <span className="queue-row-speaker">
                      via {row.attributed_speaker}
                    </span>
                  )}
                </div>
                <h3 className="queue-row-title">{row.title}</h3>
                <p className="queue-row-body">{row.body}</p>
                {row.evidence_quote && (
                  <blockquote className="queue-row-evidence">
                    “{row.evidence_quote}”
                    {row.source_document && (
                      <cite className="queue-row-source">
                        {" "}
                        — {row.source_document}
                      </cite>
                    )}
                  </blockquote>
                )}
              </div>
              <div className="queue-row-actions">
                <button
                  disabled={busy}
                  onClick={() => setActionRow(row)}
                >
                  Confirm / Deny
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      <section className="queue-denied" aria-label="Denied facts">
        <h3>Denied ({denied.length})</h3>
        {denied.length === 0 ? (
          <p className="queue-empty">Nothing denied.</p>
        ) : (
          <ul className="queue-denied-list">
            {denied.map((d) => (
              <li key={`${d.document_path}/${d.chunk_hash}/${d.fact_text_hash}`}>
                <span className="queue-denied-path">{d.document_path}</span>
                <button
                  disabled={busy}
                  onClick={() => {
                    void (async () => {
                      setBusy(true);
                      setError(null);
                      try {
                        // Send only the triple key (queue_denials' PK).
                        const { document_path, chunk_hash, fact_text_hash } = d;
                        await unDenyQueueFact({
                          document_path,
                          chunk_hash,
                          fact_text_hash,
                        });
                        afterAction();
                      } catch (err) {
                        setError(
                          err instanceof Error ? err.message : String(err),
                        );
                      } finally {
                        setBusy(false);
                      }
                    })();
                  }}
                >
                  Undeny
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {actionRow && (
        <ConfirmDenyModal
          row={actionRow}
          busy={busy}
          onClose={() => setActionRow(null)}
          onConfirm={async (factId) => {
            await confirmQueueFact(factId);
            setNotice("Confirmed");
            afterAction();
          }}
          onDeny={async (factId) => {
            await denyQueueFact(factId);
            setNotice("Denied — re-extractions of this fact are suppressed");
            afterAction();
          }}
          onError={setError}
        />
      )}
    </section>
  );
}
