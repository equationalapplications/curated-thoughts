import { useState, useEffect } from "react";
import type { VaultFile } from "../../lib/tauri";
import { deleteVaultFile } from "../../lib/tauri";
import { getVaultLayout } from "../../lib/tauri";

interface Props {
  files: VaultFile[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}

function FileRow({
  file,
  isSelected,
  onSelect,
  deletable,
}: {
  file: VaultFile;
  isSelected: boolean;
  onSelect: () => void;
  deletable: boolean;
}) {
  const [confirming, setConfirming] = useState(false);

  function handleDelete(e: React.MouseEvent) {
    e.stopPropagation();
    if (!confirming) {
      setConfirming(true);
      return;
    }
    deleteVaultFile(file.path).catch((err) =>
      console.error("delete_vault_file failed:", err)
    );
    setConfirming(false);
  }

  return (
    <div className={`tree-file-row${isSelected ? " tree-file-row--active" : ""}`}>
      <button
        className={`tree-file${isSelected ? " tree-file--active" : ""}`}
        onClick={onSelect}
        title={file.name}
      >
        {file.name}
      </button>
      {deletable && (
        <button
          className={`tree-file-delete${confirming ? " tree-file-delete--confirm" : ""}`}
          onClick={handleDelete}
          onBlur={() => setConfirming(false)}
          title={confirming ? "Click again to confirm" : "Delete file"}
          aria-label={confirming ? "Confirm delete" : "Delete file"}
        >
          {confirming ? "✕" : "🗑"}
        </button>
      )}
    </div>
  );
}

export function FolderTree({ files, selectedPath, onSelect }: Props) {
  const [layout, setLayout] = useState<{
    immutableDir: string;
    wikiDir: string;
    labels: {
      immutableDir: string;
      wikiDir: string;
    };
  } | null>(null);

  // Load folder layout configuration on mount
  useEffect(() => {
    getVaultLayout()
      .then(setLayout)
      .catch((err) => {
        console.error("Failed to load vault layout:", err);
        // Fallback to hardcoded labels if fetch fails
        setLayout({
          immutableDir: "immutable-source-files",
          wikiDir: "wiki",
          labels: {
            immutableDir: "Source Files",
            wikiDir: "Wiki Pages",
          },
        });
      });
  }, []);

  const docs = files.filter((f) => f.tier === "user_doc");
  const wiki = files.filter((f) => f.tier === "wiki");

  const immutableLabel = layout?.labels.immutableDir ?? "Source Files";
  const wikiLabel = layout?.labels.wikiDir ?? "Wiki Pages";

  if (files.length === 0) {
    return <p className="placeholder">Drop documents into your vault folder to get started</p>;
  }

  return (
    <div className="folder-tree">
      {docs.length > 0 && (
        <section className="tree-section">
          <h4 className="tree-section-label">{immutableLabel}</h4>
          {docs.map((f) => (
            <FileRow
              key={f.path}
              file={f}
              isSelected={selectedPath === f.path}
              onSelect={() => onSelect(f.path)}
              deletable
            />
          ))}
        </section>
      )}
      {wiki.length > 0 && (
        <section className="tree-section">
          <h4 className="tree-section-label">{wikiLabel}</h4>
          {wiki.map((f) => (
            <FileRow
              key={f.path}
              file={f}
              isSelected={selectedPath === f.path}
              onSelect={() => onSelect(f.path)}
              deletable={false}
            />
          ))}
        </section>
      )}
    </div>
  );
}
