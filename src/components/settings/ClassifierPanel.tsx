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
 */
export function ClassifierPanel() {
  const { mode } = usePrivacyMode();
  const strict = mode === 'strict';
  const [provider, setProvider] = useState<ClassifierProviderKind>('unconfigured');
  const [url, setUrl] = useState('');
  const [accountId, setAccountId] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [minConfidence, setMinConfidence] = useState(0.5);
  const [status, setStatus] = useState<'idle' | 'saving' | 'saved'>('idle');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    getClassifierConfig()
      .then((cfg) => {
        if (!active) return;
        setProvider(cfg.provider);
        setUrl(cfg.url ?? '');
        setAccountId(cfg.account_id ?? '');
        setApiKey(cfg.api_key ?? '');
        setMinConfidence(cfg.min_confidence ?? 0.5);
      })
      .catch((err) => active && setError(String(err)));
    return () => {
      active = false;
    };
  }, []);

  async function save() {
    setStatus('saving');
    setError(null);
    try {
      await setClassifierConfig({
        provider,
        url: provider === 'jev_http' ? url.trim() : null,
        account_id: provider === 'cloudflare_jev' ? accountId.trim() : null,
        api_key: apiKey.trim() || null,
        min_confidence: minConfidence,
        timeout_secs: null,
      });
      setStatus('saved');
    } catch (err) {
      setError(String(err));
      setStatus('idle');
    }
  }

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
        disabled={strict}
        onChange={(e) => setProvider(e.target.value as ClassifierProviderKind)}
      >
        <option value="unconfigured">None</option>
        <option value="cloudflare_jev">Cloudflare Workers AI (typesafe/jev)</option>
        <option value="jev_http">Jev-compatible endpoint</option>
      </select>
      {provider === 'jev_http' && (
        <>
          <label htmlFor="classifier-url">Endpoint URL</label>
          <input id="classifier-url" type="url" value={url} disabled={strict} onChange={(e) => setUrl(e.target.value)} />
        </>
      )}
      {provider === 'cloudflare_jev' && (
        <>
          <label htmlFor="classifier-account">Cloudflare account ID</label>
          <input id="classifier-account" type="text" value={accountId} disabled={strict} onChange={(e) => setAccountId(e.target.value)} />
        </>
      )}
      {provider !== 'unconfigured' && (
        <>
          <label htmlFor="classifier-key">API token</label>
          <input id="classifier-key" type="password" value={apiKey} disabled={strict} onChange={(e) => setApiKey(e.target.value)} />
          <label htmlFor="classifier-min">Minimum confidence</label>
          <input
            id="classifier-min"
            type="number"
            min={0}
            max={1}
            step={0.05}
            value={minConfidence}
            disabled={strict}
            onChange={(e) => setMinConfidence(Number(e.target.value))}
          />
        </>
      )}
      <button type="button" disabled={strict || status === 'saving'} onClick={() => void save()}>
        Save classifier
      </button>
      {status === 'saved' && <p className="settings-hint">Saved.</p>}
    </div>
  );
}
