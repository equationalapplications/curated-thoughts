import { useState, useEffect } from "react";
import { listLocalModels, pullModel, getRecommendedModel } from "../../lib/tauri";
import { guardListen, onPullProgress, safeUnlisten } from "../../lib/events";
import { reportBackgroundError } from "../../lib/errorFeed";

export function ModelPanel() {
  const [models, setModels] = useState<string[]>([]);
  const [recommended, setRecommended] = useState("");
  const [newModel, setNewModel] = useState("");
  const [phase, setPhase] = useState<"idle" | "pulling" | "done" | "error">("idle");
  const [progress, setProgress] = useState(0);
  const [progressUnavailable, setProgressUnavailable] = useState(false);
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
    setProgressUnavailable(false);
    setError(null);
    // Progress listener is best-effort: if it cannot attach, show that and
    // STILL pull — aborting the user's pull over a display listener would be
    // worse. guardListen logs the failure; no second logger here.
    const unlisten = guardListen(
      onPullProgress(({ completed, total }) => {
        setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
      }),
      "ollama-pull-progress",
    );
    // Review M5: AWAIT (not `void`) — Tauri does not buffer events, so
    // pullModel() must not start until registration settles; early
    // ollama-pull-progress events would otherwise be lost and a fast/cached
    // pull could finish before the listener exists (progress stuck at 0%).
    // A rejection still lets the pull proceed (degraded display only).
    await unlisten.catch(() => setProgressUnavailable(true));
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
      void safeUnlisten(unlisten);
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
      {phase === "pulling" && progressUnavailable && <p className="settings-hint">Progress unavailable — pull continuing.</p>}
      {phase === "done" && <p className="model-success">Model pulled successfully.</p>}
      {phase === "error" && <p className="model-error">Error: {error}</p>}
    </div>
  );
}
