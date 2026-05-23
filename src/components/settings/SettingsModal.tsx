import { useMemo } from "react";
import { AgentIntegrationPanel } from "./AgentIntegrationPanel";
import { FolderRulesPanel } from "./FolderRulesPanel";
import { MaintenanceDashboard } from "./MaintenanceDashboard";
import { ModelPanel } from "./ModelPanel";
import { VaultPanel } from "./VaultPanel";

interface Props {
  onClose: () => void;
  vaultPath: string;
}

function defaultBrainDir(): string {
  // Mirror the Rust convention: $HOME/.brain
  const isWindows =
    typeof navigator !== "undefined" && /Win/i.test(navigator.platform ?? "");
  if (isWindows) {
    const home =
      (window as unknown as { env?: { USERPROFILE?: string } })?.env
        ?.USERPROFILE ?? "C:\\Users\\You";
    return `${home}\\.brain`;
  }
  return "~/.brain";
}

export function SettingsModal({ onClose, vaultPath }: Props) {
  const brainDir = useMemo(() => defaultBrainDir(), []);

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
        <hr className="settings-divider" />
        <AgentIntegrationPanel brainDir={brainDir} />
        <hr className="settings-divider" />
        <MaintenanceDashboard />
      </div>
    </div>
  );
}
