import { useState, useEffect } from "react";
import { getRelatedChunks, getStructuralNeighbors, SearchResult } from "../lib/tauri";

export function useRelatedChunks(docPath: string | null): SearchResult[] {
  const [chunks, setChunks] = useState<SearchResult[]>([]);

  useEffect(() => {
    if (!docPath) {
      setChunks([]);
      return;
    }
    Promise.all([
      getRelatedChunks(docPath).catch((): SearchResult[] => []),
      getStructuralNeighbors(docPath).catch((): SearchResult[] => []),
    ]).then(([semantic, structural]) => {
      const seenPositions = new Set(semantic.map((r) => `${r.doc_path}:${r.chunk_position}`));
      const uniqueStructural = structural.filter(
        (r) => !seenPositions.has(`${r.doc_path}:${r.chunk_position}`)
      );
      setChunks([...semantic, ...uniqueStructural]);
    });
  }, [docPath]);

  return chunks;
}
