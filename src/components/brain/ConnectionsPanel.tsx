import { useEffect, useState } from "react";
import { getEntityConnections, type EntityConnections } from "../../lib/tauri";
import { useProviderHealth } from "../../hooks/useProviderHealth";
import { ProviderNotice } from "../health/ProviderNotice";

interface Props {
  entityId: string | null;
  onSelectEntity: (entityId: string) => void;
}

export function ConnectionsPanel({ entityId, onSelectEntity }: Props) {
  const [connections, setConnections] = useState<EntityConnections | null>(null);
  const { generation, embedding } = useProviderHealth();

  useEffect(() => {
    let cancelled = false;
    if (!entityId) {
      setConnections(null);
      return;
    }
    // Don't fire the request when the embedder is unavailable — the
    // ProviderNotice is the only meaningful surface in that state.
    if (embedding === "error" || embedding === "unconfigured") {
      setConnections(null);
      return;
    }
    getEntityConnections(entityId)
      .then((loaded) => {
        if (!cancelled) setConnections(loaded);
      })
      .catch(() => {
        if (!cancelled) setConnections({ outgoing: [], backlinks: [] });
      });
    return () => {
      cancelled = true;
    };
  }, [entityId, embedding]);

  if (!entityId) return null;

  const byType =
    connections == null
      ? new Map<string, never[]>()
      : (() => {
          const m = new Map<string, typeof connections.outgoing>();
          for (const edge of connections.outgoing) {
            const list = m.get(edge.edge_type) ?? [];
            list.push(edge);
            m.set(edge.edge_type, list);
          }
          return m;
        })();

  return (
    <aside className="connections-panel" aria-label="Connections">
      <h3>Connections</h3>
      <ProviderNotice
        feature="similarity"
        embedding={embedding}
        generation={generation}
      />
      {connections === null ? null : (
        <>
          <section className="connections-section">
            <h4>Linked from</h4>
            {connections.backlinks.length === 0 ? (
              <p className="placeholder">No entities link here yet.</p>
            ) : (
              <ul>
                {connections.backlinks.map((backlink) => (
                  <li key={backlink.entity_id}>
                    <button
                      type="button"
                      className="connections-backlink"
                      onClick={() => onSelectEntity(backlink.entity_id)}
                    >
                      {backlink.name}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {[...byType.entries()].map(([type, edges]) => (
            <section key={type} className="connections-section">
              <h4>{type}</h4>
              <ul>
                {edges.map((edge) => (
                  <li key={edge.id} className="connections-edge">
                    {edge.source_label} → {edge.target_label}
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </>
      )}
    </aside>
  );
}
