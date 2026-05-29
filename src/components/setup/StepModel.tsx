import { useEffect, useRef, useState } from "react";
import {
  downloadSidecarEngine,
  downloadModelWeights,
  updateProvider,
  type GenerationConfig,
} from "../../lib/tauri";
import {
  onGgufDownloadProgress,
  onSidecarDownloadProgress,
  onProviderError,
} from "../../lib/events";

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
  const [phase, setPhase] = useState<Phase>("choice");
  const [progress, setProgress] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [externalUrl, setExternalUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [modelName, setModelName] = useState("");
  const unlistens = useRef<Array<() => void>>([]);

  const cleanup = () => {
    unlistens.current.forEach((u) => {
      if (typeof u === "function") {
        u();
      }
    });
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
    const [unlistenProgress, unlistenEngineProgress, unlistenError] = await Promise.all([
      onSidecarDownloadProgress(({ downloaded, total }) => {
        setProgress(total > 0 ? Math.round((downloaded / total) * 100) : 0);
      }),
      onGgufDownloadProgress(({ downloaded, total }) => {
        setProgress(total > 0 ? Math.round((downloaded / total) * 100) : 0);
      }),
      onProviderError(({ message }) => {
        setErrorMsg(message);
        setPhase("auto-error");
      }),
    ]);
    unlistens.current = [unlistenProgress, unlistenEngineProgress, unlistenError];

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
    <div className="setup-step">
      <h2>Set Up AI Generation</h2>

      {phase === "choice" && (
        <>
          <p>Choose how to power the Active Librarian:</p>
          <button onClick={runAutoInstall} disabled={!AUTO_INSTALL_AVAILABLE}>
            Auto-Install (recommended)
          </button>
          <p className="ollama-hint">Downloads llama-server and a model to your machine.</p>
          {!AUTO_INSTALL_AVAILABLE && (
            <p className="ollama-hint" style={{ color: "gray" }}>
              Auto-install is unavailable until the recommended model checksum is configured.
            </p>
          )}
          <button onClick={() => setPhase("skip")}>Skip / Use my own</button>
          <p className="ollama-hint">
            Point to an existing OpenAI-compatible endpoint or continue without a provider.
          </p>
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
          <p style={{ color: "red" }}>Error: {errorMsg}</p>
          <button onClick={() => setPhase("choice")}>Back</button>
          <button onClick={runAutoInstall}>Retry</button>
        </>
      )}

      {phase === "skip" && (
        <>
          <p>Optional: enter an OpenAI-compatible base URL and API key.</p>
          <input
            type="text"
            placeholder="http://localhost:11434/v1"
            value={externalUrl}
            onChange={(e) => setExternalUrl(e.target.value)}
          />
          <input
            type="password"
            placeholder="API key (optional)"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
          />
          <input
            type="text"
            placeholder="Model name (optional)"
            value={modelName}
            onChange={(e) => setModelName(e.target.value)}
          />
          <button onClick={handleSkipSave}>Save & continue</button>
          <button onClick={() => setPhase("choice")}>Back</button>
          {errorMsg && <p style={{ color: "red" }}>{errorMsg}</p>}
        </>
      )}
    </div>
  );
}
