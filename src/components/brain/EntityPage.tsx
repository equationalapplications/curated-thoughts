import { useCallback, useEffect, useState } from "react";
import {
  addEntityFact,
  archiveEntity,
  getEntity,
  type EntityDetail,
} from "../../lib/tauri";
import { EntitySummarySection } from "./EntitySummarySection";
import { FactCard } from "./FactCard";
import { WikilinkText } from "./WikilinkText";

function formatDay(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString();
}

interface Props {
  entityId: string | null;
  onNavigateEntity: (name: string) => void;
  onOpenSource: (path: string, chunkId?: string | null) => void;
  /** Option/Alt+click peek — forwarded to FactCard. Optional; absent = no peek. */
  onPeekSource?: (path: string, chunkId: string | null) => void;
  onEntityLoaded: (detail: EntityDetail) => void;
  /** Fired after any write so the sidebar can refresh counts. */
  onMutated: () => void;
  onArchived: () => void;
}

export function EntityPage({
  entityId,
  onNavigateEntity,
  onOpenSource,
  onPeekSource,
  onEntityLoaded,
  onMutated,
  onArchived,
}: Props) {
  const [detail, setDetail] = useState<EntityDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [newFact, setNewFact] = useState("");

  const load = useCallback(async () => {
    if (!entityId) {
      setDetail(null);
      return;
    }
    try {
      const loaded = await getEntity(entityId);
      setDetail(loaded);
      setError(null);
      if (loaded) onEntityLoaded(loaded);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [entityId, onEntityLoaded]);

  useEffect(() => {
    let cancelled = false;
    if (!entityId) {
      setDetail(null);
      return;
    }
    getEntity(entityId)
      .then((loaded) => {
        if (cancelled) return;
        setDetail(loaded);
        setError(null);
        if (loaded) onEntityLoaded(loaded);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [entityId, onEntityLoaded]);

  async function handleMutation(action: () => Promise<unknown>) {
    setError(null);
    try {
      await action();
      await load();
      onMutated();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function submitFact() {
    const body = newFact.trim();
    if (!body || !entityId) return;
    await handleMutation(() => addEntityFact(entityId, body));
    setNewFact("");
  }

  async function archiveThisEntity() {
    if (!entityId) return;
    setError(null);
    try {
      await archiveEntity(entityId);
      onArchived();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  if (!entityId) {
    // `.entity-page` is a reading column, so its no-selection state was a
    // 12.5px line pinned to the top-left of an 850px pane — the third
    // different answer to "nothing selected" in the app. `.empty-pane`
    // centres it and gives it an icon, like Review, Tasks and the editor.
    return (
      <main className="entity-page entity-page--empty">
        <div className="empty-pane">
          <span className="empty-pane__icon" aria-hidden="true">
            <svg className="icon" viewBox="0 0 24 24" focusable="false">
              <path d="M12 4.2a3.1 3.1 0 0 0-3.1 3.1v1.4a2.7 2.7 0 0 0-1.9 2.6v5.4A2.7 2.7 0 0 0 9.7 19.4h4.6a2.7 2.7 0 0 0 2.7-2.7v-5.4a2.7 2.7 0 0 0-1.9-2.6V7.3A3.1 3.1 0 0 0 12 4.2z" />
              <path d="M9.9 19.4v.9a2.1 2.1 0 0 0 4.2 0v-.9" />
            </svg>
          </span>
          <h2 className="empty-pane__title">No entity selected</h2>
          <p className="empty-pane__hint">
            Pick one from the sidebar, or create a new one to start writing
            facts about it.
          </p>
        </div>
      </main>
    );
  }
  if (!detail) {
    return (
      <main className="entity-page entity-page--empty">
        {error ? (
          <p className="entity-page-error" role="alert">
            {error}
          </p>
        ) : (
          <p className="placeholder">Loading…</p>
        )}
      </main>
    );
  }

  return (
    <main className="entity-page">
      <header className="entity-page-header">
        <h2>{detail.name}</h2>
        <span className="entity-type-chip">{detail.entity_type}</span>
        <span className="entity-page-meta">
          Created {formatDay(detail.created_at)} · Updated {formatDay(detail.updated_at)} ·{" "}
          {detail.facts.length} fact{detail.facts.length === 1 ? "" : "s"}
        </span>
        <button type="button" className="entity-archive-btn" onClick={archiveThisEntity}>
          Archive entity
        </button>
      </header>
      {error && (
        <p className="entity-page-error" role="alert">
          {error}
        </p>
      )}

      <EntitySummarySection
        entityId={detail.id}
        summary={detail.summary}
        onChanged={() => void handleMutation(async () => {})}
        onNavigateEntity={onNavigateEntity}
      />

      <section className="entity-facts">
        <div className="entity-section-header">
          <h3>Facts</h3>
        </div>
        <form
          className="entity-add-fact"
          onSubmit={(e) => {
            e.preventDefault();
            void submitFact();
          }}
        >
          <input
            placeholder="Add a fact..."
            value={newFact}
            onChange={(e) => setNewFact(e.target.value)}
          />
          <button type="submit">Add fact</button>
        </form>
        {detail.facts.length === 0 && (
          <p className="placeholder">No facts yet.</p>
        )}
        {detail.facts.map((fact) => (
          <FactCard
            key={fact.id}
            entityId={detail.id}
            fact={fact}
            onChanged={() => void handleMutation(async () => {})}
            onNavigateEntity={onNavigateEntity}
            onOpenSource={onOpenSource}
            onPeekSource={onPeekSource}
          />
        ))}
      </section>

      {detail.tasks.length > 0 && (
        <section className="entity-tasks">
          <h3>Open tasks</h3>
          <ul>
            {detail.tasks.map((task) => (
              <li key={task.id}>
                <WikilinkText text={task.description} onNavigate={onNavigateEntity} />
              </li>
            ))}
          </ul>
        </section>
      )}

      {detail.events.length > 0 && (
        <section className="entity-events">
          <h3>Recent activity</h3>
          <ul>
            {detail.events.map((event) => (
              <li key={event.id}>
                <span className="entity-event-date">
                  {new Date(event.created_at).toLocaleDateString()}
                </span>{" "}
                {event.summary}
              </li>
            ))}
          </ul>
        </section>
      )}
    </main>
  );
}
