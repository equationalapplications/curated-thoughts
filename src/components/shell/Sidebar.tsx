import { IndexingStatus } from "./IndexingStatus";
import { SearchResults } from "./SearchResults";
import { FolderTree } from "./FolderTree";
import { useSearch } from "../../hooks/useSearch";
import { useVaultFiles } from "../../hooks/useVaultFiles";

interface Props {
  reviewCount: number;
  selectedDoc: string | null;
  onDocSelect: (path: string) => void;
}

export function Sidebar({ reviewCount, selectedDoc, onDocSelect }: Props) {
  const { query, setQuery, results, searching } = useSearch();
  const files = useVaultFiles();

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
          <FolderTree files={files} selectedPath={selectedDoc} onSelect={onDocSelect} />
        </>
      )}
      {reviewCount > 0 && (
        <div className="review-badge">{reviewCount} pages ready to review</div>
      )}
    </aside>
  );
}
