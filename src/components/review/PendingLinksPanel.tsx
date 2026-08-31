import { useCallback, useEffect, useState } from "react";
import { approveLink, listPendingLinks, type PendingLink } from "../../lib/tauri";

/**
 * Symlinks under documents/ that resolve outside the vault and have not been
 * approved. Renders nothing when there are none, so a vault with no symlinks
 * never shows this (spec D3a).
 */
export function PendingLinksPanel() {
  const [pending, setPending] = useState<PendingLink[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const result = await listPendingLinks();
      // Defend against mocks or older backends that return null instead
      // of an empty array — `pending.length` would throw otherwise.
      setPending(result ?? []);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (!pending.length && !error) return null;

  return (
    <section className="pending-links">
      {error && <p role="alert">Could not check linked folders: {error}</p>}
      {pending.map((p) => (
        <div key={p.link} className="pending-link">
          <p>
            <code>{p.link}</code> points to <span className="pending-link-target">{p.target}</span>.
            Include it in your brain?
          </p>
          <button
            type="button"
            onClick={async () => {
              try {
                await approveLink(p.link);
                await refresh();
              } catch (e) {
                setError(e instanceof Error ? e.message : String(e));
              }
            }}
          >
            Include
          </button>
        </div>
      ))}
    </section>
  );
}
