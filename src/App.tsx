import { useState } from "react";
import { useSetupStatus } from "./hooks/useSetupStatus";
import { SetupWizard } from "./components/setup/SetupWizard";
import { AppShell } from "./components/shell/AppShell";

export function App() {
  const { loading, needsSetup, vaultPath } = useSetupStatus();
  const [setupComplete, setSetupComplete] = useState(false);
  const [resolvedVaultPath, setResolvedVaultPath] = useState<string | null>(null);

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
        onComplete={(path: string) => {
          setResolvedVaultPath(path);
          setSetupComplete(true);
        }}
      />
    );
  }

  const activePath = resolvedVaultPath ?? vaultPath!;
  return <AppShell vaultPath={activePath} />;
}

export default App;
