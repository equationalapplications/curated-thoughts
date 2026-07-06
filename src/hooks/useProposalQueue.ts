import { useState, useEffect, useCallback } from "react";
import { listProposals, type ProposalSummary } from "../lib/tauri";

const POLL_MS = 5000;

export function useProposalQueue(vaultPath: string) {
  const [queue, setQueue] = useState<ProposalSummary[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    listProposals({ status: "pending" })
      .then((next) => {
        setQueue(next);
        setError(null);
      })
      .catch(() => {
        setQueue([]);
        setError("Review queue is temporarily unavailable.");
      });
  }, []);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => clearInterval(id);
  }, [refresh, vaultPath]);

  return { queue, refresh, error };
}
