import { FolderRulesPanel } from "./FolderRulesPanel";
import { ModelPanel } from "./ModelPanel";
import { VaultPanel } from "./VaultPanel";

interface Props {
  onClose: () => void;
  vaultPath: string;
}

export function SettingsModal({
  onClose,
  vaultPath,
}: Props) {
  return (
    <div className="review-overlay" onClick={onClose}>
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="review-header">
          <h2>Settings</h2>
          <button type="button" className="review-close" onClick={onClose}>
            ✕
          </button>
        </div>
        <VaultPanel vaultPath={vaultPath} />
        <hr className="settings-divider" />
        <ModelPanel />
        <hr className="settings-divider" />
        <FolderRulesPanel />
      </div>
    </div>
  );
}
