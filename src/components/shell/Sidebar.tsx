import { IndexingStatus } from "./IndexingStatus";
import { SearchResults } from "./SearchResults";
import { useSearch } from "../../hooks/useSearch";

interface Props {
  reviewCount: number;
  onDocSelect: (path: string) => void;
}

export function Sidebar({ reviewCount, onDocSelect }: Props) {
  const { query, setQuery, results, searching } = useSearch();

  return (
    <aside className="sidebar">
      <div className="search-bar">
        <input
          type="search"
          placeholder="Search your brain..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {searching && <span className="search-spinner" aria-label="Searching">↻</span>}
      </div>
      {results.length > 0 ? (
        <SearchResults results={results} onSelect={onDocSelect} />
      ) : (
        <>
          <IndexingStatus />
          <div className="folder-tree">
            <p className="placeholder">Documents will appear here</p>
          </div>
        </>
      )}
      {reviewCount > 0 && (
        <div className="review-badge">{reviewCount} pages ready to review</div>
      )}
    </aside>
  );
}
