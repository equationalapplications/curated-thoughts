import { revealVault } from "../../lib/tauri";
import { useVaultSwitcher } from "../../hooks/useVaultSwitcher";

interface Props {
  vaultPath: string;
  onRerunWizard?: () => void;
}

function revealLabel(): string {
  if (typeof navigator === "undefined") return "Reveal in folder";
  const p = navigator.platform ?? "";
  if (/Win/i.test(p)) return "Reveal in Explorer";
  if (/Mac/i.test(p)) return "Reveal in Finder";
  return "Reveal in file manager";
}

export function VaultPanel({ vaultPath, onRerunWizard }: Props) {
  const { changeVault, switching, isSystemBusy } = useVaultSwitcher(vaultPath);

  const folderName =
    vaultPath.split(/[/\\]/).filter(Boolean).pop() ?? vaultPath;

  return (
    <div className="settings-section">
      <h3>Vault</h3>
      <div className="vault-info">
        <span className="vault-path" title={vaultPath}>
          {folderName}
        </span>
        <span className="vault-full-path">{vaultPath}</span>
      </div>
      <div className="vault-actions">
        <button
          type="button"
          onClick={() => changeVault()}
          disabled={switching || isSystemBusy}
        >
          {switching ? "Switching…" : "Change vault…"}
        </button>
        <button
          type="button"
          onClick={() => revealVault().catch((e) => window.alert(String(e)))}
          className="vault-reveal-btn"
        >
          {revealLabel()}
        </button>
      </div>
      {onRerunWizard && (
        <button type="button" onClick={onRerunWizard} className="vault-rerun-wizard-btn">
          Re-run setup wizard
        </button>
      )}
      {isSystemBusy && (
        <p className="vault-hint vault-busy-hint">
          Background wiki maintenance is active. Wait for it to finish before
          switching vaults.
        </p>
      )}
      <p className="vault-hint">
        Switching vaults closes the current document and re-indexes the new
        folder.
      </p>
    </div>
  );
}
