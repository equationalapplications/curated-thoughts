import { SearchResults } from "../shell/SearchResults";
import { FolderTree } from "../shell/FolderTree";
import { EditorPane } from "../shell/EditorPane";
import { RelatedNotes } from "../shell/RelatedNotes";
import { useSearch } from "../../hooks/useSearch";
import { useVaultFiles } from "../../hooks/useVaultFiles";
import { isWikiDocPath } from "../../lib/paths";

interface Props {
  vaultPath: string;
  selectedDoc: string | null;
  onDocSelect: (path: string) => void;
  /**
   * Optional chunk id within `selectedDoc` to scroll/highlight on load.
   * Driven by the active nav target's `chunkId` field in AppShell.
   */
  anchorChunkId?: string | null;
}

export function LibraryMode({
  vaultPath,
  selectedDoc,
  onDocSelect,
  anchorChunkId = null,
}: Props) {
  const { query, setQuery, results, searching } = useSearch(vaultPath);
  const files = useVaultFiles(vaultPath);
  const docFiles = files.filter((f) => f.tier === "user_doc");
  // Search results can span tiers; trust the path, not the mode.
  const isWiki = isWikiDocPath(selectedDoc, vaultPath);

  return (
    <div className="mode-layout">
      <aside className="sidebar">
        <div className="search-bar">
          <input
            type="search"
            placeholder="Search documents..."
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {searching && (
            <span className="search-spinner" aria-label="Searching">↻</span>
          )}
        </div>
        {results.length > 0 ? (
          <SearchResults results={results} onSelect={onDocSelect} />
        ) : (
          <FolderTree
            files={docFiles}
            selectedPath={selectedDoc}
            onSelect={onDocSelect}
          />
        )}
      </aside>
      <EditorPane
        selectedDoc={selectedDoc}
        isWiki={isWiki}
        anchorChunkId={anchorChunkId}
      />
      <RelatedNotes selectedDoc={selectedDoc} />
    </div>
  );
}
