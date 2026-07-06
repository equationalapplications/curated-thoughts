import { message, open } from "@tauri-apps/plugin-dialog";
import { useMemo, useState } from "react";
import { useWikiStatus } from "./useWikiStatus";
import {
  backupVaultDb,
  checkVaultBackup,
  switchVault,
} from "../lib/tauri";

export function useVaultSwitcher(vaultPath: string) {
  const [switching, setSwitching] = useState(false);
  const wikiStatus = useWikiStatus();

  const backupHintPath = useMemo(() => {
    const sep = vaultPath.includes("\\") ? "\\" : "/";
    const root = vaultPath.replace(/[/\\]+$/, "");
    return `${root}${sep}.brain${sep}brain.db.bak`;
  }, [vaultPath]);

  async function changeVault() {
    if (wikiStatus.busy) {
      await message(
        `Background wiki maintenance is active${
          wikiStatus.activeJobLabel ? `: ${wikiStatus.activeJobLabel}` : ""
        }. Wait for it to finish before switching vaults.`,
        { title: "Vault busy", kind: "warning", okLabel: "OK" },
      );
      return;
    }

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

  return { changeVault, switching, isSystemBusy: wikiStatus.busy };
}
