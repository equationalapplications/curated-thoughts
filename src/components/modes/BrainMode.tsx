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
}

export function BrainMode({ vaultPath, selectedDoc, onDocSelect }: Props) {
  const { query, setQuery, results, searching } = useSearch(vaultPath);
  const files = useVaultFiles(vaultPath);
  const wikiFiles = files.filter((f) => f.tier === "wiki");
  const isWiki = isWikiDocPath(selectedDoc, vaultPath);

  return (
    <div className="mode-layout">
      <aside className="sidebar">
        <div className="search-bar">
          <input
            type="search"
            placeholder="Search your brain..."
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
            files={wikiFiles}
            selectedPath={selectedDoc}
            onSelect={onDocSelect}
          />
        )}
      </aside>
      <EditorPane selectedDoc={selectedDoc} isWiki={isWiki} />
      <RelatedNotes selectedDoc={selectedDoc} />
    </div>
  );
}
