import { useCallback, useEffect, useState } from "react";
import { listEntities, type EntitySummary, type EntitySort } from "../lib/tauri";

export function useEntityList(initialSort: EntitySort = "updated_desc") {
  const [entities, setEntities] = useState<EntitySummary[]>([]);
  const [sort, setSort] = useState<EntitySort>(initialSort);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      setEntities(await listEntities(sort));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [sort]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { entities, sort, setSort, error, loading, refresh };
}
