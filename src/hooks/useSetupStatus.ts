import { useEffect, useState } from "react";
import { getProviderConfig, getVaultPath, getBrainDir } from "../lib/tauri";

export interface SetupStatus {
  loading: boolean;
  needsSetup: boolean;
  vaultPath: string | null;
  providerConfigured: boolean;
}

export function useSetupStatus(): SetupStatus {
  const [loading, setLoading] = useState(true);
  const [vaultPath, setVaultPath] = useState<string | null>(null);
  const [providerConfigured, setProviderConfigured] = useState(false);

  useEffect(() => {
    Promise.all([
      getVaultPath(),
      getBrainDir()
        .then(getProviderConfig)
        .catch(() => null),
    ])
      .then(([path, config]) => {
        setVaultPath(path);
        setProviderConfigured(
          config !== null && config.generation.provider !== "unconfigured",
        );
      })
      .finally(() => setLoading(false));
  }, []);

  return {
    loading,
    needsSetup: !vaultPath || !providerConfigured,
    vaultPath,
    providerConfigured,
  };
}
