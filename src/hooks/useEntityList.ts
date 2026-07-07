import { useCallback, useEffect, useState } from "react";
import { listEntities, type EntitySummary } from "../lib/tauri";

export function useEntityList() {
  const [entities, setEntities] = useState<EntitySummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      setEntities(await listEntities("updated_desc"));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { entities, error, loading, refresh };
}
