import { useEffect, useState } from "react";
import { getBrainDir } from "../../lib/tauri";
import { AgentIntegrationPanel } from "./AgentIntegrationPanel";
import { CloudBridgePanel } from "./CloudBridgePanel";
import { FolderRulesPanel } from "./FolderRulesPanel";
import { MaintenanceDashboard } from "./MaintenanceDashboard";
import { GenerationPanel } from "./GenerationPanel";
import { EmbeddingPanel } from "./EmbeddingPanel";
import { VaultPanel } from "./VaultPanel";

interface Props {
  onClose: () => void;
  vaultPath: string;
}

export function SettingsModal({ onClose, vaultPath }: Props) {
  const [brainDir, setBrainDir] = useState<string | null>(null);
  const [brainDirError, setBrainDirError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;

    getBrainDir()
      .then((dir) => {
        if (active) {
          setBrainDir(dir);
        }
      })
      .catch((error) => {
        if (active) {
          console.error("Failed to resolve brain directory for MCP snippet:", error);
          setBrainDirError(
            "Could not resolve the MCP brain directory. The config snippet is unavailable.",
          );
        }
      });

    return () => {
      active = false;
    };
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
        <GenerationPanel />
        <EmbeddingPanel />
        <hr className="settings-divider" />
        <FolderRulesPanel />
        <hr className="settings-divider" />
        <AgentIntegrationPanel brainDir={brainDir} brainDirError={brainDirError} />
        <hr className="settings-divider" />
        <CloudBridgePanel />
        <hr className="settings-divider" />
        <MaintenanceDashboard />
      </div>
    </div>
  );
}
