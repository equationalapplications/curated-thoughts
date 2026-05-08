import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IndexingStatus } from "./IndexingStatus";
import { SearchResults } from "./SearchResults";
import { FolderTree } from "./FolderTree";
import { useSearch } from "../../hooks/useSearch";
import { useVaultFiles } from "../../hooks/useVaultFiles";
import { copyToVault } from "../../lib/tauri";

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
    let unlisten: (() => void) | undefined;

    getCurrentWindow()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "leave") {
          setDragging(false);
          return;
        }
        const { x, y } = payload.position;
        const rect = sidebarRef.current?.getBoundingClientRect();
        const overSidebar = !!rect && x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
        if (payload.type === "enter" || payload.type === "over") {
          setDragging(overSidebar);
        } else if (payload.type === "drop") {
          setDragging(false);
          if (!overSidebar) return;
          for (const src of payload.paths) {
            copyToVault(src).catch((e) =>
              console.error("copy_to_vault failed:", e)
            );
          }
        }
      })
      .then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
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
          <span>Drop to add to Documents</span>
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
