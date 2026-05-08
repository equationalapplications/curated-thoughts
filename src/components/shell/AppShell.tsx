import { useEffect, useState } from "react";
import { AppHeader } from "./AppHeader";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { ReviewModal } from "../review/ReviewModal";
import { SettingsModal } from "../settings/SettingsModal";
import { startFileWatcher } from "../../lib/tauri";
import { useReviewQueue } from "../../hooks/useReviewQueue";

interface Props { vaultPath: string }

/** Vault-relative paths from the file list use `wiki/...`; DB/search may still use absolute paths containing `/wiki/`. */
function isWikiDocPath(p: string | null | undefined): boolean {
  if (!p) return false;
  const norm = p.replace(/\\/g, "/");
  if (norm.startsWith("wiki/")) return true;
  return norm.includes("/wiki/");
}

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);
  const [showReview, setShowReview] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const isWiki = isWikiDocPath(selectedDoc);
  const { queue, refresh } = useReviewQueue();

  useEffect(() => {
    startFileWatcher().catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-root">
      <AppHeader onSettingsOpen={() => setShowSettings(true)} />
      <div className="app-shell">
        <Sidebar
          vaultPath={vaultPath}
          reviewCount={queue.length}
          selectedDoc={selectedDoc}
          onDocSelect={setSelectedDoc}
          onReviewOpen={() => setShowReview(true)}
        />
        <EditorPane selectedDoc={selectedDoc} isWiki={isWiki} />
        <RelatedNotes selectedDoc={selectedDoc} />
      </div>
      {showReview && (
        <ReviewModal
          queue={queue}
          onClose={() => setShowReview(false)}
          onAction={() => { refresh(); }}
        />
      )}
      {showSettings && <SettingsModal onClose={() => setShowSettings(false)} />}
    </div>
  );
}
