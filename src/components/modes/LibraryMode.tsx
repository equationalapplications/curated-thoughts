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
  onPickFile?: () => void;
}

export function LibraryMode({
  vaultPath,
  selectedDoc,
  onDocSelect,
  anchorChunkId = null,
  onPickFile,
}: Props) {
  const { query, setQuery, results, searching } = useSearch(vaultPath);
  const files = useVaultFiles(vaultPath);
  const docFiles = files.filter((f) => f.tier === "user_doc");
  // Search results can span tiers; trust the path, not the mode.
  const isWiki = isWikiDocPath(selectedDoc, vaultPath);

  const isFirstRun = docFiles.length === 0 && selectedDoc === null && !query;

  return (
    <div className="mode-layout">
      {isFirstRun ? (
        <div className="library-empty" role="region" aria-label="Library first-run empty state">
          <p className="placeholder">Drop your first document to get started.</p>
          {onPickFile && (
            <button type="button" onClick={onPickFile}>Choose a folder</button>
          )}
        </div>
      ) : (
        <>
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
            {query ? (
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
        </>
      )}
    </div>
  );
}
