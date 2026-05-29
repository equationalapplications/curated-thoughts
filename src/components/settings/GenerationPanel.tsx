import { useEffect, useState } from "react";
import { getProviderConfig, updateProvider } from "../../lib/tauri";
import { onProviderLoading, onProviderReady, onProviderError } from "../../lib/events";
import type { GenerationConfig } from "../../lib/tauri";

type ProviderStatus = "loading" | "ready" | "unconfigured" | "error";

export function GenerationPanel() {
  const [config, setConfig] = useState<GenerationConfig | null>(null);
  const [status, setStatus] = useState<ProviderStatus>("loading");
  const [externalUrl, setExternalUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [modelName, setModelName] = useState("");
  const [savePhase, setSavePhase] = useState<"idle" | "saving" | "error">("idle");
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let unlistens: Array<() => void> = [];

    const setup = async () => {
      const cfg = await getProviderConfig().catch(() => null);
      if (!active) return;
      if (cfg) {
        setConfig(cfg.generation);
        setExternalUrl(cfg.generation.external_url ?? "");
        setApiKey(cfg.generation.api_key ?? "");
        setModelName(cfg.generation.model_name ?? "");
        setStatus(cfg.generation.provider === "unconfigured" ? "unconfigured" : "ready");
      }

      const [loadingUnlisten, readyUnlisten, errorUnlisten] = await Promise.all([
        onProviderLoading(() => setStatus("loading")),
        onProviderReady(() => {
          setStatus(cfg?.generation.provider === "unconfigured" ? "unconfigured" : "ready");
        }),
        onProviderError(() => setStatus("error")),
      ]);
      unlistens = [loadingUnlisten, readyUnlisten, errorUnlisten];
    };

    setup();
    return () => {
      active = false;
      unlistens.forEach((u) => u());
    };
  }, []);

  const handleSave = async () => {
    setSavePhase("saving");
    setSaveError(null);
    const newConfig: GenerationConfig = {
      provider: externalUrl.trim() ? "external" : "unconfigured",
      external_url: externalUrl.trim() || null,
      api_key: apiKey.trim() || null,
      model_name: modelName.trim() || null,
      model_path: config?.model_path ?? null,
    };
    try {
      await updateProvider(newConfig);
      setSavePhase("idle");
      setStatus(newConfig.provider === "unconfigured" ? "unconfigured" : "ready");
      setConfig(newConfig);
    } catch (e) {
      const message = String(e);
      if (message.includes("provider-not-ready")) {
        setStatus("loading");
        setSavePhase("idle");
      } else {
        setSaveError(message);
        setSavePhase("error");
      }
    }
  };

  return (
    <div className="model-panel">
      <h3>AI Generation</h3>

      {status === "loading" && <p className="settings-hint">Waking up the Librarian…</p>}
      {status === "ready" && config?.provider === "sidecar" && (
        <p className="settings-hint">Local model running.</p>
      )}
      {status === "error" && (
        <p className="model-error">
          Provider failed to start. Configure an external URL below or retry from onboarding.
        </p>
      )}
      {status === "unconfigured" && (
        <p className="model-error">
          No generation provider configured. Enter an external URL or run Auto-Install from onboarding.
        </p>
      )}

      <div className="rule-form">
        <label htmlFor="gen-url">External base URL</label>
        <input
          id="gen-url"
          type="text"
          placeholder="http://localhost:11434/v1"
          value={externalUrl}
          onChange={(e) => setExternalUrl(e.target.value)}
          className="rule-input"
        />
        <label htmlFor="gen-key">API key (optional)</label>
        <input
          id="gen-key"
          type="password"
          placeholder="sk-..."
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          className="rule-input"
        />
        <label htmlFor="gen-model">Model name</label>
        <input
          id="gen-model"
          type="text"
          placeholder="e.g. llama3.2, gpt-4o"
          value={modelName}
          onChange={(e) => setModelName(e.target.value)}
          className="rule-input"
        />
        <button
          className="rule-add-btn"
          onClick={handleSave}
          disabled={savePhase === "saving"}
        >
          {savePhase === "saving" ? "Saving…" : "Save"}
        </button>
      </div>

      {savePhase === "error" && (
        <p className="model-error">Failed to save settings to disk: {saveError}</p>
      )}
    </div>
  );
}
