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
        type="button"
        className={`tree-file${isSelected ? " tree-file--active" : ""}`}
        onClick={onSelect}
        title={file.name}
      >
        <svg
          className="icon icon--sm tree-file-icon"
          viewBox="0 0 24 24"
          aria-hidden="true"
          focusable="false"
        >
          <path d="M6 4.5A1.5 1.5 0 0 1 7.5 3h5.2L18 8.3v11.2a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 19.5z" />
        </svg>
        {/* The name lives in a span because the button itself is a flex
            container (icon + label), and `text-overflow: ellipsis` does not
            apply to a flex container's own text — it only clips an inline
            child. With the bare text node, "Designing Data-Intensive
            Applications.md" wrapped onto a second line flush with the left
            edge of the row, which read as a broken tree. */}
        <span className="tree-file-name">{file.name}</span>
      </button>
      {deletable && (
        <button
          type="button"
          className={`tree-file-delete${confirming ? " tree-file-delete--confirm" : ""}`}
          onClick={handleDelete}
          onBlur={() => setConfirming(false)}
          title={confirming ? "Click again to confirm" : "Delete file"}
          aria-label={confirming ? "Confirm delete" : "Delete file"}
        >
          {confirming ? (
          <svg className="icon icon--sm" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
            <path d="M6 6l12 12M18 6 6 18" />
          </svg>
        ) : (
          <svg className="icon icon--sm" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
            <path d="M4.5 6.5h15M9.5 6.5V5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v1.5" />
            <path d="M6.5 6.5 7.4 19a1.5 1.5 0 0 0 1.5 1.4h6.2a1.5 1.5 0 0 0 1.5-1.4l.9-12.5" />
            <path d="M10.5 10v6.5M13.5 10v6.5" />
          </svg>
        )}
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
