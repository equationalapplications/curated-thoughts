import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

interface Props {
  onComplete: () => void;
}

interface MigrationProgressEvent {
  current: number;
  total: number;
  phase: string;
}

interface MigrationErrorEvent {
  message: string;
}

export function SplashScreen({ onComplete }: Props) {
  const [progress, setProgress] = useState<{ current: number; total: number; phase: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unlistenProgress = listen<MigrationProgressEvent>(
      "migration-progress",
      (event) => setProgress(event.payload),
    );
    const unlistenComplete = listen("migration-complete", () => {
      onComplete();
    });
    const unlistenError = listen<MigrationErrorEvent>(
      "migration-error",
      (event) => setError(event.payload.message),
    );
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
      unlistenError.then((fn) => fn());
    };
  }, [onComplete]);

  if (error) {
    return (
      <div className="splash-screen splash-screen--error" role="alert">
        <h2>Migration failed</h2>
        <p>{error}</p>
        <p>The vault was left unchanged. Please restart to retry.</p>
        <button
          type="button"
          className="splash-screen__retry"
          onClick={() => window.location.reload()}
        >
          Restart to retry
        </button>
      </div>
    );
  }

  return (
    <div className="splash-screen" role="status">
      <h2>Optimizing your library…</h2>
      {progress && (
        <progress
          className="splash-screen__progress"
          value={progress.current}
          max={progress.total}
          aria-valuenow={progress.current}
          aria-valuemax={progress.total}
        />
      )}
      {progress && (
        <p className="splash-screen__phase">
          {progress.phase} — {progress.current} / {progress.total}
        </p>
      )}
    </div>
  );
}