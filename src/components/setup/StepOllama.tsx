import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-shell";
import { checkOllama, getRecommendedModel, pullModel, startOllamaServer } from "../../lib/tauri";
import { onPullProgress } from "../../lib/events";

interface Props { onNext: () => void }

type Phase = "checking" | "needs-install" | "select-model" | "starting-server" | "pulling" | "ready" | "error";

const POLL_INTERVAL_MS = 3000;

export function StepOllama({ onNext }: Props) {
  const [phase, setPhase] = useState<Phase>("checking");
  const [progress, setProgress] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [model, setModel] = useState<string>("");
  const [ollamaRunning, setOllamaRunning] = useState(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  function stopPolling() {
    if (pollRef.current) { clearInterval(pollRef.current); pollRef.current = null; }
  }

  async function pull(modelId: string) {
    stopPolling();
    if (!ollamaRunning) {
      setPhase("starting-server");
      await startOllamaServer().catch(() => {});
    }
    setPhase("pulling");
    setProgress(0);
    const unlisten = await onPullProgress(({ completed, total }) => {
      setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
    });
    try {
      await pullModel(modelId);
      setPhase("ready");
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("error");
    } finally {
      unlisten();
    }
  }

  useEffect(() => {
    (async () => {
      const [s, recommended] = await Promise.all([checkOllama(), getRecommendedModel()]);
      setModel(recommended);
      setOllamaRunning(s.running);

      if (!s.installed) {
        setPhase("needs-install");
        open("https://ollama.com/download");
        pollRef.current = setInterval(async () => {
          const status = await checkOllama();
          if (status.installed) {
            stopPolling();
            setOllamaRunning(status.running);
            setPhase("select-model");
          }
        }, POLL_INTERVAL_MS);
      } else {
        setPhase("select-model");
      }
    })();
    return stopPolling;
  }, []);

  return (
    <div className="setup-step">
      <h2>Set Up AI Model</h2>

      {phase === "checking" && <p>Checking for Ollama...</p>}

      {phase === "needs-install" && (
        <>
          <p>Ollama is required for local AI. The download page has been opened in your browser.</p>
          <p className="ollama-hint">Or install via Homebrew: <code>brew install ollama</code></p>
          <button onClick={() => open("https://ollama.com/download")}>Re-open download page</button>
          <p className="ollama-hint">This page will advance automatically once Ollama is installed.</p>
        </>
      )}

      {phase === "select-model" && (
        <>
          <p>Ollama is installed. Choose a model to download:</p>
          <div className="model-select">
            <label htmlFor="model-input">Model name</label>
            <input
              id="model-input"
              type="text"
              value={model}
              onChange={(e) => setModel(e.target.value.trim())}
              placeholder="e.g. llama3.2:3b"
              spellCheck={false}
            />
            <p className="ollama-hint">
              Recommended for your Mac. Browse more at{" "}
              <span className="link" onClick={() => open("https://ollama.com/library")}>
                ollama.com/library
              </span>.
            </p>
          </div>
          <button onClick={() => pull(model)} disabled={!model}>
            Download &amp; continue
          </button>
        </>
      )}

      {phase === "starting-server" && <p>Starting Ollama server…</p>}

      {phase === "pulling" && (
        <>
          <p>Downloading {model}… {progress}%</p>
          <progress value={progress} max={100} style={{ width: "100%" }} />
        </>
      )}

      {phase === "ready" && (
        <>
          <p>Ollama is ready.</p>
          <button onClick={onNext}>Continue</button>
        </>
      )}

      {phase === "error" && (
        <>
          <p style={{ color: "red" }}>Error: {errorMsg}</p>
          <button onClick={() => pull(model)}>Retry</button>
        </>
      )}
    </div>
  );
}
