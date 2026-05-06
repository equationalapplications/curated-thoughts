import { useState, useEffect } from "react";
import { getFolderRules, setFolderRule, deleteFolderRule, FolderRule } from "../../lib/tauri";

const MODES = ["index", "summarize", "synthesize"] as const;

export function FolderRulesPanel() {
  const [rules, setRules] = useState<FolderRule[]>([]);
  const [folderPath, setFolderPath] = useState("");
  const [mode, setMode] = useState<string>("index");
  const [autoApprove, setAutoApprove] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = () => getFolderRules().then(setRules).catch(() => {});

  useEffect(() => { load(); }, []);

  async function handleAdd() {
    if (!folderPath.trim()) return;
    setSaving(true);
    try {
      await setFolderRule(folderPath.trim(), mode, autoApprove);
      setFolderPath("");
      setMode("index");
      setAutoApprove(false);
      await load();
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete(id: number) {
    await deleteFolderRule(id);
    await load();
  }

  return (
    <div className="folder-rules-panel">
      <h3>Folder Rules</h3>
      <p className="settings-hint">Set how the librarian processes each folder.</p>
      {rules.length > 0 && (
        <div className="rules-list">
          {rules.map((r) => (
            <div key={r.id} className="rule-row">
              <span className="rule-path">{r.folder_path}</span>
              <span className="rule-mode">{r.librarian_mode}</span>
              {r.auto_approve && <span className="rule-auto">auto</span>}
              <button className="rule-delete" onClick={() => handleDelete(r.id)}>✕</button>
            </div>
          ))}
        </div>
      )}
      <div className="rule-form">
        <input
          type="text"
          placeholder="Folder path (e.g. /vault/documents/research)"
          value={folderPath}
          onChange={(e) => setFolderPath(e.target.value)}
          className="rule-input"
        />
        <select value={mode} onChange={(e) => setMode(e.target.value)} className="rule-select">
          {MODES.map((m) => <option key={m} value={m}>{m}</option>)}
        </select>
        <label className="rule-auto-label">
          <input type="checkbox" checked={autoApprove} onChange={(e) => setAutoApprove(e.target.checked)} />
          Auto-approve
        </label>
        <button className="rule-add-btn" onClick={handleAdd} disabled={saving || !folderPath.trim()}>
          {saving ? "Saving…" : "Add rule"}
        </button>
      </div>
    </div>
  );
}
