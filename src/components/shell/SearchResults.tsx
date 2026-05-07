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
            {r.doc_path.split("/").at(-1)}:{r.start_line}
            {r.end_line !== r.start_line ? `–${r.end_line}` : ""}
          </span>
          <span className="search-result-meta" aria-label="chunk metadata">
            <span className="result-chip result-chip--strategy">{r.strategy}</span>
            <span className="result-chip result-chip--score">
              {(r.score * 100).toFixed(0)}% match
            </span>
            {r.symbol_name ? (
              <span className="result-chip result-chip--symbol" title={r.symbol_name}>
                {r.symbol_name}
              </span>
            ) : null}
          </span>
          <span className="search-result-snippet">
            {r.chunk_text.slice(0, 120)}…
          </span>
        </button>
      ))}
    </div>
  );
}
