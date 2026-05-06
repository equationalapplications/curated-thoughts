import type { VaultFile } from "../../lib/tauri";

interface Props {
  files: VaultFile[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}

export function FolderTree({ files, selectedPath, onSelect }: Props) {
  const docs = files.filter((f) => f.tier === "user_doc");
  const wiki = files.filter((f) => f.tier === "wiki");

  if (files.length === 0) {
    return <p className="placeholder">Drop documents into your vault folder to get started</p>;
  }

  return (
    <div className="folder-tree">
      {docs.length > 0 && (
        <section className="tree-section">
          <h4 className="tree-section-label">Documents</h4>
          {docs.map((f) => (
            <button
              key={f.path}
              className={`tree-file${selectedPath === f.path ? " tree-file--active" : ""}`}
              onClick={() => onSelect(f.path)}
            >
              {f.name}
            </button>
          ))}
        </section>
      )}
      {wiki.length > 0 && (
        <section className="tree-section">
          <h4 className="tree-section-label">Wiki</h4>
          {wiki.map((f) => (
            <button
              key={f.path}
              className={`tree-file${selectedPath === f.path ? " tree-file--active" : ""}`}
              onClick={() => onSelect(f.path)}
            >
              {f.name}
            </button>
          ))}
        </section>
      )}
    </div>
  );
}
