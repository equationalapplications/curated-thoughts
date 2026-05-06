import { IndexingStatus } from "./IndexingStatus";

interface Props { reviewCount: number }

export function Sidebar({ reviewCount }: Props) {
  return (
    <aside className="sidebar">
      <div className="search-bar">
        <input type="search" placeholder="Search your brain..." />
      </div>
      <IndexingStatus />
      <div className="folder-tree">
        <p className="placeholder">Documents will appear here</p>
      </div>
      {reviewCount > 0 && (
        <div className="review-badge">{reviewCount} pages ready to review</div>
      )}
    </aside>
  );
}
