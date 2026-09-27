import { useState } from 'react';
import {
  runWikiHeal,
  runWikiPrune,
  runWikiReembed,
  forgetWikiSource,
} from '../../lib/tauri';
import { useWikiStatus } from '../../hooks/useWikiStatus';
import { HealthReportPanel } from './HealthReportPanel';
import { DraftsPanel } from './DraftsPanel';

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
      <p className="maintenance-description">
        Engine diagnostics since launch: {wikiStatus.diagnosticErrors ?? 0} errors,{' '}
        {wikiStatus.diagnosticWarnings ?? 0} warnings (details in the app log).
      </p>
      <p className="maintenance-description" aria-live="polite">
        {wikiStatus.watcherHealth === 'degraded' ? (
          /* A drawn warning glyph, not "⚠": the emoji rendered at the
             platform's own size and weight, so the one line in the panel that
             most needed to stand out was the least controlled. */
          <span className="maintenance-warning">
            <svg className="icon icon--sm" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
              <path d="M12 8.5v4" />
              <path d="M12 16.2h.01" />
              <path d="M10.3 4.2 3.6 16.4A2 2 0 0 0 5.3 19.4h13.4a2 2 0 0 0 1.7-3L13.7 4.2a2 2 0 0 0-3.4 0z" />
            </svg>
            <span>
              Vault watcher degraded: a notify error was recorded or the watcher failed its
              liveness check, so file changes may be missed. Details in .brain/errors.log.
            </span>
          </span>
        ) : (
          'Vault watcher: no degradation reported at the last self-check.'
        )}
      </p>

      {/* One row per operation: name + consequence on the left, control on
          the right. See the .action-row note in index.css for what this
          replaced. */}
      <div className="action-list">
        <div className="action-row">
          <div className="action-row__text">
            <span className="action-row__title">Heal Database</span>
            <p className="action-row__desc">
              Removes ghost notes whose source file was deleted outside the app.
            </p>
          </div>
          <div className="action-row__control">
            <button type="button" disabled={isSystemBusy} onClick={() => runCommand('heal')}>
              Heal
            </button>
          </div>
        </div>

        <div className="action-row">
          <div className="action-row__text">
            <span className="action-row__title">Prune Trash</span>
            <p className="action-row__desc">
              Permanently deletes inferred entries soft-deleted more than 7 days ago.
              <strong> This cannot be undone.</strong> An automatic prune runs daily to keep
              inferred trash from growing unbounded.
            </p>
          </div>
          <div className="action-row__control">
            <button type="button" disabled={isSystemBusy} onClick={() => runCommand('prune')}>
              Prune now
            </button>
          </div>
        </div>

        <div className="action-row">
          <div className="action-row__text">
            <label className="action-row__title" htmlFor="forget-path">
              Forget source file
            </label>
            <p className="action-row__desc">
              Remove wiki memory entries for one vault source file. Takes a vault-relative
              path, typically under <code>documents/</code> or <code>wiki/</code>.
            </p>
          </div>
          <div className="action-row__control">
            <input
              id="forget-path"
              type="text"
              value={forgetPath}
              onChange={(e) => setForgetPath(e.target.value)}
              placeholder="vault-relative path"
              disabled={isSystemBusy}
            />
            <button
              type="button"
              className="btn btn--danger"
              disabled={isSystemBusy || !forgetPath.trim()}
              onClick={() => runCommand('forget')}
            >
              Forget
            </button>
          </div>
        </div>

        <div className="action-row">
          <div className="action-row__text">
            <span className="action-row__title">Full Re-index</span>
            <p className="action-row__desc">
              Re-chunks and re-embeds all tiers. Required after switching embedding models.
            </p>
          </div>
          <div className="action-row__control">
            <button type="button" disabled={isSystemBusy} onClick={() => runCommand('reembed')}>
              Re-index
            </button>
          </div>
        </div>
      </div>

      <DraftsPanel />
      <HealthReportPanel />
    </div>
  );
}
