import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { listVaultFiles, VaultFile } from "../lib/tauri";
import { safeUnlisten } from "../lib/events";

export function useVaultFiles(vaultPath: string) {
  const [files, setFiles] = useState<VaultFile[]>([]);

  const refresh = useCallback(() => {
    listVaultFiles().then(setFiles).catch(() => setFiles([]));
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = listen("vault-event", refresh);
    return () => {
      void safeUnlisten(unlisten);
    };
  }, [refresh, vaultPath]);

  return files;
}
