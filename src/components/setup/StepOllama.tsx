import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-shell";
import { checkOllama, getRecommendedModel, pullModel, startOllamaServer } from "../../lib/tauri";
import { onPullProgress } from "../../lib/events";

interface Props { onNext: () => void }

const POLL_INTERVAL_MS = 3000;

export function StepOllama({ onNext }: Props) {
  const [phase, setPhase] = useState<"checking" | "needs-install" | "pulling" | "ready" | "error">("checking");
  const [progress, setProgress] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [model, setModel] = useState<string>("llama3.2:3b");
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  function stopPolling() {
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }

  async function startPull(modelId?: string) {
    stopPolling();
    const target = modelId ?? model;
    setPhase("pulling");
    setProgress(0);
    const unlisten = await onPullProgress(({ completed, total }) => {
      setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
    });
    try {
      await pullModel(target);
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

      if (s.installed && s.running && s.models.length > 0) {
        setPhase("ready");
      } else if (!s.installed) {
        setPhase("needs-install");
        open("https://ollama.com/download");
        pollRef.current = setInterval(async () => {
          const status = await checkOllama();
          if (status.installed) {
            if (!status.running) await startOllamaServer().catch(() => {});
            startPull(recommended);
          }
        }, POLL_INTERVAL_MS);
      } else if (s.installed && !s.running) {
        await startOllamaServer().catch(() => {});
        startPull(recommended);
      } else {
        startPull(recommended);
      }
    })();

    return stopPolling;
  }, []);

  return (
    <div className="setup-step">
      <h2>Install Ollama</h2>
      {phase === "checking" && <p>Checking for Ollama...</p>}
      {phase === "needs-install" && (
        <>
          <p>
            The Ollama download page has been opened in your browser. Install it, then come back — this will continue automatically.
          </p>
          <p className="ollama-hint">Or install via Homebrew: <code>brew install ollama</code></p>
          <button onClick={() => open("https://ollama.com/download")}>Re-open download page</button>
        </>
      )}
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
          <button onClick={() => startPull()}>Retry</button>
        </>
      )}
    </div>
  );
}
