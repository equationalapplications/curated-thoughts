import { useCallback, useEffect, useState } from "react";
import { useSetupStatus } from "./hooks/useSetupStatus";
import { SetupWizard } from "./components/setup/SetupWizard";
import { AppShell } from "./components/shell/AppShell";
import { getVaultPath } from "./lib/tauri";
import { initWorkspaceId, startAutoHeal } from "./lib/wiki";

export function App() {
  const { loading, needsSetup, vaultPath } = useSetupStatus();
  const [setupComplete, setSetupComplete] = useState(false);
  const [currentVaultPath, setCurrentVaultPath] = useState<string | null>(null);
  const [vaultLoadError, setVaultLoadError] = useState<string | null>(null);

  const handleVaultChanged = useCallback((newPath: string) => {
    setCurrentVaultPath(newPath);
  }, []);

  const activePath = currentVaultPath ?? vaultPath;

  useEffect(() => {
    if (!activePath) return;
    initWorkspaceId(activePath).catch((err) =>
      console.error('[wiki] initWorkspaceId failed:', err)
    );
  }, [activePath]);

  useEffect(() => {
    const cleanup = startAutoHeal();
    // startAutoHeal registers Tauri event listeners and returns an unsubscribe function.
    return cleanup;
  }, []);

  if (loading) {
    return (
      <div className="loading-screen">
        <p>Loading...</p>
      </div>
    );
  }

  if (needsSetup && !setupComplete) {
    return (
      <SetupWizard
        onComplete={async () => {
          setVaultLoadError(null);
          try {
            const p = await getVaultPath();
            if (p) setCurrentVaultPath(p);
            else
              setVaultLoadError(
                "Vault path is still unavailable after setup. Try reloading.",
              );
          } catch (e) {
            setVaultLoadError(String(e));
          } finally {
            setSetupComplete(true);
          }
        }}
      />
    );
  }

  if (!activePath) {
    return (
      <div className="loading-screen">
        <p>Could not determine your vault folder.</p>
        {vaultLoadError ? <p>{vaultLoadError}</p> : null}
        <button type="button" onClick={() => window.location.reload()}>
          Reload
        </button>
      </div>
    );
  }

  return (
    <AppShell vaultPath={activePath} onVaultChanged={handleVaultChanged} />
  );
}

export default App;
