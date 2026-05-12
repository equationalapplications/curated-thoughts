import { useState, useEffect, useRef } from "react";
import { searchVault, SearchResult } from "../lib/tauri";

const DEBOUNCE_MS = 300;

export function useSearch(vaultPath: string) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const vaultPathRef = useRef(vaultPath);
  vaultPathRef.current = vaultPath;

  useEffect(() => {
    setQuery("");
    setResults([]);
    setSearching(false);
  }, [vaultPath]);

  useEffect(() => {
    if (timer.current) clearTimeout(timer.current);
    if (!query.trim()) {
      setResults([]);
      return;
    }
    timer.current = setTimeout(async () => {
      const pathWhenScheduled = vaultPathRef.current;
      setSearching(true);
      try {
        const r = await searchVault(query);
        if (vaultPathRef.current !== pathWhenScheduled) return;
        setResults(r);
      } catch {
        if (vaultPathRef.current !== pathWhenScheduled) return;
        setResults([]);
      } finally {
        setSearching(false);
      }
    }, DEBOUNCE_MS);
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, [query, vaultPath]);

  return { query, setQuery, results, searching };
}
