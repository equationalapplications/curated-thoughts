import { useState } from "react";
import { ONTOLOGY_OPTIONS } from "../../lib/ontology";
import { setOntologySelection, type OntologySelection } from "../../lib/tauri";

/**
 * Ontology choice for the setup wizard. Deliberately not its own step: the
 * default is correct for most people, so this renders as one preselected radio
 * with the alternatives behind a disclosure (spec D4).
 */
export function OntologyChoice() {
  const [selection, setSelection] = useState<OntologySelection>("schema-org");
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const visible = expanded
    ? ONTOLOGY_OPTIONS
    : ONTOLOGY_OPTIONS.filter((o) => o.value === selection);

  const choose = async (next: OntologySelection) => {
    if (next === selection) return;
    setSelection(next);
    try {
      await setOntologySelection(next);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
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
      {error && <p role="alert">Could not save that choice: {error}</p>}
    </fieldset>
  );
}
