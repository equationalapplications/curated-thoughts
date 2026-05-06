import { useEffect, useState } from "react";
import { checkOllama, getVaultPath } from "../lib/tauri";

export interface SetupStatus {
  loading: boolean;
  needsSetup: boolean;
  vaultPath: string | null;
  ollamaReady: boolean;
}

export function useSetupStatus(): SetupStatus {
  const [loading, setLoading] = useState(true);
  const [vaultPath, setVaultPath] = useState<string | null>(null);
  const [ollamaReady, setOllamaReady] = useState(false);

  useEffect(() => {
    Promise.all([getVaultPath(), checkOllama()])
      .then(([path, status]) => {
        setVaultPath(path);
        setOllamaReady(status.installed && status.running);
      })
      .finally(() => setLoading(false));
  }, []);

  return {
    loading,
    needsSetup: !vaultPath || !ollamaReady,
    vaultPath,
    ollamaReady,
  };
}
