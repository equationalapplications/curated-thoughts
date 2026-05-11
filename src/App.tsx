import { useCallback, useState } from "react";
import { useSetupStatus } from "./hooks/useSetupStatus";
import { SetupWizard } from "./components/setup/SetupWizard";
import { AppShell } from "./components/shell/AppShell";

export function App() {
  const { loading, needsSetup, vaultPath } = useSetupStatus();
  const [setupComplete, setSetupComplete] = useState(false);
  const [currentVaultPath, setCurrentVaultPath] = useState<string | null>(null);

  const handleVaultChanged = useCallback((newPath: string) => {
    setCurrentVaultPath(newPath);
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
        onComplete={() => {
          setSetupComplete(true);
        }}
      />
    );
  }

  const activePath = currentVaultPath ?? vaultPath!;
  return (
    <AppShell vaultPath={activePath} onVaultChanged={handleVaultChanged} />
  );
}

export default App;
