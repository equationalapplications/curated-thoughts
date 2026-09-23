import { useEffect, useState } from 'react';
import {
  getClassifierConfig,
  setClassifierConfig,
  type ClassifierProviderKind,
} from '../../lib/tauri';
import { usePrivacyMode } from '../../hooks/usePrivacyMode';

/**
 * Optional Jev classifier for "Type untyped facts" (spec CT-REQ-CLASS-01).
 * Saving emits `classifier-config-changed`, which rebuilds the wiki engine.
 *
 * The API token is stored in the OS keychain, never in `config.json` and
 * never returned by `getClassifierConfig`. The input is always blank on load
 * (we don't know the existing value). Typing a new value replaces the stored
 * token; the explicit "Clear stored token" button removes it.
 */
export function ClassifierPanel() {
  const { mode } = usePrivacyMode();
  const strict = mode === 'strict';
  const [provider, setProvider] = useState<ClassifierProviderKind>('unconfigured');
  const [url, setUrl] = useState('');
  const [accountId, setAccountId] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [minConfidence, setMinConfidence] = useState(0.5);
  const [timeoutSecs, setTimeoutSecs] = useState<number | null>(null);
  const [hasApiKey, setHasApiKey] = useState(false);
  const [status, setStatus] = useState<'idle' | 'loading' | 'saving' | 'saved'>('loading');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setStatus('loading');
    getClassifierConfig()
      .then((cfg) => {
        if (!active) return;
        setProvider(cfg.provider);
        setUrl(cfg.url ?? '');
        setAccountId(cfg.account_id ?? '');
        setApiKey('');
        setMinConfidence(cfg.min_confidence ?? 0.5);
        setTimeoutSecs(cfg.timeout_secs ?? null);
        setHasApiKey(cfg.has_api_key ?? false);
        setStatus('idle');
      })
      .catch((err) => {
        if (!active) return;
        setError(String(err));
        setStatus('idle');
      });
    return () => {
      active = false;
    };
  }, []);

  // Shared persist path for both save affordances. api_key semantics on
  // the wire:
  //   Some(newKey) -> store/replace in keychain
  //   Some("")     -> delete keychain entry
  //   null         -> leave keychain alone (user didn't touch the field)
  async function persistConfig(apiKeyPayload: string | null) {
    setStatus('saving');
    setError(null);
    try {
      await setClassifierConfig({
        provider,
        url: provider === 'jev_http' ? url.trim() : null,
        account_id: provider === 'cloudflare_jev' ? accountId.trim() : null,
        api_key: apiKeyPayload,
        min_confidence: minConfidence,
        timeout_secs: timeoutSecs,
      });
      // Optimistic local view: reflect what the keychain should now hold.
      setHasApiKey(Boolean(apiKeyPayload));
      setApiKey('');
      setStatus('saved');
    } catch (err) {
      setError(String(err));
      setStatus('idle');
    }
  }

  function save() {
    return persistConfig(apiKey.trim() || null);
  }

  function clearStoredKey() {
    // Some("") -> delete keychain entry.
    return persistConfig('');
  }

  const loading = status === 'loading';
  const saving = status === 'saving';
  const disableControls = strict || loading || saving;

  return (
    <div className="model-panel">
      <h3>Classifier (optional)</h3>
      <p className="settings-hint">
        A Jev classifier types facts faster and more cheaply than the generation model when you run
        “Type untyped facts” in Maintenance. It proposes no relationships; schema switches always
        use the generation model.
      </p>
      <p className="settings-hint">
        <strong>Privacy:</strong> fact titles and bodies are sent to the configured endpoint, one
        request per untyped fact.
      </p>
      {strict && (
        <p className="settings-hint">
          Strict privacy blocks the classifier. Switch to Ephemeral or Connected to use it.
        </p>
      )}
      {error && (
        <p className="settings-error" role="alert">
          {error}
        </p>
      )}
      <label htmlFor="classifier-provider">Classifier provider</label>
      <select
        id="classifier-provider"
        value={provider}
        disabled={disableControls}
        onChange={(e) => setProvider(e.target.value as ClassifierProviderKind)}
      >
        <option value="unconfigured">None</option>
        <option value="cloudflare_jev">Cloudflare Workers AI (typesafe/jev)</option>
        <option value="jev_http">Jev-compatible endpoint</option>
      </select>
      {provider === 'jev_http' && (
        <>
          <label htmlFor="classifier-url">Endpoint URL</label>
          <input id="classifier-url" type="url" value={url} disabled={disableControls} onChange={(e) => setUrl(e.target.value)} />
        </>
      )}
      {provider === 'cloudflare_jev' && (
        <>
          <label htmlFor="classifier-account">Cloudflare account ID</label>
          <input id="classifier-account" type="text" value={accountId} disabled={disableControls} onChange={(e) => setAccountId(e.target.value)} />
        </>
      )}
      {provider !== 'unconfigured' && (
        <>
          <label htmlFor="classifier-key">
            API token {hasApiKey && <span className="settings-hint">(a token is currently stored in the OS keychain)</span>}
          </label>
          <input
            id="classifier-key"
            type="password"
            value={apiKey}
            placeholder={hasApiKey ? 'Leave blank to keep the stored token' : 'Paste a token to store in the OS keychain'}
            disabled={disableControls}
            onChange={(e) => setApiKey(e.target.value)}
          />
          {hasApiKey && (
            <button
              type="button"
              className="settings-button-secondary"
              disabled={disableControls}
              onClick={() => void clearStoredKey()}
            >
              Clear stored token
            </button>
          )}
          <label htmlFor="classifier-min">Minimum confidence</label>
          <input
            id="classifier-min"
            type="number"
            min={0}
            max={1}
            step={0.05}
            value={minConfidence}
            disabled={disableControls}
            onChange={(e) => setMinConfidence(Number(e.target.value))}
          />
          <label htmlFor="classifier-timeout">Request timeout (seconds)</label>
          <input
            id="classifier-timeout"
            type="number"
            min={1}
            value={timeoutSecs ?? ''}
            placeholder="30"
            disabled={disableControls}
            onChange={(e) => {
              const raw = e.target.value.trim();
              setTimeoutSecs(raw === '' ? null : Math.max(1, Number(raw)));
            }}
          />
        </>
      )}
      <button type="button" disabled={disableControls} onClick={() => void save()}>
        {saving ? 'Saving…' : 'Save classifier'}
      </button>
      {status === 'saved' && <p className="settings-hint">Saved.</p>}
    </div>
  );
}
