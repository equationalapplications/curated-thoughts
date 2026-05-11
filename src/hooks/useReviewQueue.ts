import { useState, useEffect, useCallback } from "react";
import { getReviewQueue, ReviewPage } from "../lib/tauri";

const POLL_MS = 5000;

export function useReviewQueue(vaultPath: string) {
  const [queue, setQueue] = useState<ReviewPage[]>([]);

  const refresh = useCallback(() => {
    getReviewQueue().then(setQueue).catch(() => {});
  }, []);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => clearInterval(id);
  }, [refresh, vaultPath]);

  return { queue, refresh };
}
