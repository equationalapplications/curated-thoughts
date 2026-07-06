import { useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  applyOkfImport,
  exportOkfBundle,
  previewOkfImport,
  type OkfImportMode,
  type OkfImportPreview,
} from "../../lib/tauri";

interface Props {
  onImported?: () => void;
}

interface PendingImport {
  path: string;
  preview: OkfImportPreview;
}

export function OkfInteropBar({ onImported }: Props) {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingImport | null>(null);
  const [mode, setMode] = useState<OkfImportMode>("merge");

  const fail = (e: unknown) =>
    setNotice(typeof e === "string" ? e : e instanceof Error ? e.message : "Operation failed.");

  const handleExport = async () => {
    setNotice(null);
    const dest = await save({
      defaultPath: "brain-okf.zip",
      filters: [{ name: "OKF bundle", extensions: ["zip"] }],
    });
    if (!dest) return;
    setBusy(true);
    try {
      const summary = await exportOkfBundle(dest, null);
      setNotice(`Exported ${summary.entities} entities (${summary.files} files) to ${summary.path}`);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const handleImport = async () => {
    setNotice(null);
    const src = await open({
      multiple: false,
      filters: [{ name: "OKF bundle", extensions: ["zip"] }],
    });
    if (typeof src !== "string") return;
    setBusy(true);
    try {
      const preview = await previewOkfImport(src, "merge");
      setPending({ path: src, preview });
      setMode("merge");
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const handleConfirm = async () => {
    if (!pending) return;
    setBusy(true);
    try {
      const result = await applyOkfImport(pending.path, mode);
      setPending(null);
      setNotice(
        `Imported ${result.facts_added} fact(s), ${result.tasks_added} task(s), ` +
          `${result.edges_added} edge(s), ${result.events_added} event(s). ` +
          `New facts need embedding — run Maintenance → Re-embed.`,
      );
      onImported?.();
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="okf-interop">
      <div className="okf-interop-actions">
        <button type="button" disabled={busy} onClick={() => void handleExport()}>
          Export brain as OKF bundle
        </button>
        <button type="button" disabled={busy} onClick={() => void handleImport()}>
          Import bundle
        </button>
      </div>
      {notice && <p className="okf-interop-notice">{notice}</p>}
      {pending && (
        <div className="okf-interop-preview" role="dialog" aria-label="Import preview">
          <h4>Import preview {pending.preview.profile ? `(${pending.preview.profile})` : "(legacy bundle)"}</h4>
          <ul>
            {pending.preview.entities.map((e) => (
              <li key={e.entity_id}>
                <strong>{e.name}</strong>
                {e.entity_exists ? " (exists)" : " (new)"} — {e.facts_new} new facts
                {e.facts_existing > 0 ? `, ${e.facts_existing} existing` : ""}, {e.tasks_new} tasks,{" "}
                {e.edges_total} edges, {e.events_new} events
                {e.events_duplicate > 0 ? ` (${e.events_duplicate} duplicate)` : ""}
              </li>
            ))}
          </ul>
          {pending.preview.warnings.length > 0 && (
            <p className="okf-interop-warnings">{pending.preview.warnings.join(" · ")}</p>
          )}
          <fieldset className="okf-interop-modes">
            <legend>Mode</legend>
            {(["merge", "replace", "clone"] as const).map((m) => (
              <label key={m}>
                <input
                  type="radio"
                  name="okf-import-mode"
                  value={m}
                  checked={mode === m}
                  onChange={() => setMode(m)}
                />
                {m === "merge" && "Merge (add new, keep existing)"}
                {m === "replace" && "Replace (overwrite entity content)"}
                {m === "clone" && "Clone (import as new entities)"}
              </label>
            ))}
          </fieldset>
          <div className="okf-interop-actions">
            <button type="button" disabled={busy} onClick={() => void handleConfirm()}>
              Confirm import
            </button>
            <button type="button" disabled={busy} onClick={() => setPending(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
