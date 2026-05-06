import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { listVaultFiles, VaultFile } from "../lib/tauri";

export function useVaultFiles() {
  const [files, setFiles] = useState<VaultFile[]>([]);

  const refresh = useCallback(() => {
    listVaultFiles().then(setFiles).catch(() => setFiles([]));
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = listen("vault-event", refresh);
    return () => { unlisten.then((fn) => fn()); };
  }, [refresh]);

  return files;
}
