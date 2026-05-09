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

function isAbsolutePath(norm: string): boolean {
  return norm.startsWith("/") || /^[A-Za-z]:\//.test(norm);
}

/** Strip configured vault root from an absolute path; otherwise null. */
function vaultRelative(norm: string, vaultRoot: string): string | null {
  const n = norm.replace(/\\/g, "/");
  const root = vaultRoot.replace(/\\/g, "/").replace(/\/+$/, "");
  if (!root) return null;

  // Windows paths are case-insensitive; Unix paths are case-sensitive.
  const caseInsensitive = /^[A-Za-z]:\//.test(root);
  const lhs = n.slice(0, root.length);
  const rhs = root;
  const matchesPrefix = caseInsensitive
    ? lhs.toLowerCase() === rhs.toLowerCase()
    : lhs === rhs;

  if (n.length >= root.length && matchesPrefix) {
    if (n.length === root.length) return "";
    const sep = n[root.length];
    if (sep === "/" || sep === "\\") {
      return n.slice(root.length + 1);
    }
  }
  return null;
}

/**
 * Wiki docs live under the vault's top-level `wiki/` directory only.
 * Avoid `includes("/wiki/")` so `documents/wiki/...` is not treated as wiki.
 */
function isWikiDocPath(p: string | null | undefined, vaultRoot: string): boolean {
  if (!p) return false;
  const norm = p.replace(/\\/g, "/");
  if (!isAbsolutePath(norm)) {
    const first = norm.split("/").filter(Boolean)[0];
    return first === "wiki";
  }
  const rel = vaultRelative(norm, vaultRoot);
  if (rel === null || rel === "") return false;
  const first = rel.split("/").filter(Boolean)[0];
  return first === "wiki";
}

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);
  const [showReview, setShowReview] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const isWiki = isWikiDocPath(selectedDoc, vaultPath);
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
