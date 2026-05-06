import type { SearchResult } from "../../lib/tauri";

interface Props {
  results: SearchResult[];
  onSelect: (path: string) => void;
}

export function SearchResults({ results, onSelect }: Props) {
  if (results.length === 0) return null;
  return (
    <div className="search-results">
      {results.map((r, i) => (
        <button
          key={i}
          className="search-result"
          onClick={() => onSelect(r.doc_path)}
        >
          <span className="search-result-path">
            {r.doc_path.split("/").at(-1)}
          </span>
          <span className="search-result-snippet">
            {r.chunk_text.slice(0, 120)}…
          </span>
        </button>
      ))}
    </div>
  );
}
