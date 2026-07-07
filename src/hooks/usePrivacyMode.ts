import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getPrivacyMode,
  setPrivacyMode as setPrivacyModeInvoke,
  type PrivacyMode,
  type PrivacyState,
} from "../lib/tauri";

export type { PrivacyMode };

const LEGACY_STORAGE_KEY = "ct-privacy-mode";

function mapLegacyMode(raw: string): PrivacyMode | null {
  if (raw === "strict" || raw === "ephemeral") return raw;
  if (raw === "full" || raw === "connected") return "connected";
  return null;
}

function readLegacyMode(): PrivacyMode | null {
  try {
    const raw = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (!raw) return null;
    return mapLegacyMode(raw);
  } catch {
    return null;
  }
}

function clearLegacyMode() {
  try {
    localStorage.removeItem(LEGACY_STORAGE_KEY);
  } catch {
    /* ignore */
  }
}

const defaultState: PrivacyState = {
  mode: "strict",
  chosen: false,
  needs_migration_disclosure: false,
  ephemeral_disclosure_acknowledged: false,
};

export function usePrivacyMode(): PrivacyState & {
  setMode: (mode: PrivacyMode) => Promise<void>;
  loading: boolean;
} {
  const [state, setState] = useState<PrivacyState>(defaultState);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    const next = await getPrivacyMode();
    setState(next);
    return next;
  }, []);

  useEffect(() => {
    let active = true;

    const load = async () => {
      try {
        let next = await getPrivacyMode();
        if (!next.chosen) {
          const legacy = readLegacyMode();
          if (legacy) {
            const result = await setPrivacyModeInvoke(legacy);
            next = result.state;
            clearLegacyMode();
          }
        }
        if (active) {
          setState(next);
        }
      } catch {
        if (active) {
          setState(defaultState);
        }
      } finally {
        if (active) {
          setLoading(false);
        }
      }
    };

    load();

    const unlistenPromise = listen<PrivacyState>("privacy-mode-changed", (event) => {
      setState(event.payload);
    });

    return () => {
      active = false;
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  const setMode = useCallback(async (mode: PrivacyMode) => {
    const result = await setPrivacyModeInvoke(mode);
    setState(result.state);
    clearLegacyMode();
  }, []);

  return {
    ...state,
    loading,
    setMode,
  };
}
