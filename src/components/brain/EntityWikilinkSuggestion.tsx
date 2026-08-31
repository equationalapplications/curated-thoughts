import type { EntitySummary } from "../../lib/tauri";

export interface EntityWikilinkSuggestionItem {
  entity: EntitySummary;
}

export interface EntityWikilinkSuggestionProps {
  entities: EntitySummary[];
  query: string;
  onSelect: (entity: EntitySummary) => void;
}

export function filterEntitySuggestions(
  entities: EntitySummary[],
  query: string,
): EntitySummary[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return entities;
  return entities.filter((e) => e.name.toLowerCase().startsWith(needle));
}

export function EntityWikilinkSuggestion({
  entities,
  query,
  onSelect,
}: EntityWikilinkSuggestionProps) {
  const matches = filterEntitySuggestions(entities, query);
  if (matches.length === 0) {
    return (
      <div className="entity-wikilink-suggestion entity-wikilink-suggestion--empty">
        No entities match.
      </div>
    );
  }
  return (
    <ul className="entity-wikilink-suggestion" role="listbox" aria-label="Entity suggestions">
      {matches.map((entity) => (
        <li key={entity.id}>
          <button
            type="button"
            className="entity-wikilink-suggestion-item"
            role="option"
            aria-selected={false}
            onClick={() => onSelect(entity)}
          >
            <span className="entity-wikilink-suggestion-name">{entity.name}</span>
            <span className="entity-wikilink-suggestion-type">{entity.entity_type}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
