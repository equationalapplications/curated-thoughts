import { useState, useEffect } from "react";
import { getRelatedChunks, SearchResult } from "../lib/tauri";

export function useRelatedChunks(docPath: string | null): SearchResult[] {
  const [chunks, setChunks] = useState<SearchResult[]>([]);

  useEffect(() => {
    if (!docPath) {
      setChunks([]);
      return;
    }
    getRelatedChunks(docPath).then(setChunks).catch(() => setChunks([]));
  }, [docPath]);

  return chunks;
}
