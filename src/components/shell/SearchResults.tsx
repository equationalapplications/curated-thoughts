import type { SearchResult } from "../../lib/tauri";

interface Props {
  results: SearchResult[];
  onSelect: (path: string) => void;
}

function relTypeLabel(relType?: string): string {
  if (relType === 'CALLS')      return 'Calls this symbol';
  if (relType === 'IMPORTS')    return 'Imports this module';
  return 'Structurally linked';
}

function ResultItem({ r, onSelect }: { r: SearchResult; onSelect: (path: string) => void }) {
  return (
    <button
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
        {r.structural ? (
          <span
            className="result-chip result-chip--connected"
            title={relTypeLabel(r.rel_type)}
          >
            Connected
          </span>
        ) : null}
      </span>
      <span className="search-result-snippet">
        {r.chunk_text.slice(0, 120)}…
      </span>
    </button>
  );
}

export function SearchResults({ results, onSelect }: Props) {
  if (results.length === 0) return null;

  const semanticResults = results.filter((r) => !r.structural);
  const structuralResults = results.filter((r) => r.structural === true);

  return (
    <div className="search-results">
      {semanticResults.map((r) => (
        <ResultItem key={`${r.doc_path}:${r.chunk_position}`} r={r} onSelect={onSelect} />
      ))}
      {structuralResults.length > 0 && (
        <>
          <div className="search-results-divider">
            <span>Structural context</span>
          </div>
          {structuralResults.map((r) => (
            <ResultItem key={`structural:${r.doc_path}:${r.chunk_position}`} r={r} onSelect={onSelect} />
          ))}
        </>
      )}
    </div>
  );
}
