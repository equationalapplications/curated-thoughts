import { useEffect, useRef, useState } from "react";
import { getStructuralNeighbors, searchVault, SearchResult } from "../lib/tauri";

const DEBOUNCE_MS = 300;
const MAX_CACHE_ENTRIES = 500;
const STRUCTURAL_HOPS = 1;

function getTierWeight(result: SearchResult): number {
  if (result.entity_id === "tier_fact") return 1.5;
  if (result.entity_id === "tier_wisdom") return 1.0;
  if (result.entity_id === "tier_working") return 0.6;
  // fallback: path heuristic for results without entity_id (e.g. structural neighbors)
  const normalized = result.doc_path.replace(/\\/g, "/").toLowerCase();
  if (normalized.includes("/documents/")) return 1.5;
  if (normalized.includes("/wiki/")) return 1.0;
  return 0.6;
}

function applyTierWeights(results: SearchResult[]): SearchResult[] {
  return results.map((result) => ({
    ...result,
    score: result.score * getTierWeight(result),
  }));
}

function makeCacheKey(query: string): string {
  return query.trim().toLowerCase();
}

export function useMemoryRead(vaultPath: string) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const latestRequestId = useRef(0);
  const cache = useRef<Map<string, SearchResult[]>>(new Map());
  const vaultPathRef = useRef(vaultPath);
  vaultPathRef.current = vaultPath;

  useEffect(() => {
    setQuery("");
    setResults([]);
    setSearching(false);
    cache.current.clear();
  }, [vaultPath]);

  useEffect(() => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }

    const trimmed = query.trim();
    if (!trimmed) {
      setResults([]);
      return;
    }

    const cacheKey = makeCacheKey(trimmed);
    const cached = cache.current.get(cacheKey);
    if (cached) {
      setResults(cached);
      return;
    }

    const requestId = ++latestRequestId.current;
    timer.current = setTimeout(async () => {
      setSearching(true);
      try {
        const semanticResults = await searchVault(trimmed, 10);
        if (latestRequestId.current !== requestId || vaultPathRef.current !== vaultPath) return;

        const weighted = applyTierWeights(semanticResults);
        const uniqueDocPaths = Array.from(new Set(weighted.map((r) => r.doc_path)));

        const structuralResults = (
          await Promise.allSettled(
            uniqueDocPaths.map((docPath) =>
              getStructuralNeighbors(docPath, STRUCTURAL_HOPS),
            ),
          )
        )
          .flatMap((result) => (result.status === 'fulfilled' ? result.value : [] as SearchResult[]));

        if (latestRequestId.current !== requestId || vaultPathRef.current !== vaultPath) return;

        const semanticKeyed = new Set(weighted.map((r) => `${r.doc_path}:${r.chunk_position}`));
        const uniqueStructural = structuralResults.filter(
          (r) => !semanticKeyed.has(`${r.doc_path}:${r.chunk_position}`),
        );

        const merged = [...weighted, ...uniqueStructural];
        cache.current.set(cacheKey, merged);
        if (cache.current.size > MAX_CACHE_ENTRIES) {
          const oldestKey = cache.current.keys().next().value;
          if (oldestKey !== undefined) {
            cache.current.delete(oldestKey);
          }
        }

        setResults(merged);
      } catch {
        if (latestRequestId.current !== requestId || vaultPathRef.current !== vaultPath) return;
        setResults([]);
      } finally {
        if (latestRequestId.current === requestId && vaultPathRef.current === vaultPath) {
          setSearching(false);
        }
      }
    }, DEBOUNCE_MS);

    return () => {
      if (timer.current) {
        clearTimeout(timer.current);
        timer.current = null;
      }
    };
  }, [query, vaultPath]);

  return { query, setQuery, results, searching };
}
