import { useEffect, useState } from "react";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { startFileWatcher } from "../../lib/tauri";

interface Props { vaultPath: string }

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);
  const isWiki = selectedDoc?.includes("/wiki/") ?? false;

  useEffect(() => {
    startFileWatcher(vaultPath).catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-shell">
      <Sidebar reviewCount={0} selectedDoc={selectedDoc} onDocSelect={setSelectedDoc} />
      <EditorPane selectedDoc={selectedDoc} isWiki={isWiki} />
      <RelatedNotes selectedDoc={selectedDoc} />
    </div>
  );
}
