import { useCallback, useState } from "react";
import { createEntity, type EntityDetail } from "../../lib/tauri";
import { EntityList } from "../brain/EntityList";
import { EntityPage } from "../brain/EntityPage";
import { ConnectionsPanel } from "../brain/ConnectionsPanel";
import { OkfInteropBar } from "../shell/OkfInteropBar";
import { ProviderNotice } from "../health/ProviderNotice";
import { useEntityList } from "../../hooks/useEntityList";
import { useProviderHealth } from "../../hooks/useProviderHealth";

interface Props {
  selectedEntityId: string | null;
  onEntitySelect: (id: string) => void;
  onOpenSource: (path: string, chunkId?: string | null) => void;
  onEntityName: (name: string) => void;
}

export function BrainMode({
  selectedEntityId,
  onEntitySelect,
  onOpenSource,
  onEntityName,
}: Props) {
  const { entities, error, loading, refresh, setSort } = useEntityList();
  const { generation, embedding } = useProviderHealth();
  const [entityError, setEntityError] = useState<string | null>(null);

  /**
   * Resolve a wikilink name (case-insensitive) to entity id.
   * Used by WikilinkText and EntityPage navigation.
   */
  const navigateByName = useCallback(
    (name: string): string | null => {
      const needle = name.trim().toLowerCase();
      const match = entities.find((e) => e.name.toLowerCase() === needle);
      return match?.id ?? null;
    },
    [entities],
  );

  /**
   * Called when EntityPage loads an entity detail.
   * Updates parent with the entity's canonical name.
   */
  const handleLoaded = useCallback(
    (detail: EntityDetail) => {
      onEntityName(detail.name);
    },
    [onEntityName],
  );

  /**
   * Create a new entity and refresh the list.
   */
  const handleCreate = useCallback(
    async (name: string) => {
      setEntityError(null);
      try {
        const detail = await createEntity({ name });
        await refresh();
        onEntitySelect(detail.id);
      } catch (err) {
        setEntityError(
          err instanceof Error ? err.message : String(err),
        );
      }
    },
    [refresh, onEntitySelect],
  );

  /**
   * Called when EntityPage archives an entity.
   * Refresh list, clear selection, and reset name display.
   */
  const handleArchived = useCallback(async () => {
    setEntityError(null);
    try {
      await refresh();
      onEntitySelect("");
      onEntityName("");
    } catch (err) {
      setEntityError(
        err instanceof Error ? err.message : String(err),
      );
    }
  }, [refresh, onEntitySelect, onEntityName]);

  /**
   * Refresh list when entity is mutated (facts added, etc).
   */
  const handleMutated = useCallback(async () => {
    try {
      await refresh();
    } catch {
      // Silent fail on refresh; EntityPage already shows error.
    }
  }, [refresh]);

  return (
    <div className="brain-mode">
      <ProviderNotice
        feature="related_notes"
        embedding={embedding}
        generation={generation}
      />
      <div className="mode-layout">
        <aside className="sidebar">
        {entityError && (
          <p className="entity-error" role="alert">
            {entityError}
          </p>
        )}
        {loading && entities.length === 0 && (
          <p className="placeholder">Loading entities…</p>
        )}
        {error && entities.length === 0 && (
          <p className="entity-error" role="alert">
            {error}
          </p>
        )}
        {entities.length > 0 && (
          <>
            {loading && (
              <p className="placeholder brain-loading-overlay" aria-live="polite">
                Refreshing entities…
              </p>
            )}
            <EntityList
              entities={entities}
              selectedId={selectedEntityId}
              onSelect={onEntitySelect}
              onCreate={handleCreate}
              onSortChange={setSort}
            />
            <OkfInteropBar onImported={refresh} />
          </>
        )}
      </aside>
      <EntityPage
        entityId={selectedEntityId}
        onNavigateEntity={(name) => {
          const id = navigateByName(name);
          if (id) onEntitySelect(id);
        }}
        onOpenSource={onOpenSource}
        onEntityLoaded={handleLoaded}
        onMutated={handleMutated}
        onArchived={handleArchived}
      />
      <ConnectionsPanel
        entityId={selectedEntityId}
        onSelectEntity={onEntitySelect}
      />
      </div>
    </div>
  );
}
