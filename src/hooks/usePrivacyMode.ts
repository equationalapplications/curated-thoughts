import { useCallback, useEffect, useState } from "react";

export type PrivacyMode = "strict" | "ephemeral" | "full";

const STORAGE_KEY = "ct-privacy-mode";

function readStored(): PrivacyMode {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === "strict" || raw === "ephemeral" || raw === "full") return raw;
  } catch {
    /* private browsing */
  }
  return "strict";
}

export function usePrivacyMode(): {
  mode: PrivacyMode;
  setMode: (mode: PrivacyMode) => void;
} {
  const [mode, setModeState] = useState<PrivacyMode>(readStored);

  const setMode = useCallback((next: PrivacyMode) => {
    setModeState(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      /* ignore */
    }
  }, []);

  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY && e.newValue) {
        const raw = e.newValue;
        if (raw === "strict" || raw === "ephemeral" || raw === "full") {
          setModeState(raw);
        }
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  return { mode, setMode };
}
