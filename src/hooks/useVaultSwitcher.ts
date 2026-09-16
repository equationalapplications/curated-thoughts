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
      "Back up this vault's index and knowledge base before switching?\n\n" +
        `This saves the index AND the knowledge base — entities, facts, tasks, ` +
        `agent memories and manual edits — to ${backupHintPath}, so it can be ` +
        `brought back if you switch back.`,
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

    // The brain is per-vault (#213): switching without a backup permanently
    // destroys this vault's knowledge. Gate it explicitly — and do it here,
    // before the restore prompt and regardless of whether the target vault
    // has a backup, because a restore also overwrites the current brain with
    // no fresh backup of it.
    if (backupChoice === "No") {
      const confirmed = await message(
        "Continuing permanently deletes this vault's entire knowledge base: " +
          "every approved entity, fact, edge and task, all pending proposals, " +
          "and all agent memories and manual edits.\n\n" +
          "Re-indexing your documents will not bring back agent memories or " +
          "manual edits. This cannot be undone.",
        {
          title: "Destroy this vault's knowledge?",
          kind: "warning",
          buttons: {
            yes: "Switch without backup",
            no: "Go back",
            cancel: "Go back",
          },
        },
      );
      if (confirmed !== "Yes") return;
    }

    setSwitching(true);
    try {
      if (backupChoice === "Yes") {
        await backupVaultDb();
      }

      let restore = false;
      if (hasBackup) {
        const r = await message(
          "Found a previous backup for this vault. Restore it?\n\n" +
            "This brings back the knowledge base saved in that backup — not " +
            "just the document index.\n\n" +
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
