import { useEffect, useState } from "react";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { ReviewModal } from "../review/ReviewModal";
import { startFileWatcher } from "../../lib/tauri";
import { useReviewQueue } from "../../hooks/useReviewQueue";

interface Props { vaultPath: string }

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);
  const [showReview, setShowReview] = useState(false);
  const isWiki = selectedDoc?.includes("/wiki/") ?? false;
  const { queue, refresh } = useReviewQueue();

  useEffect(() => {
    startFileWatcher(vaultPath).catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-shell">
      <Sidebar
        reviewCount={queue.length}
        selectedDoc={selectedDoc}
        onDocSelect={setSelectedDoc}
        onReviewOpen={() => setShowReview(true)}
      />
      <EditorPane selectedDoc={selectedDoc} isWiki={isWiki} />
      <RelatedNotes selectedDoc={selectedDoc} />
      {showReview && (
        <ReviewModal
          queue={queue}
          vaultPath={vaultPath}
          onClose={() => setShowReview(false)}
          onAction={() => { refresh(); }}
        />
      )}
    </div>
  );
}
