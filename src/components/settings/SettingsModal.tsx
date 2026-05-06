import { FolderRulesPanel } from "./FolderRulesPanel";

interface Props { onClose: () => void }

export function SettingsModal({ onClose }: Props) {
  return (
    <div className="review-overlay" onClick={onClose}>
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="review-header">
          <h2>Settings</h2>
          <button className="review-close" onClick={onClose}>✕</button>
        </div>
        <FolderRulesPanel />
      </div>
    </div>
  );
}
