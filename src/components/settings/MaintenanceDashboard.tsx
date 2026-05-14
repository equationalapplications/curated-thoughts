import { useState } from 'react';
import {
  runWikiHeal,
  runWikiPrune,
  runWikiReembed,
  forgetWikiSource,
} from '../../lib/tauri';
import { useWikiStatus } from '../../hooks/useWikiStatus';

export function MaintenanceDashboard() {
  const wikiStatus = useWikiStatus();
  const isSystemBusy = wikiStatus.busy;
  const statusLabel = wikiStatus.activeJobLabel ?? 'Idle';
  const [lastError, setLastError] = useState<string | null>(null);
  const [forgetPath, setForgetPath] = useState('');

  async function runCommand(command: 'heal' | 'prune' | 'reembed' | 'forget') {
    setLastError(null);
    try {
      if (command === 'heal') {
        await runWikiHeal();
      } else if (command === 'prune') {
        await runWikiPrune();
      } else if (command === 'forget') {
        await forgetWikiSource(forgetPath.trim());
        setForgetPath('');
      } else {
        await runWikiReembed();
      }
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

      <p className="maintenance-status" aria-live="polite">
        {isSystemBusy
          ? `Background job active: ${statusLabel}. Please wait…`
          : 'No active wiki jobs. Maintenance commands are available.'}
      </p>

      <div className="maintenance-actions">
        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('heal')}
        >
          Heal Database
        </button>
        <p className="maintenance-description">
          Removes ghost notes whose source file was deleted outside the app.
        </p>

        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('prune')}
        >
          Prune Trash
        </button>
        <p className="maintenance-description">
          Permanently deletes inferred entries soft-deleted more than 7 days ago.
          <strong> This cannot be undone.</strong>
        </p>
        <p className="maintenance-description">
          Automatic prune runs daily to keep inferred trash from growing unbounded.
        </p>

        <label htmlFor="forget-path" className="maintenance-label">
          Forget source file path
        </label>
        <input
          id="forget-path"
          type="text"
          value={forgetPath}
          onChange={(e) => setForgetPath(e.target.value)}
          placeholder="vault-relative path (typically under documents/ or wiki/)"
          disabled={isSystemBusy}
        />
        <button
          type="button"
          disabled={isSystemBusy || !forgetPath.trim()}
          onClick={() => runCommand('forget')}
        >
          Forget Source
        </button>
        <p className="maintenance-description">
          Remove all indexed chunks for a specific vault source file.
        </p>

        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('reembed')}
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
