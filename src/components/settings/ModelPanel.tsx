import { useState, useEffect } from "react";
import { listLocalModels, pullModel, getRecommendedModel } from "../../lib/tauri";
import { onPullProgress } from "../../lib/events";
import { reportBackgroundError } from "../../lib/errorFeed";

export function ModelPanel() {
  const [models, setModels] = useState<string[]>([]);
  const [recommended, setRecommended] = useState("");
  const [newModel, setNewModel] = useState("");
  const [phase, setPhase] = useState<"idle" | "pulling" | "done" | "error">("idle");
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listLocalModels()
      .then(setModels)
      .catch((e) => {
        reportBackgroundError(
          `Failed to load models: ${String(e)}`,
          () => listLocalModels().then(setModels)
        );
      });
    getRecommendedModel()
      .then(setRecommended)
      .catch((e) => {
        reportBackgroundError(
          `Failed to get recommended model: ${String(e)}`,
          () => getRecommendedModel().then(setRecommended)
        );
      });
  }, []);

  async function handlePull() {
    if (!newModel.trim()) return;
    setPhase("pulling");
    setProgress(0);
    setError(null);
    const unlisten = await onPullProgress(({ completed, total }) => {
      setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
    });
    try {
      await pullModel(newModel.trim());
      setPhase("done");
      const updated = await listLocalModels();
      setModels(updated);
      setNewModel("");
    } catch (e) {
      setError(String(e));
      setPhase("error");
    } finally {
      unlisten();
    }
  }

  return (
    <div className="model-panel">
      <h3>AI Models</h3>
      <p className="settings-hint">Recommended for your Mac: <strong>{recommended || "detecting…"}</strong></p>

      {models.length > 0 && (
        <div className="model-list">
          <p className="model-list-label">Installed</p>
          {models.map((m) => (
            <div key={m} className="model-chip">{m}</div>
          ))}
        </div>
      )}

      <div className="rule-form">
        <input
          type="text"
          placeholder="Model name (e.g. llama3.2:3b)"
          value={newModel}
          onChange={(e) => setNewModel(e.target.value)}
          className="rule-input"
          disabled={phase === "pulling"}
        />
        <button
          className="rule-add-btn"
          onClick={handlePull}
          disabled={phase === "pulling" || !newModel.trim()}
        >
          {phase === "pulling" ? `Pulling ${progress}%` : "Pull model"}
        </button>
      </div>

      {phase === "pulling" && (
        <progress value={progress} max={100} style={{ width: "100%", height: "6px" }} />
      )}
      {phase === "done" && <p className="model-success">Model pulled successfully.</p>}
      {phase === "error" && <p className="model-error">Error: {error}</p>}
    </div>
  );
}
