import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AgentIntegrationPanel } from "./AgentIntegrationPanel";
import { FolderRulesPanel } from "./FolderRulesPanel";
import { MaintenanceDashboard } from "./MaintenanceDashboard";
import { ModelPanel } from "./ModelPanel";
import { VaultPanel } from "./VaultPanel";

interface Props {
  onClose: () => void;
  vaultPath: string;
}

export function SettingsModal({ onClose, vaultPath }: Props) {
  const [brainDir, setBrainDir] = useState<string>("~/.brain");

  useEffect(() => {
    invoke<string>("get_brain_dir")
      .then(setBrainDir)
      .catch(() => {});
  }, []);

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
