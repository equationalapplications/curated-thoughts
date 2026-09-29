import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { safeUnlisten, guardListen } from "../../lib/events";
import { needsChunkHashMigration } from "../../lib/tauri";

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
    const unlistenProgress = guardListen(
      listen<MigrationProgressEvent>(
        "migration-progress",
        (event) => setProgress(event.payload),
      ),
      "migration-progress",
    );
    const unlistenComplete = guardListen(
      listen("migration-complete", () => {
        onComplete();
      }),
      "migration-complete",
    );
    // Fallback when the completion listener fails to REGISTER: the event can
    // never arrive, so the splash would sit here forever (issue #236
    // review). Poll the migration gate command instead — it flips to `false`
    // once the backend migration has done its work, which is exactly the
    // condition "migration-complete" fires under. A rejected gate query
    // keeps the poll running; the error event still drives the error UI.
    let pollTimer: ReturnType<typeof setInterval> | null = null;
    void unlistenComplete.catch((err) => {
      console.warn("[splash] migration-complete registration failed; polling gate", err);
      const poll = async () => {
        try {
          if (!(await needsChunkHashMigration())) {
            if (pollTimer) clearInterval(pollTimer);
            pollTimer = null;
            onComplete();
          }
        } catch {
          // gate query failed — keep polling; do not complete on a guess
        }
      };
      void poll();
      pollTimer = setInterval(poll, 2000);
    });
    const unlistenError = guardListen(
      listen<MigrationErrorEvent>(
        "migration-error",
        (event) => setError(event.payload.message),
      ),
      "migration-error",
    );
    return () => {
      void safeUnlisten(unlistenProgress);
      void safeUnlisten(unlistenComplete);
      void safeUnlisten(unlistenError);
      if (pollTimer) clearInterval(pollTimer);
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