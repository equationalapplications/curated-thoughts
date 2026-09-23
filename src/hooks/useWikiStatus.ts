import { useEffect, useState } from 'react';
import {
  subscribeEntityStatus,
  getWikiStatus,
  type IngestHealth,
  type WikiStatusEventPayload,
  type WikiStatusPayload,
} from '../lib/tauri';

export interface WikiStatus extends WikiStatusPayload {
  busy: boolean;
  activeJob:
    | 'idle'
    | 'ingesting'
    | 'librarian'
    | 'healing'
    | 'pruning'
    | 'forgetting'
    | 'multiple';
  activeJobLabel: string | null;
}

const jobLabels: Record<WikiStatus['activeJob'], string | null> = {
  idle: null,
  ingesting: 'Ingesting',
  librarian: 'Refreshing knowledge',
  healing: 'Healing',
  pruning: 'Pruning',
  forgetting: 'Forgetting',
  multiple: 'Multiple jobs',
};

function isIngestActive(ingest: IngestHealth | undefined): boolean {
  return !!ingest && ingest !== 'idle';
}

/// Whether ingest is doing work that a vault switch would interrupt.
///
/// Distinct from `isIngestActive`, which drives `activeJob` and the status
/// banner. 'degraded' means the watchdog exhausted its respawn cap and parked
/// the pipeline: no work is in flight and none will be until the user acts, so
/// it still belongs in `activeJob` (the banner) but must NOT count as busy.
/// Gating on it would put `switchVault` behind `wikiStatus.busy` — and the
/// bounded `switch_vault` (10s join + epoch bump) is precisely the documented
/// recovery from a parked pipeline (spec §7), leaving force-quitting the app
/// as the only way out.
function isIngestBusy(ingest: IngestHealth | undefined): boolean {
  return isIngestActive(ingest) && ingest !== 'degraded';
}

function getActiveJob(payload: WikiStatusPayload): WikiStatus['activeJob'] {
  const active = [
    isIngestActive(payload.ingest) ? 'ingesting' : null,
    payload.librarian ? 'librarian' : null,
    payload.healing ? 'healing' : null,
    payload.pruning ? 'pruning' : null,
    payload.forgetting ? 'forgetting' : null,
  ].filter(Boolean) as Array<WikiStatus['activeJob']>;

  if (active.length === 0) return 'idle';
  if (active.length === 1) return active[0];
  return 'multiple';
}

export function useWikiStatus(): WikiStatus {
  const [status, setStatus] = useState<WikiStatus>({
    ingest: 'idle',
    ingestStage: null,
    ingestSubject: null,
    librarian: false,
    healing: false,
    pruning: false,
    forgetting: false,
    diagnosticErrors: 0,
    diagnosticWarnings: 0,
    busy: false,
    activeJob: 'idle',
    activeJobLabel: null,
  });

  useEffect(() => {
    let cleanup: (() => void) | null = null;
    let cancelled = false;

    const normalizePayload = (
      payload: WikiStatusEventPayload,
    ): Partial<WikiStatusPayload> => ({
      ...payload,
      healing: payload.healing ?? payload.heal,
      pruning: payload.pruning ?? payload.prune,
    });

    const applyPayload = (next: WikiStatusPayload) => {
      setStatus((prev) => {
        const activeJob = getActiveJob(next);
        const ingestBusy = isIngestBusy(next.ingest);
        return {
          ...next,
          busy:
            ingestBusy ||
            next.librarian ||
            next.healing ||
            next.pruning ||
            next.forgetting,
          activeJob,
          activeJobLabel: jobLabels[activeJob],
        };
      });
    };

    // Snapshot the counters the engine has already accumulated during
    // `setupWiki` — the listener below is not installed until after this
    // resolves, so events emitted in that window would otherwise be lost
    // (CodeRabbit review of the 7.7.4 adoption PR).
    getWikiStatus()
      .then((snapshot) => {
        if (cancelled) return;
        applyPayload({
          ingest: snapshot.ingest,
          ingestStage: snapshot.ingestStage ?? null,
          ingestSubject: snapshot.ingestSubject ?? null,
          librarian: snapshot.librarian,
          healing: snapshot.healing,
          pruning: snapshot.pruning,
          forgetting: snapshot.forgetting,
          diagnosticErrors: snapshot.diagnosticErrors ?? 0,
          diagnosticWarnings: snapshot.diagnosticWarnings ?? 0,
        });
      })
      .catch((err: unknown) => {
        // A failed snapshot must not block event subscription — log and
        // continue so a transient IPC error doesn't leave the hook stuck.
        console.warn('[useWikiStatus] snapshot failed:', err);
      });

    subscribeEntityStatus((e) => {
      if (cancelled) return;
      setStatus((prev) => {
        const normalized = normalizePayload(e.payload);
        // Use explicit undefined checks so a `null` from the backend
        // clears the previous value rather than being treated as
        // "absent" by `??`. Working → idle transition needs to drop the
        // last stage/subject so the UI doesn't keep showing a stale
        // banner (CodeRabbit review PRRT_kwDOSVmXas6d28eC).
        const ingestStage =
          normalized.ingestStage !== undefined
            ? normalized.ingestStage
            : prev.ingestStage;
        const ingestSubject =
          normalized.ingestSubject !== undefined
            ? normalized.ingestSubject
            : prev.ingestSubject;
        const payload: WikiStatusPayload = {
          ingest: (normalized.ingest ?? prev.ingest) as IngestHealth,
          ingestStage,
          ingestSubject,
          librarian: normalized.librarian ?? prev.librarian,
          healing: normalized.healing ?? prev.healing,
          pruning: normalized.pruning ?? prev.pruning,
          forgetting: normalized.forgetting ?? prev.forgetting,
          diagnosticErrors: normalized.diagnosticErrors ?? prev.diagnosticErrors ?? 0,
          diagnosticWarnings: normalized.diagnosticWarnings ?? prev.diagnosticWarnings ?? 0,
        };
        const activeJob = getActiveJob(payload);
        const ingestBusy = isIngestBusy(payload.ingest);
        return {
          ...payload,
          busy:
            ingestBusy ||
            payload.librarian ||
            payload.healing ||
            payload.pruning ||
            payload.forgetting,
          activeJob,
          activeJobLabel: jobLabels[activeJob],
        };
      });
    })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
          return;
        }
        cleanup = unlisten;
      })
      .catch((error) => {
        console.error('Failed to subscribe to wiki status events', error);
      });

    return () => {
      cancelled = true;
      cleanup?.();
    };
  }, []);

  return status;
}
