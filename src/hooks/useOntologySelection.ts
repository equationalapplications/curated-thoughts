import { useCallback, useEffect, useState } from "react";
import {
  getOntologySelection,
  setOntologySelection as persist,
  type OntologySelection,
} from "../lib/tauri";

export function useOntologySelection() {
  const [selection, setSelection] = useState<OntologySelection>("schema-org");
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let active = true;
    getOntologySelection()
      .then((s) => {
        if (active) setSelection(s);
      })
      .catch((e) => console.error("[ontology] could not read selection", e))
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  const save = useCallback(async (next: OntologySelection) => {
    await persist(next);
    setSelection(next);
  }, []);

  return { selection, save, loading };
}
