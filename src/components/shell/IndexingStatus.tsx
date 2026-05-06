import { useIndexingStatus } from "../../hooks/useIndexingStatus";

export function IndexingStatus() {
  const { indexed, pending } = useIndexingStatus();

  if (pending > 0) {
    return (
      <div className="indexing-badge indexing-badge--busy">
        Indexing {pending} file{pending !== 1 ? "s" : ""}…
      </div>
    );
  }
  if (indexed === 0) return null;
  return (
    <div className="indexing-badge">
      {indexed} doc{indexed !== 1 ? "s" : ""} indexed
    </div>
  );
}
