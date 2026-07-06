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

    Promise.all([
      onProviderLoading(() => setGeneration("loading")),
      onProviderReady(() => {
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
      onProviderError(() => setGeneration("error")),
      onEmbedInitProgress(() => setEmbedding("loading")),
      onEmbedInitDone(() => setEmbedding("ok")),
      onEmbedInitError(() => setEmbedding("error")),
    ]).then((uls) => unlisteners.push(...uls));

    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, []);

  return { generation, embedding };
}
