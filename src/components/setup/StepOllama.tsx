import { useEffect, useState } from "react";
import { checkOllama, pullModel } from "../../lib/tauri";
import { onPullProgress } from "../../lib/events";

interface Props { onNext: () => void }

const DEFAULT_MODEL = "llama3.2:3b";

export function StepOllama({ onNext }: Props) {
  const [phase, setPhase] = useState<"checking" | "needs-install" | "pulling" | "ready" | "error">("checking");
  const [progress, setProgress] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    checkOllama().then((s) => {
      if (s.installed && s.running && s.models.length > 0) {
        setPhase("ready");
      } else if (!s.installed) {
        setPhase("needs-install");
      } else {
        startPull();
      }
    });
  }, []);

  async function startPull() {
    setPhase("pulling");
    setProgress(0);
    const unlisten = await onPullProgress(({ completed, total }) => {
      setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
    });
    try {
      await pullModel(DEFAULT_MODEL);
      setPhase("ready");
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("error");
    } finally {
      unlisten();
    }
  }

  return (
    <div className="setup-step">
      <h2>Install Ollama</h2>
      {phase === "checking" && <p>Checking for Ollama...</p>}
      {phase === "needs-install" && (
        <>
          <p>Ollama is required for local AI processing. Download it from <strong>ollama.com</strong>, then click below.</p>
          <button onClick={() => startPull()}>I've installed Ollama</button>
        </>
      )}
      {phase === "pulling" && (
        <>
          <p>Downloading {DEFAULT_MODEL}... {progress}%</p>
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
