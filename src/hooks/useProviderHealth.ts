import { useEffect, useState } from "react";
import { getProviderConfig } from "../lib/tauri";
import {
  onEmbedInitDone,
  onEmbedInitError,
  onEmbedInitProgress,
  onProviderError,
  onProviderLoading,
  onProviderReady,
} from "../lib/events";

export type HealthState = "ok" | "loading" | "error" | "unconfigured";

function generationFromConfig(
  provider: "unconfigured" | "sidecar" | "external",
): HealthState {
  return provider === "unconfigured" ? "unconfigured" : "ok";
}

export function useProviderHealth(): {
  generation: HealthState;
  embedding: HealthState;
} {
  const [generation, setGeneration] = useState<HealthState>("loading");
  const [embedding, setEmbedding] = useState<HealthState>("loading");

  useEffect(() => {
    let active = true;
    const unlisteners: Array<() => void> = [];

    getProviderConfig()
      .then((cfg) => {
        if (!active) return;
        setGeneration(generationFromConfig(cfg.generation.provider));
        setEmbedding(
          cfg.embedding.provider === "fastembed" ? "ok" : "unconfigured",
        );
      })
      .catch(() => {
        if (active) {
          setGeneration("error");
          setEmbedding("error");
        }
      });

    void Promise.all([
      onProviderLoading(() => {
        if (active) setGeneration("loading");
      }),
      onProviderReady(() => {
        if (!active) return;
        getProviderConfig()
          .then((cfg) => {
            if (active) {
              setGeneration(generationFromConfig(cfg.generation.provider));
            }
          })
          .catch(() => {
            if (active) setGeneration("error");
          });
      }),
      onProviderError(() => {
        if (active) setGeneration("error");
      }),
      onEmbedInitProgress(() => {
        if (active) setEmbedding("loading");
      }),
      onEmbedInitDone(() => {
        if (active) setEmbedding("ok");
      }),
      onEmbedInitError(() => {
        if (active) setEmbedding("error");
      }),
    ]).then((uls) => {
      if (!active) {
        uls.forEach((u) => u());
      } else {
        unlisteners.push(...uls);
      }
    });

    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, []);

  return { generation, embedding };
}
