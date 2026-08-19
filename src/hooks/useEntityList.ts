import { useCallback, useEffect, useRef, useState } from "react";
import { listEntities, type EntitySummary, type EntitySort } from "../lib/tauri";
import { refreshWikilinkResolver } from "../components/brain/WikilinkText";

export function useEntityList(initialSort: EntitySort = "updated_desc") {
  const [entities, setEntities] = useState<EntitySummary[]>([]);
  const [sort, setSort] = useState<EntitySort>(initialSort);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // Bumped on every `refresh()`; a stale response from a previous sort
  // request must not overwrite the newer one.
  const requestGeneration = useRef(0);

  const refresh = useCallback(async () => {
    const myGeneration = ++requestGeneration.current;
    setLoading(true);
    try {
      const result = await listEntities(sort);
      if (requestGeneration.current !== myGeneration) return;
      setEntities(result);
      setError(null);
      // Refresh the WikilinkText resolver so newly created/imported entities
      // render as resolved in their `[[Name]]` chips immediately, rather
      // than waiting for the user to restart the app.
      void refreshWikilinkResolver();
    } catch (err) {
      if (requestGeneration.current !== myGeneration) return;
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (requestGeneration.current === myGeneration) {
        setLoading(false);
      }
    }
  }, [sort]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { entities, sort, setSort, error, loading, refresh };
}
