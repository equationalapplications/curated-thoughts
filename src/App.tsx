import { useCallback, useEffect, useState } from "react";
import { useSetupStatus } from "./hooks/useSetupStatus";
import { AppShell } from "./components/shell/AppShell";
import { initWorkspaceId, startAutoHeal, startAutoMaintenance } from "./lib/wiki";

export function App() {
  const { loading, needsSetup, vaultPath } = useSetupStatus();
  const [currentVaultPath, setCurrentVaultPath] = useState<string | null>(null);

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
    if (import.meta.env.DEV) {
      import('./lib/searchProfiling')
        .then(({ profileSearchLatency, logSearchProfile }) => {
          interface SearchProfilingWindow extends Window {
            __searchProfiling?: {
              profileSearchLatency: typeof profileSearchLatency;
              logSearchProfile: typeof logSearchProfile;
            };
          }

          (window as SearchProfilingWindow).__searchProfiling = {
            profileSearchLatency,
            logSearchProfile,
          };
          console.info(
            '[searchProfiling] dev helper available on window.__searchProfiling',
          );
        })
        .catch((err) => {
          console.warn('[searchProfiling] failed to expose dev helper', err);
        });
    }

    const cleanupHeal = startAutoHeal();
    const cleanupMaintenance = startAutoMaintenance();
    // startAutoHeal registers Tauri event listeners and returns an unsubscribe function.
    return () => {
      cleanupHeal();
      cleanupMaintenance();
    };
  }, []);

  if (loading) {
    return (
      <div className="loading-screen">
        <p>Loading...</p>
      </div>
    );
  }

  if (!activePath) {
    return (
      <div className="loading-screen">
        <p>Could not determine your vault folder.</p>
        <button type="button" onClick={() => window.location.reload()}>
          Reload
        </button>
      </div>
    );
  }

  return (
    <AppShell
      vaultPath={activePath}
      onVaultChanged={handleVaultChanged}
      needsSetup={needsSetup}
    />
  );
}

export default App;