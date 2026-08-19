import { useMemo, useState } from "react";
import type { EntitySort, EntitySummary } from "../../lib/tauri";

interface Props {
  entities: EntitySummary[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onCreate: (name: string) => void;
  /** Fires when the user changes the sort dropdown, enabling parent to re-query. */
  onSortChange?: (sort: EntitySort) => void;
}

export function EntityList({ entities, selectedId, onSelect, onCreate, onSortChange }: Props) {
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<EntitySort>("updated_desc");
  const [creating, setCreating] = useState(false);
  const [draftName, setDraftName] = useState("");

  const groups = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const visible = needle
      ? entities.filter((e) => e.name.toLowerCase().includes(needle))
      : entities;
    const byType = new Map<string, EntitySummary[]>();
    for (const e of visible) {
      const list = byType.get(e.entity_type) ?? [];
      list.push(e);
      byType.set(e.entity_type, list);
    }
    return [...byType.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [entities, filter]);

  function submitCreate() {
    const name = draftName.trim();
    if (!name) return;
    onCreate(name);
    setDraftName("");
    setCreating(false);
  }

  return (
    <div className="entity-list">
      <div className="search-bar">
        <input
          type="search"
          placeholder="Filter entities..."
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </div>
      <select aria-label="Sort entities" value={sort} onChange={(e) => {
        const next = e.target.value as EntitySort;
        setSort(next);
        onSortChange?.(next);
      }}>
        <option value="updated_desc">Recently updated</option>
        <option value="name_asc">Name (A → Z)</option>
        <option value="name_desc">Name (Z → A)</option>
        <option value="created_desc">Recently created</option>
      </select>
      {creating ? (
        <form
          className="entity-create-form"
          onSubmit={(e) => {
            e.preventDefault();
            submitCreate();
          }}
        >
          <input
            autoFocus
            aria-label="New entity name"
            placeholder="Entity name"
            value={draftName}
            onChange={(e) => setDraftName(e.target.value)}
          />
          <button type="submit">Create</button>
          <button type="button" onClick={() => setCreating(false)}>
            Cancel
          </button>
        </form>
      ) : (
        <button
          type="button"
          className="entity-create-btn"
          onClick={() => setCreating(true)}
        >
          + New entity
        </button>
      )}
      {groups.length === 0 && (
        <p className="placeholder">
          No entities yet. Approve proposals in Review or create one.
        </p>
      )}
      {groups.map(([type, list]) => (
        <section key={type} className="entity-group">
          <h4 className="entity-group-heading">{type}</h4>
          <ul className="entity-group-items">
            {list.map((e) => (
              <li key={e.id}>
                <button
                  type="button"
                  className={
                    e.id === selectedId
                      ? "entity-row entity-row--selected"
                      : "entity-row"
                  }
                  onClick={() => onSelect(e.id)}
                >
                  <span className="entity-row-name">{e.name}</span>
                  <span className="entity-row-count">{e.fact_count}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
