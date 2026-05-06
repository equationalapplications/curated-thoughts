import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { setVaultPath } from "../../lib/tauri";

interface Props { onNext: (path: string) => void }

export function StepVaultPicker({ onNext }: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function pickFolder() {
    const path = await open({ directory: true, multiple: false, title: "Choose your vault folder" });
    if (typeof path === "string") setSelected(path);
  }

  async function confirm() {
    if (!selected) return;
    setSaving(true);
    await setVaultPath(selected);
    setSaving(false);
    onNext(selected);
  }

  return (
    <div className="setup-step">
      <h2>Choose Your Vault</h2>
      <p>Pick the folder where your documents live. The app will watch it for changes.</p>
      <button onClick={pickFolder}>Browse...</button>
      {selected && <p className="selected-path">{selected}</p>}
      <button onClick={confirm} disabled={!selected || saving}>
        {saving ? "Saving..." : "Confirm"}
      </button>
    </div>
  );
}
