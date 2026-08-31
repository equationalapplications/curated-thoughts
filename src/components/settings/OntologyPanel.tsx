import { useState } from "react";
import { useOntologySelection } from "../../hooks/useOntologySelection";
import { ONTOLOGY_OPTIONS, manifestFor, modeFor } from "../../lib/ontology";
import type { OntologySelection } from "../../lib/tauri";
import { getWorkspaceId, wiki } from "../../lib/wiki";

/** Every tier that carries a seeded manifest (spec D5). */
function seededEntities(): string[] {
  return ["tier_fact", "tier_wisdom", getWorkspaceId()];
}

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

    setBusy(true);
    setError(null);
    try {
      await save(next);

      // seedManifests only writes when an entity has no row, so an existing
      // vault must be reseeded explicitly.
      const manifest = manifestFor(next);
      const mode = modeFor(next);
      for (const entityId of seededEntities()) {
        if (manifest) {
          await wiki.setOntologyManifest(entityId, manifest, { mode });
        }
        // Reclassify under the new manifest. `remaining` is the engine's
        // documented convergence signal; it is always 0 when mode is 'off'.
        let remaining = Infinity;
        while (remaining > 0) {
          const result = await wiki.runOntologyBackfill(entityId);
          remaining = result.remaining;
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
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
