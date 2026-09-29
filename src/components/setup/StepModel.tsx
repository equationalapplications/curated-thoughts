import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  downloadSidecarEngine,
  downloadModelWeights,
  updateProvider,
  type GenerationConfig,
} from "../../lib/tauri";
import {
  guardListen,
  onGgufDownloadProgress,
  onSidecarDownloadProgress,
  onProviderError,
  safeUnlisten,
} from "../../lib/events";
import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { WizardStep } from "./WizardStep";

interface Props {
  onNext: () => void;
}

type Phase =
  | "choice"
  | "auto-downloading-engine"
  | "auto-downloading-model"
  | "auto-starting"
  | "auto-ready"
  | "auto-error"
  | "skip";

const RECOMMENDED_MODEL = {
  url: "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf",
  filename: "llama-3.2-3b-instruct-q4_k_m.gguf",
  sha256: "REPLACE_WITH_KNOWN_SHA256",
};

const AUTO_INSTALL_AVAILABLE = !RECOMMENDED_MODEL.sha256.startsWith("REPLACE_WITH_");

export function StepModel({ onNext }: Props) {
  const { mode: privacyMode } = usePrivacyMode();
  const strictPrivacy = privacyMode === "strict";
  const [phase, setPhase] = useState<Phase>("choice");
  const [progress, setProgress] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [externalUrl, setExternalUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [modelName, setModelName] = useState("");
  const unlistens = useRef<Array<Promise<UnlistenFn>>>([]);

  const cleanup = () => {
    unlistens.current.forEach((u) => void safeUnlisten(u));
    unlistens.current = [];
  };

  useEffect(() => {
    return cleanup;
  }, []);

  const runAutoInstall = async () => {
    if (!AUTO_INSTALL_AVAILABLE) {
      setErrorMsg(
        "Auto-install is unavailable: recommended model checksum is not configured. Please use Skip / Use my own."
      );
      setPhase("auto-error");
      return;
    }

    cleanup();
    // Store the pending subscriptions before draining them so cleanup can
    // remove them even when the step unmounts before listen() resolves.
    unlistens.current = [
      guardListen(onSidecarDownloadProgress(({ downloaded, total }) => {
        setProgress(total > 0 ? Math.round((downloaded / total) * 100) : 0);
      }), "sidecar-download-progress"),
      guardListen(onGgufDownloadProgress(({ downloaded, total }) => {
        setProgress(total > 0 ? Math.round((downloaded / total) * 100) : 0);
      }), "gguf-download-progress"),
      guardListen(onProviderError(({ message }) => {
        setErrorMsg(message);
        setPhase("auto-error");
      }), "provider-error"),
    ];
    // Pre-install drain (NOT teardown): make sure registration outcomes are
    // settled before the downloads start; a rejected subscription degrades
    // progress display but must NOT abort the install.
    await Promise.allSettled(unlistens.current);

    try {
      setPhase("auto-downloading-engine");
      setProgress(0);
      await downloadSidecarEngine();

      setPhase("auto-downloading-model");
      setProgress(0);
      await downloadModelWeights(
        RECOMMENDED_MODEL.url,
        RECOMMENDED_MODEL.filename,
        RECOMMENDED_MODEL.sha256,
      );

      setPhase("auto-starting");
      await updateProvider({
        provider: "sidecar",
        model_path: `models/${RECOMMENDED_MODEL.filename}`,
        model_name: null,
        external_url: null,
        api_key: null,
      });
      setPhase("auto-ready");
      setTimeout(onNext, 800);
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("auto-error");
    }
  };

  const handleSkipSave = async () => {
    setErrorMsg(null);
    const config: GenerationConfig = externalUrl.trim()
      ? {
          provider: "external",
          external_url: externalUrl.trim(),
          api_key: apiKey.trim() || null,
          model_path: null,
          model_name: modelName.trim() || null,
        }
      : {
          provider: "unconfigured",
          external_url: null,
          api_key: null,
          model_path: null,
          model_name: null,
        };
    try {
      await updateProvider(config);
      onNext();
    } catch (e) {
      setErrorMsg(String(e));
    }
  };

  return (
    <WizardStep
      title="Pick your AI"
      subtitle="Choose how to power the Active Librarian."
      onNext={onNext}
      nextDisabled={false}
      isLoading={false}
    >

      {phase === "choice" && (
        <>
          <p>Choose how to power the Active Librarian:</p>
          {/* Primary: it is the recommended path and the one the step is
              written around. It rendered as a plain secondary button
              identical to "Skip / Use my own" directly beneath it. */}
          <button
            type="button"
            className="btn btn--primary"
            onClick={runAutoInstall}
            disabled={!AUTO_INSTALL_AVAILABLE}
          >
            Auto-Install (recommended)
          </button>
          <p className="ollama-hint">Downloads llama-server and a model to your machine.</p>
          {!AUTO_INSTALL_AVAILABLE && (
            /* A hard-coded `color: "gray"` inline style — the only colour
               literal left in the wizard, and wrong in the light theme. */
            <p className="ollama-hint wizard-note">
              Auto-install is unavailable until the recommended model checksum is configured.
            </p>
          )}
          {!strictPrivacy ? (
            <button type="button" className="btn" onClick={() => setPhase("skip")}>
              Skip / Use my own
            </button>
          ) : null}
          {!strictPrivacy ? (
            <p className="ollama-hint">
              Point to an existing OpenAI-compatible endpoint or continue without a provider.
            </p>
          ) : (
            <p className="ollama-hint">
              Strict privacy keeps generation local. Use Auto-Install or continue without a provider.
            </p>
          )}
        </>
      )}

      {phase === "auto-downloading-engine" && (
        <>
          <p>Downloading inference engine… {progress > 0 ? `${progress}%` : ""}</p>
          <progress value={progress} max={100} style={{ width: "100%" }} />
        </>
      )}

      {phase === "auto-downloading-model" && (
        <>
          <p>Downloading model… {progress}%</p>
          <progress value={progress} max={100} style={{ width: "100%" }} />
        </>
      )}

      {phase === "auto-starting" && <p>Starting local inference engine…</p>}

      {phase === "auto-ready" && <p>Ready.</p>}

      {phase === "auto-error" && (
        <>
          <p className="wizard-error">Error: {errorMsg}</p>
          <button type="button" className="btn" onClick={() => setPhase("choice")}>
            Back
          </button>
          <button type="button" className="btn btn--primary" onClick={runAutoInstall}>
            Retry
          </button>
        </>
      )}

      {phase === "skip" && (
        /* The same stacked, label-above-field form the settings panels use —
           the raw label/input pairs here were unstyled, so each label sat on
           the same line as the field above it. */
        <div className="settings-form">
          <div>
            <label htmlFor="external-url">OpenAI-compatible base URL</label>
            <input
              id="external-url"
              type="text"
              placeholder="http://localhost:11434/v1"
              value={externalUrl}
              onChange={(e) => setExternalUrl(e.target.value)}
            />
          </div>
          <div>
            <label htmlFor="api-key">API key (optional)</label>
            <input
              id="api-key"
              type="password"
              placeholder="API key (optional)"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
            />
          </div>
          <div>
            <label htmlFor="model-name">Model name (optional)</label>
            <input
              id="model-name"
              type="text"
              placeholder="Model name (optional)"
              value={modelName}
              onChange={(e) => setModelName(e.target.value)}
            />
          </div>
          <div className="settings-form__actions">
            <button type="button" className="btn btn--primary" onClick={handleSkipSave}>
              Save &amp; continue
            </button>
            <button type="button" className="btn" onClick={() => setPhase("choice")}>
              Back
            </button>
          </div>
          {errorMsg && <p className="wizard-error">{errorMsg}</p>}
        </div>
      )}
    </WizardStep>
  );
}
