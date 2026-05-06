import { useEffect, useState } from "react";
import { getIndexingStatus, IndexingStatus } from "../lib/tauri";

const POLL_MS = 2000;

export function useIndexingStatus(): IndexingStatus {
  const [status, setStatus] = useState<IndexingStatus>({ indexed: 0, pending: 0 });

  useEffect(() => {
    const tick = () => getIndexingStatus().then(setStatus).catch(() => {});
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => clearInterval(id);
  }, []);

  return status;
}
