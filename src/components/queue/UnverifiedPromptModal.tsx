import { useCallback, useEffect, useRef, useState } from "react";
import {
  confirmQueueFact,
  denyQueueFact,
  drainUnverifiedPrompts,
  getQueueFact,
  type QueueRow,
} from "../../lib/tauri";

/**
 * Active Verification Prompting (spec §3.4): when an unconfirmed fact matches
 * during an agent's recall, the agent appends a drop line; this modal drains
 * `<vault>/.brain/unverified-match.jsonl` (backend-capped at 3 per drain) and
 * offers a one-tap confirm.
 *
 * Anti-spoof M1 (the frontend M1 contract): the drop line's title/body/quote
 * payload is NEVER rendered. The modal re-reads each fact BY ID from the
 * brain and shows that row — a tampered payload can at worst trigger a
 * nuisance modal, never mislead.
 *
 * Volume bounds (spec §4): per-drain cap (backend), a "N more" digest, and a
 * snooze preference (localStorage, 7 days).
 */

const SNOOZE_KEY = "ct.unverifiedPromptSnoozeUntil";
const SNOOZE_MS = 7 * 24 * 60 * 60 * 1000;
const POLL_MS = 30_000;

export function snoozedUntil(): number {
  try {
    const raw = window.localStorage.getItem(SNOOZE_KEY);
    return raw ? Number(raw) || 0 : 0;
  } catch {
    return 0;
  }
}

export function setSnoozedUntil(ts: number): void {
  try {
    window.localStorage.setItem(SNOOZE_KEY, String(ts));
  } catch {
    // Storage unavailable (private mode): snooze just won't persist.
  }
}

interface Props {
  onAction?: () => void;
  pollMs?: number;
}

export function UnverifiedPromptModal({ onAction, pollMs = POLL_MS }: Props) {
  const [facts, setFacts] = useState<QueueRow[]>([]);
  const [moreCount, setMoreCount] = useState(0);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);

  const drain = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      if (Date.now() < snoozedUntil()) return;
      const report = await drainUnverifiedPrompts();
      const prompts = Array.isArray(report?.prompts) ? report!.prompts : [];
      if (prompts.length === 0) return;
      // Re-read every prompt's fact FROM THE BRAIN by id. A payload with a
      // poisoned title/body is ignored by construction: only `fact_id` is
      // used, and facts the brain no longer has are dropped silently.
      const rows: QueueRow[] = [];
      for (const p of prompts) {
        const row = await getQueueFact(p.fact_id);
        if (row) rows.push(row);
      }
      if (rows.length === 0) return;
      setFacts(rows);
      setMoreCount(report?.total_unacked ?? 0);
      setOpen(true);
    } catch {
      // Advisory: prompting must never break the app (spec §4 poisoned-file
      // bound — a bad drain degrades to "no prompts this tick").
    } finally {
      inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    void drain();
    const t = window.setInterval(() => void drain(), pollMs);
    return () => window.clearInterval(t);
  }, [drain, pollMs]);

  async function act(fn: (factId: string) => Promise<void>, factId: string) {
    setBusy(true);
    setError(null);
    try {
      await fn(factId);
      setFacts((prev) => {
        const next = prev.filter((f) => f.fact_id !== factId);
        if (next.length === 0) setOpen(false);
        return next;
      });
      onAction?.();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (!open || facts.length === 0) return null;

  return (
    <div
      className="queue-modal-backdrop queue-prompt-backdrop"
      role="presentation"
    >
      <div
        className="queue-modal queue-prompt-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Verify a fact you might want to confirm"
      >
        <h3 className="queue-modal-title">Worth verifying?</h3>
        <p className="queue-modal-hint">
          These unconfirmed facts just came up — confirm the ones you stand
          behind.
        </p>
        {error && (
          <p role="alert" className="queue-error">
            {error}
          </p>
        )}
        <ul className="queue-prompt-list">
          {facts.map((f) => (
            <li key={f.fact_id} className="queue-prompt-item">
              <h4>{f.title}</h4>
              <p>{f.body}</p>
              {f.evidence_quote && (
                <blockquote>“{f.evidence_quote}”</blockquote>
              )}
              <div className="queue-modal-actions">
                <button
                  className="queue-modal-confirm"
                  disabled={busy}
                  onClick={() => void act(confirmQueueFact, f.fact_id)}
                >
                  Confirm
                </button>
                <button
                  className="queue-modal-deny"
                  disabled={busy}
                  onClick={() => void act(denyQueueFact, f.fact_id)}
                >
                  Deny
                </button>
              </div>
            </li>
          ))}
        </ul>
        {moreCount > 0 && (
          <p className="queue-prompt-digest">
            …and {moreCount} more waiting in the confirmation queue.
          </p>
        )}
        <div className="queue-modal-actions">
          <button
            className="queue-modal-cancel"
            onClick={() => {
              setSnoozedUntil(Date.now() + SNOOZE_MS);
              setOpen(false);
            }}
          >
            Snooze for a week
          </button>
          <button
            className="queue-modal-cancel"
            onClick={() => setOpen(false)}
          >
            Not now
          </button>
        </div>
      </div>
    </div>
  );
}
