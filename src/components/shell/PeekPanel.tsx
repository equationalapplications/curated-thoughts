import { useEffect, useRef, useState } from "react";
import { fetchChunkContent } from "../../lib/tauri";

export type PeekTarget = { path: string; hash: string };

interface Props {
  target: PeekTarget | null;
  onDismiss: () => void;
  onPromote: (path: string, hash: string) => void;
}

/** Same shape, with `target` narrowed — the body only ever mounts open. */
interface BodyProps {
  target: PeekTarget;
  onDismiss: () => void;
  onPromote: (path: string, hash: string) => void;
}

type BodyStatus = "loading" | "ready" | "not-found" | "error";

function basename(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

function focusableWithin(root: HTMLElement): HTMLElement[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), textarea, input, select, [tabindex]:not([tabindex="-1"])',
    ),
  );
}

function PeekPanelBody({ target, onDismiss, onPromote }: BodyProps) {
  const [status, setStatus] = useState<BodyStatus>("loading");
  const [text, setText] = useState<string | null>(null);
  const panelRef = useRef<HTMLElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);

  // Body state: fetch the chunk slice for the current target.
  useEffect(() => {
    let cancelled = false;
    setStatus("loading");
    setText(null);
    fetchChunkContent(target.path, target.hash)
      .then((result) => {
        if (cancelled) return;
        if (result === null) {
          setStatus("not-found");
        } else {
          setText(result);
          setStatus("ready");
        }
      })
      .catch(() => {
        if (!cancelled) setStatus("error");
      });
    return () => {
      cancelled = true;
    };
  }, [target.path, target.hash]);

  // Focus: capture the opener before moving focus into the dialog; restore
  // it on unmount (guarded by isConnected — the opener may be gone by then).
  useEffect(() => {
    const active = document.activeElement;
    if (active instanceof HTMLElement) {
      openerRef.current = active;
    }
    panelRef.current
      ?.querySelector<HTMLElement>(".peek-open-btn")
      ?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener?.isConnected) {
        opener.focus();
      }
    };
  }, []);

  // One window keydown listener routes Esc and the focus trap.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        onDismiss();
        return;
      }
      if (e.key !== "Tab" || !panelRef.current) return;
      const focusables = focusableWithin(panelRef.current);
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement;
      const inside = active instanceof Node && panelRef.current.contains(active);
      if (e.shiftKey && (active === first || !inside)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (active === last || !inside)) {
        e.preventDefault();
        first.focus();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onDismiss]);

  return (
    <>
      <button
        type="button"
        className="peek-backdrop"
        aria-label="Close source peek"
        onClick={onDismiss}
      />
      <aside
        ref={panelRef}
        className="peek-panel"
        role="dialog"
        aria-modal="true"
        aria-label={`Source peek: ${basename(target.path)}`}
      >
        <header className="peek-panel-header">
          <h2 title={target.path}>{basename(target.path)}</h2>
          <button
            type="button"
            className="peek-open-btn"
            onClick={() => onPromote(target.path, target.hash)}
          >
            Open ↗
          </button>
        </header>
        <div className="peek-panel-body">
          {status === "loading" && <p className="placeholder">Loading…</p>}
          {status === "ready" && <div className="peek-chunk-text">{text}</div>}
          {status === "not-found" && (
            <div className="peek-notice" role="status">
              The source may have moved since this fact was created.
            </div>
          )}
          {status === "error" && (
            <div className="peek-notice peek-notice--error" role="alert">
              Could not load this passage.
            </div>
          )}
        </div>
      </aside>
    </>
  );
}

export function PeekPanel(props: Props) {
  if (props.target == null) return null;
  return <PeekPanelBody {...props} target={props.target} />;
}
