import { useEffect, useState } from "react";
import { fetchChunkContent } from "../../lib/tauri";
import { applyInertGuard, useFocusTrap } from "../../a11y";

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

function PeekPanelBody({ target, onDismiss, onPromote }: BodyProps) {
  const [status, setStatus] = useState<BodyStatus>("loading");
  const [text, setText] = useState<string | null>(null);
  const panelRef = useFocusTrap<HTMLElement>({
    active: true,
    onEscape: onDismiss,
  });

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

  // Background guard: everything outside the dialog gets inert +
  // aria-hidden while the peek is open; released on unmount/close.
  // The backdrop button is exempt — it IS the click-outside dismiss
  // affordance. (Focus capture/restore is handled by useFocusTrap.)
  useEffect(() => {
    if (!panelRef.current) return;
    return applyInertGuard(panelRef.current, document.body, {
      allow: (el) => el.classList.contains("peek-backdrop"),
    });
  }, []);

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
