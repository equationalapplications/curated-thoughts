import { useState } from "react";
import { useOntologySelection } from "../../hooks/useOntologySelection";
import { ONTOLOGY_OPTIONS } from "../../lib/ontology";
import type { OntologySelection } from "../../lib/tauri";
import { applyOntologyChange } from "../../lib/wiki";

export function OntologyPanel() {
  const { selection, save, loading } = useOntologySelection();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleChange = async (next: OntologySelection) => {
    if (next === selection || busy) return;

    // D6: switching invalidates every existing okf_type and typed edge.
    const ok = window.confirm(
      "Existing type labels and connections will be rebuilt. " +
        "Your notes, facts, and search are not affected.",
    );
    if (!ok) return;

    // Capture the prior selection so a failed save/reseed/backfill can
    // restore the user's previous choice and not leave the radio on a
    // selection that did not actually take effect.
    const prior = selection;

    setBusy(true);
    setError(null);
    try {
      await save(next);
      await applyOntologyChange(next);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      // Restore the prior selection so the radio reflects what is actually
      // persisted and a retry is not blocked by the same-selection guard.
      try {
        await save(prior);
      } catch {
        // best-effort: surface the original error, leave the prior save
        // to whichever future operation can succeed.
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings-section">
      <h3>Knowledge schema</h3>
      <p className="settings-hint">
        What kinds of things Tessera tracks, and how facts are connected.
        Changing this rebuilds type labels and connections — your notes and
        search are untouched.
      </p>
      {loading ? (
        <p className="settings-hint">Loading…</p>
      ) : (
        ONTOLOGY_OPTIONS.map((option) => (
          <label key={option.value} className="settings-ontology-option">
            <input
              type="radio"
              name="ontology-selection"
              value={option.value}
              checked={selection === option.value}
              disabled={busy}
              onChange={() => void handleChange(option.value)}
            />
            <span className="settings-ontology-label">{option.label}</span>
            <span className="settings-ontology-sub">{option.subLabel}</span>
            {option.packageId && (
              <span className="settings-ontology-pkg">{option.packageId}</span>
            )}
          </label>
        ))
      )}
      {busy && <p className="settings-hint">Rebuilding type labels…</p>}
      {error && <p role="alert">Switch failed: {error}</p>}
    </div>
  );
}
