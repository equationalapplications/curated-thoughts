import { useEffect, useState } from 'react';
import {
  subscribeEntityStatus,
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
    busy: false,
    activeJob: 'idle',
    activeJobLabel: null,
  });

  useEffect(() => {
    let cleanup: (() => void) | null = null;

    const normalizePayload = (
      payload: WikiStatusEventPayload,
    ): Partial<WikiStatusPayload> => ({
      ...payload,
      healing: payload.healing ?? payload.heal,
      pruning: payload.pruning ?? payload.prune,
    });

    subscribeEntityStatus((e) => {
      setStatus((prev) => {
        const normalized = normalizePayload(e.payload);
        const payload: WikiStatusPayload = {
          ingest: (normalized.ingest ?? prev.ingest) as IngestHealth,
          ingestStage: normalized.ingestStage ?? prev.ingestStage,
          ingestSubject: normalized.ingestSubject ?? prev.ingestSubject,
          librarian: normalized.librarian ?? prev.librarian,
          healing: normalized.healing ?? prev.healing,
          pruning: normalized.pruning ?? prev.pruning,
          forgetting: normalized.forgetting ?? prev.forgetting,
        };
        const activeJob = getActiveJob(payload);
        const ingestBusy = isIngestActive(payload.ingest);
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
        cleanup = unlisten;
      })
      .catch((error) => {
        console.error('Failed to subscribe to wiki status events', error);
      });

    return () => {
      cleanup?.();
    };
  }, []);

  return status;
}
