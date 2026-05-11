import { open } from "@tauri-apps/plugin-dialog";
import { useMemo, useState } from "react";
import {
  backupVaultDb,
  checkVaultBackup,
  revealVault,
  switchVault,
} from "../../lib/tauri";

interface Props {
  vaultPath: string;
  onVaultChanged: (newPath: string) => void;
}

function revealLabel(): string {
  if (typeof navigator === "undefined") return "Reveal in folder";
  const p = navigator.platform ?? "";
  if (/Win/i.test(p)) return "Reveal in Explorer";
  if (/Mac/i.test(p)) return "Reveal in Finder";
  return "Reveal in file manager";
}

export function VaultPanel({ vaultPath, onVaultChanged }: Props) {
  const [switching, setSwitching] = useState(false);

  const backupHintPath = useMemo(() => {
    const sep = vaultPath.includes("\\") ? "\\" : "/";
    const root = vaultPath.replace(/[/\\]+$/, "");
    return `${root}${sep}.brain${sep}brain.db.bak`;
  }, [vaultPath]);

  async function handleChangeVault() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Choose a new vault folder",
    });
    if (typeof selected !== "string" || selected === vaultPath) return;

    const hasBackup = await checkVaultBackup(selected);

    const doBackup = window.confirm(
      "Back up your current index before switching?\n\n" +
        `This saves your indexed data to ${backupHintPath} so it can be restored if you switch back.`,
    );

    setSwitching(true);
    try {
      if (doBackup) {
        await backupVaultDb();
      }

      let restore = false;
      if (hasBackup) {
        restore = window.confirm(
          "Found a previous index for this vault. Restore it?\n\n" +
            "(Files changed since the backup will be re-indexed.)",
        );
      }

      await switchVault(selected, restore);
      onVaultChanged(selected);
    } catch (e) {
      window.alert("Failed to switch vault: " + String(e));
    } finally {
      setSwitching(false);
    }
  }

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
        <button type="button" onClick={handleChangeVault} disabled={switching}>
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
      <p className="vault-hint">
        Switching vaults closes the current document and re-indexes the new
        folder.
      </p>
    </div>
  );
}
