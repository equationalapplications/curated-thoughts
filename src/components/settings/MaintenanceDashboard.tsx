import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useWikiStatus } from '../../hooks/useWikiStatus';

export function MaintenanceDashboard() {
  const wikiStatus = useWikiStatus();
  const isSystemBusy = wikiStatus.ingesting || wikiStatus.librarian || wikiStatus.heal;
  const [lastError, setLastError] = useState<string | null>(null);

  async function runCommand(command: string) {
    setLastError(null);
    try {
      await invoke(command);
    } catch (err) {
      setLastError(String(err));
    }
  }

  return (
    <div className="maintenance-dashboard">
      <h3>Database Maintenance</h3>

      {lastError && (
        <p className="maintenance-error" role="alert">
          Maintenance failed: {lastError}
        </p>
      )}

      {isSystemBusy && (
        <p className="maintenance-busy" aria-live="polite">
          Database busy — please wait…
        </p>
      )}

      <div className="maintenance-actions">
        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('run_wiki_heal')}
        >
          Heal Database
        </button>
        <p className="maintenance-description">
          Removes ghost notes whose source file was deleted outside the app.
        </p>

        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('run_wiki_prune')}
        >
          Prune Trash
        </button>
        <p className="maintenance-description">
          Permanently deletes inferred entries soft-deleted more than 7 days ago.
          <strong> This cannot be undone.</strong>
        </p>

        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('run_wiki_reembed')}
        >
          Full Re-index
        </button>
        <p className="maintenance-description">
          Re-chunks and re-embeds all tiers. Required after switching embedding models.
        </p>
      </div>
    </div>
  );
}
