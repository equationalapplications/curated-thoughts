import { useState } from "react";
import { ONTOLOGY_OPTIONS } from "../../lib/ontology";
import { setOntologySelection, type OntologySelection } from "../../lib/tauri";
import { applyOntologyChange } from "../../lib/wiki";

/**
 * Ontology choice for the setup wizard. Deliberately not its own step: the
 * default is correct for most people, so this renders as one preselected radio
 * with the alternatives behind a disclosure (spec D4).
 */
export function OntologyChoice() {
  const [selection, setSelection] = useState<OntologySelection>("schema-org");
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // D6 hot-swap guard: a second radio click must not start another
  // `applyOntologyChange` while the first is mid-flight. Without this,
  // completion order could leave the displayed selection different from
  // the active Wiki configuration (the earlier helper pulls from
  // `_ontologySelection`). Mirrors `OntologyPanel` so both surfaces
  // serialize the same way.
  const [busy, setBusy] = useState(false);

  const visible = expanded
    ? ONTOLOGY_OPTIONS
    : ONTOLOGY_OPTIONS.filter((o) => o.value === selection);

  const choose = async (next: OntologySelection) => {
    if (next === selection || busy) return;
    setBusy(true);
    setError(null);
    try {
      await setOntologySelection(next);
      // Run the shared D6 sequence so the wizard reuses the same reseed +
      // backfill path as the Settings panel. On a fresh vault the backfill
      // loop is a no-op; on a re-run of the wizard on an existing vault
      // (if ever added) it rebuilds typed classifications correctly.
      await applyOntologyChange(next);
      setSelection(next);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <fieldset className="setup-ontology">
      <legend>What kinds of things should Tessera track?</legend>
      {visible.map((option) => (
        <label key={option.value} className="setup-ontology-option">
          <input
            type="radio"
            name="ontology"
            value={option.value}
            checked={selection === option.value}
            disabled={busy}
            onChange={() => void choose(option.value)}
          />
          <span className="setup-ontology-label">{option.label}</span>
          <span className="setup-ontology-sub">{option.subLabel}</span>
        </label>
      ))}
      {!expanded && (
        <button type="button" className="setup-ontology-change" onClick={() => setExpanded(true)}>
          Change
        </button>
      )}
      {busy && <p role="alert">Saving your choice…</p>}
      {error && !busy && <p role="alert">Could not save that choice: {error}</p>}
    </fieldset>
  );
}
