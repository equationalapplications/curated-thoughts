import { useEffect, useState } from "react";
import { getProviderConfig } from "../lib/tauri";
import {
  guardListen,
  onEmbedInitDone,
  onEmbedInitError,
  onEmbedInitProgress,
  onProviderError,
  onProviderLoading,
  onProviderReady,
  safeUnlisten,
  UnlistenFn,
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

    const subscriptions: Array<Promise<UnlistenFn>> = [
      guardListen(
        onProviderLoading(() => {
          if (active) setGeneration("loading");
        }),
        "provider-loading",
      ),
      guardListen(
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
        "provider-ready",
      ),
      guardListen(
        onProviderError(() => {
          if (active) setGeneration("error");
        }),
        "provider-error",
      ),
      guardListen(
        onEmbedInitProgress(() => {
          if (active) setEmbedding("loading");
        }),
        "embed-init-progress",
      ),
      guardListen(
        onEmbedInitDone(() => {
          if (active) setEmbedding("ok");
        }),
        "embed-init-done",
      ),
      guardListen(
        onEmbedInitError(() => {
          if (active) setEmbedding("error");
        }),
        "embed-init-error",
      ),
    ];

    return () => {
      active = false;
      subscriptions.forEach((p) => void safeUnlisten(p));
    };
  }, []);

  return { generation, embedding };
}
