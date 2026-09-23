import { useCallback, useEffect, useState } from 'react';
import type { WikiFact } from '@equationalapplications/react-llm-wiki';
import { wiki, seededOntologyEntityIds } from '../../lib/wiki';
import { promoteDraft } from '../../lib/tauri';

const PAGE_SIZE = 20;

type TierDrafts = { entityId: string; facts: WikiFact[]; nextCursor: string | null };

/**
 * Draft entries per seeded tier (spec CT-REQ-DRAFT-01). Listing uses the
 * engine's read-only `listDrafts`; promotion goes through the Rust
 * `promote_draft_cmd` so the change reaches the outbox.
 *
 * Each action (load / loadMore / promote) tracks its own in-flight key so
 * rapid clicks can't issue duplicate requests and a slow promote doesn't
 * block an unrelated tier's Load more button.
 */
export function DraftsPanel() {
  const [tiers, setTiers] = useState<TierDrafts[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // pending actions: `{ entityId: { [actionId]: true } }`
  const [pending, setPending] = useState<Record<string, Record<string, boolean>>>({});

  const begin = (entityId: string, actionId: string) =>
    setPending((p) => ({ ...p, [entityId]: { ...(p[entityId] ?? {}), [actionId]: true } }));
  const end = (entityId: string, actionId: string) =>
    setPending((p) => {
      const cur = { ...(p[entityId] ?? {}) };
      delete cur[actionId];
      const next = { ...p, [entityId]: cur };
      if (Object.keys(cur).length === 0) delete next[entityId];
      return next;
    });

  const load = useCallback(async () => {
    const actionId = '__load__';
    setLoading(true);
    setError(null);
    try {
      const pages = await Promise.all(
        seededOntologyEntityIds().map(async (entityId) => {
          begin(entityId, actionId);
          try {
            const page = await wiki.listDrafts(entityId, { limit: PAGE_SIZE });
            return { entityId, facts: page.facts, nextCursor: page.nextCursor };
          } finally {
            end(entityId, actionId);
          }
        }),
      );
      setTiers(pages);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function loadMore(entityId: string, cursor: string) {
    const actionId = `more:${cursor}`;
    if (pending[entityId]?.[actionId]) return;
    setError(null);
    begin(entityId, actionId);
    try {
      const page = await wiki.listDrafts(entityId, { limit: PAGE_SIZE, cursor });
      setTiers((prev) =>
        prev.map((t) =>
          t.entityId === entityId
            ? { ...t, facts: [...t.facts, ...page.facts], nextCursor: page.nextCursor }
            : t,
        ),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      end(entityId, actionId);
    }
  }

  async function promote(entityId: string, entryId: string) {
    if (pending[entityId]?.[entryId]) return;
    setError(null);
    begin(entityId, entryId);
    try {
      await promoteDraft(entryId, entityId);
      setTiers((prev) =>
        prev.map((t) =>
          t.entityId === entityId ? { ...t, facts: t.facts.filter((f) => f.id !== entryId) } : t,
        ),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      end(entityId, entryId);
    }
  }

  const isPending = (entityId: string, actionId: string) => !!pending[entityId]?.[actionId];

  return (
    <section className="maintenance-drafts" aria-labelledby="drafts-heading">
      <h4 id="drafts-heading">Drafts</h4>
      <p className="maintenance-description">
        Draft facts are visible in search. Promoting marks one stable and records you as its reviewer.
      </p>
      {error && (
        <p className="maintenance-error" role="alert">
          Drafts: {error}
        </p>
      )}
      {loading && <p className="maintenance-description">Loading drafts…</p>}
      {tiers.map((t) => (
        <div key={t.entityId} className="maintenance-drafts-tier">
          <h5>
            {t.entityId} ({t.facts.length}
            {t.nextCursor ? '+' : ''})
          </h5>
          {t.facts.length === 0 && !t.nextCursor ? (
            <p className="maintenance-description">No drafts.</p>
          ) : (
            <ul>
              {t.facts.map((f) => {
                const promoting = isPending(t.entityId, f.id);
                return (
                  <li key={f.id}>
                    <span>{f.title}</span>{' '}
                    <button
                      type="button"
                      aria-label={`Promote ${f.title}`}
                      disabled={promoting}
                      onClick={() => void promote(t.entityId, f.id)}
                    >
                      {promoting ? 'Promoting…' : 'Promote'}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          {t.nextCursor && (
            <button
              type="button"
              aria-label={`Load more drafts for ${t.entityId}`}
              disabled={isPending(t.entityId, `more:${t.nextCursor}`)}
              onClick={() => void loadMore(t.entityId, t.nextCursor as string)}
            >
              {isPending(t.entityId, `more:${t.nextCursor}`) ? 'Loading…' : 'Load more'}
            </button>
          )}
        </div>
      ))}
    </section>
  );
}
