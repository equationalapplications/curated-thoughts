export function EmbeddingPanel() {
  return (
    <div className="model-panel">
      <h3>Embeddings</h3>
      <p className="settings-hint">
        Local vector engine: <strong>MiniLM-L6-V2 (fastembed)</strong>
      </p>
      <p className="settings-hint">
        Runs entirely in-process. No external service is required.
      </p>
    </div>
  );
}
