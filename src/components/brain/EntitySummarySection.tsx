import { useEffect, useRef, useState } from "react";
import { useCreateBlockNote, SuggestionMenuController } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { updateEntitySummary } from "../../lib/tauri";
import { useTheme } from "../../lib/ThemeContext";
import { WikilinkText, getWikilinkResolverEntities } from "./WikilinkText";
import {
  EntityWikilinkSuggestion,
  filterEntitySuggestions,
  type EntityWikilinkSuggestionItem,
} from "./EntityWikilinkSuggestion";

const SUGGESTION_DEBOUNCE_MS = 50;

/** Pure synchronous filter against the cached entity list. Exported for tests. */
export function searchEntitiesByQuery(
  query: string,
): EntityWikilinkSuggestionItem[] {
  const entities = getWikilinkResolverEntities();
  return filterEntitySuggestions(entities, query).map((entity) => ({
    entity,
  }));
}

interface Props {
  entityId: string;
  summary: string;
  onChanged: () => void;
  onNavigateEntity: (name: string) => void;
}

export function EntitySummarySection({
  entityId,
  summary,
  onChanged,
  onNavigateEntity,
}: Props) {
  const editor = useCreateBlockNote();
  const { resolved: theme } = useTheme();
  const [editing, setEditing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function startEdit() {
    setError(null);
    const blocks = await editor.tryParseMarkdownToBlocks(summary);
    editor.replaceBlocks(editor.document, blocks);
    setEditing(true);
  }

  async function save() {
    setError(null);
    try {
      const markdown = await editor.blocksToMarkdownLossy(editor.document);
      await updateEntitySummary(entityId, markdown);
      setEditing(false);
      onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <section className="entity-summary">
      <div className="entity-section-header">
        <h3>Summary</h3>
        {editing ? (
          <div className="entity-summary-actions">
            <button type="button" onClick={save}>
              Save
            </button>
            <button type="button" onClick={() => setEditing(false)}>
              Cancel
            </button>
          </div>
        ) : (
          <button type="button" onClick={startEdit}>
            Edit summary
          </button>
        )}
      </div>
      {editing ? (
        <BlockNoteView editor={editor} editable theme={theme}>
          <EntityWikilinkSuggestionMenu editor={editor} />
        </BlockNoteView>
      ) : summary.trim() ? (
        <p className="entity-summary-prose">
          <WikilinkText text={summary} onNavigate={onNavigateEntity} />
        </p>
      ) : (
        <p className="placeholder">No summary yet.</p>
      )}
      {error && (
        <p className="entity-summary-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

interface EntityWikilinkSuggestionMenuProps {
  editor: ReturnType<typeof useCreateBlockNote>;
}

function EntityWikilinkSuggestionMenu({
  editor,
}: EntityWikilinkSuggestionMenuProps) {
  const pendingRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  function handleItemClick(item: EntityWikilinkSuggestionItem) {
    editor.insertInlineContent(`[[${item.entity.name}]] `);
  }

  function debouncedFetch(query: string): Promise<EntityWikilinkSuggestionItem[]> {
    return new Promise((resolve) => {
      if (pendingRef.current) clearTimeout(pendingRef.current);
      pendingRef.current = setTimeout(
        () => resolve(searchEntitiesByQuery(query)),
        SUGGESTION_DEBOUNCE_MS,
      );
    });
  }

  // Cleanup the timer on unmount so a stale resolution can't fire after the
  // editor is gone.
  useEffect(() => {
    return () => {
      if (pendingRef.current) clearTimeout(pendingRef.current);
    };
  }, []);

  return (
    <SuggestionMenuController<typeof debouncedFetch>
      triggerCharacter="[["
      getItems={debouncedFetch}
      suggestionMenuComponent={EntityWikilinkSuggestionMenuView}
      onItemClick={handleItemClick}
    />
  );
}

function EntityWikilinkSuggestionMenuView({
  items,
  onItemClick,
}: {
  items: EntityWikilinkSuggestionItem[];
  selectedIndex: number | undefined;
  onItemClick?: (item: EntityWikilinkSuggestionItem) => void;
}) {
  const entities = items.map((item) => item.entity);
  return (
    <EntityWikilinkSuggestion
      entities={entities}
      query=""
      onSelect={(entity) => {
        const item = items.find((i) => i.entity.id === entity.id);
        if (item && onItemClick) onItemClick(item);
      }}
    />
  );
}
