import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IndexingStatus } from "./IndexingStatus";
import { SearchResults } from "./SearchResults";
import { FolderTree } from "./FolderTree";
import { useSearch } from "../../hooks/useSearch";
import { useVaultFiles } from "../../hooks/useVaultFiles";

interface Props {
  vaultPath: string;
  reviewCount: number;
  selectedDoc: string | null;
  onDocSelect: (path: string) => void;
  onReviewOpen: () => void;
}

export function Sidebar({ vaultPath, reviewCount, selectedDoc, onDocSelect, onReviewOpen }: Props) {
  const { query, setQuery, results, searching } = useSearch();
  const files = useVaultFiles();
  const [dragging, setDragging] = useState(false);
  const sidebarRef = useRef<HTMLElement>(null);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    getCurrentWindow()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "leave") {
          setDragging(false);
          return;
        }
        if (payload.type === "enter" || payload.type === "over") {
          setDragging(true);
        } else if (payload.type === "drop") {
          setDragging(false);
        }
      })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [vaultPath]);

  return (
    <aside
      ref={sidebarRef}
      className={`sidebar${dragging ? " sidebar--drop-active" : ""}`}
    >
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
      {dragging && (
        <div className="drop-overlay">
          <span>Drop anywhere to add to Documents</span>
        </div>
      )}
      {reviewCount > 0 && (
        <button className="review-badge" onClick={onReviewOpen}>
          {reviewCount} page{reviewCount !== 1 ? "s" : ""} ready to review
        </button>
      )}
    </aside>
  );
}
