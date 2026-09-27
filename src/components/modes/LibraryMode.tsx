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
        /* A drop target has to LOOK like one. The first-run state was a
           left-aligned sentence with a button under it on an unbounded
           field, so nothing on screen said "files can be dropped here" —
           the only cue was the word "Drop". It is now a centred dashed
           target matching the in-app .drop-overlay the OS drag produces, so
           the resting state and the active state are the same shape. */
        <div className="library-empty" role="region" aria-label="Library first-run empty state">
          <div className="drop-target">
            <span className="empty-pane__icon" aria-hidden="true">
              <svg className="icon" viewBox="0 0 24 24" focusable="false">
                <path d="M12 15.5V4.2M8.2 8 12 4.2 15.8 8" />
                <path d="M4.5 15v3.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15" />
              </svg>
            </span>
            <h2 className="empty-pane__title">Drop your first document</h2>
            <p className="empty-pane__hint">
              Notes, documents and source files are indexed in place — the
              files stay where they are, and nothing leaves your machine.
            </p>
            {onPickFile && (
              <div className="empty-pane__actions">
                <button type="button" className="btn btn--primary" onClick={onPickFile}>
                  Choose a folder
                </button>
              </div>
            )}
          </div>
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
                /* A drawn spinner. The "↻" character rotated at 12px read as a
                   stray mark inside the field — it had no stroke weight in
                   step with the rest of the chrome and scaled with whatever
                   font the input inherited. */
                <svg
                  className="icon icon--sm search-spinner"
                  viewBox="0 0 24 24"
                  role="img"
                  aria-label="Searching"
                  focusable="false"
                >
                  <path d="M20 12a8 8 0 1 1-2.6-5.9" />
                  <path d="M20.4 4.2v4.4H16" />
                </svg>
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
