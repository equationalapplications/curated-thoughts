import { useEffect, useState } from "react";
import { getBrainDir } from "../../lib/tauri";
import { AgentIntegrationPanel } from "./AgentIntegrationPanel";
import { AppearancePanel } from "./AppearancePanel";
import { CloudBridgePanel } from "./CloudBridgePanel";
import { FolderRulesPanel } from "./FolderRulesPanel";
import { MaintenanceDashboard } from "./MaintenanceDashboard";
import { GenerationPanel } from "./GenerationPanel";
import { EmbeddingPanel } from "./EmbeddingPanel";
import { PrivacyPanel } from "./PrivacyPanel";
import { VaultPanel } from "./VaultPanel";

export type SettingsTab =
  | "vault"
  | "privacy"
  | "models"
  | "librarian"
  | "agents"
  | "maintenance"
  | "appearance";

const TABS: { id: SettingsTab; label: string }[] = [
  { id: "vault", label: "Vault" },
  { id: "privacy", label: "Privacy" },
  { id: "models", label: "Models" },
  { id: "librarian", label: "Librarian" },
  { id: "agents", label: "Agents" },
  { id: "maintenance", label: "Maintenance" },
  { id: "appearance", label: "Appearance" },
];

interface Props {
  vaultPath: string;
  initialTab?: SettingsTab;
}

export function SettingsScreen({ vaultPath, initialTab }: Props) {
  const [tab, setTab] = useState<SettingsTab>(initialTab ?? "vault");
  const [brainDir, setBrainDir] = useState<string | null>(null);
  const [brainDirError, setBrainDirError] = useState<string | null>(null);

  useEffect(() => {
    if (initialTab) setTab(initialTab);
  }, [initialTab]);

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
          console.error(
            "Failed to resolve brain directory for MCP snippet:",
            error,
          );
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
    <div className="settings-screen">
      <nav className="settings-nav" role="tablist" aria-label="Settings sections">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            className={`settings-nav-btn${
              tab === t.id ? " settings-nav-btn--active" : ""
            }`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>
      <div className="settings-content" role="tabpanel">
        {tab === "vault" && <VaultPanel vaultPath={vaultPath} />}
        {tab === "privacy" && <PrivacyPanel />}
        {tab === "models" && (
          <>
            <GenerationPanel />
            <EmbeddingPanel />
          </>
        )}
        {tab === "librarian" && <FolderRulesPanel />}
        {tab === "agents" && (
          <>
            <AgentIntegrationPanel
              brainDir={brainDir}
              brainDirError={brainDirError}
            />
            {/* Moves to the Privacy tab in phase 6. */}
            <CloudBridgePanel />
          </>
        )}
        {tab === "maintenance" && <MaintenanceDashboard />}
        {tab === "appearance" && <AppearancePanel />}
      </div>
    </div>
  );
}
