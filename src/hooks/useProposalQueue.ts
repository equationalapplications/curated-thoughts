import { useState, useEffect, useCallback } from "react";
import { listProposals, type ProposalSummary } from "../lib/tauri";

const POLL_MS = 5000;

export function useProposalQueue(vaultPath: string) {
  const [queue, setQueue] = useState<ProposalSummary[]>([]);

  const refresh = useCallback(() => {
    listProposals({ status: "pending" })
      .then(setQueue)
      .catch(() => {});
  }, []);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => clearInterval(id);
  }, [refresh, vaultPath]);

  return { queue, refresh };
}
