import { message, open } from "@tauri-apps/plugin-dialog";
import { useMemo, useState } from "react";
import { useWikiStatus } from "../../hooks/useWikiStatus";
import {
  backupVaultDb,
  checkVaultBackup,
  revealVault,
  switchVault,
} from "../../lib/tauri";

interface Props {
  vaultPath: string;
}

function revealLabel(): string {
  if (typeof navigator === "undefined") return "Reveal in folder";
  const p = navigator.platform ?? "";
  if (/Win/i.test(p)) return "Reveal in Explorer";
  if (/Mac/i.test(p)) return "Reveal in Finder";
  return "Reveal in file manager";
}

export function VaultPanel({ vaultPath }: Props) {
  const [switching, setSwitching] = useState(false);
  const wikiStatus = useWikiStatus();
  const isSystemBusy = wikiStatus.ingesting || wikiStatus.librarian || wikiStatus.heal;

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

    let hasBackup: boolean;
    try {
      hasBackup = await checkVaultBackup(selected);
    } catch (e) {
      await message(String(e), {
        title: "Invalid vault path",
        kind: "error",
        okLabel: "OK",
      });
      return;
    }

    const backupChoice = await message(
      "Back up your current index before switching?\n\n" +
        `This saves your indexed data to ${backupHintPath} so it can be restored if you switch back.`,
      {
        title: "Switch vault",
        kind: "info",
        buttons: {
          yes: "Back up and continue",
          no: "Continue without backup",
          cancel: "Cancel",
        },
      },
    );
    // Tauri `message()` returns button *roles* ("Yes" / "No" / "Cancel"), not custom labels.
    if (backupChoice === "Cancel") return;

    setSwitching(true);
    try {
      if (backupChoice === "Yes") {
        await backupVaultDb();
      }

      let restore = false;
      if (hasBackup) {
        const r = await message(
          "Found a previous index backup for this vault. Restore it?\n\n" +
            "(Documents changed since the backup will be re-indexed.)",
          {
            title: "Restore backup?",
            kind: "info",
            buttons: {
              yes: "Restore backup",
              no: "Don't restore",
              cancel: "Cancel switch",
            },
          },
        );
        if (r === "Cancel") return;
        restore = r === "Yes";
      }

      await switchVault(selected, restore);
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
        <button type="button" onClick={handleChangeVault} disabled={switching || isSystemBusy}>
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
