import { vi, describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import type { WikiStatusEventPayload } from '../lib/tauri';

type EventCallback = (e: { payload: WikiStatusEventPayload }) => void;
let capturedCallback: EventCallback | null = null;

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockImplementation(
    (_event: string, cb: EventCallback) => {
      capturedCallback = cb;
      return Promise.resolve(() => { capturedCallback = null; });
    }
  ),
}));

import { useWikiStatus } from '../hooks/useWikiStatus';

describe('useWikiStatus', () => {
  beforeEach(() => {
    capturedCallback = null;
    vi.clearAllMocks();
  });

  it('returns initial idle status', () => {
    const { result } = renderHook(() => useWikiStatus());
    expect(result.current).toEqual({
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
  });

  it('updates when wiki-status-change fires with ingest working', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({
        payload: {
          ingest: 'working',
          ingestStage: 'Embedding',
          ingestSubject: '/note.md',
          librarian: false,
          healing: false,
          pruning: false,
          forgetting: false,
        },
      });
    });
    expect(result.current).toEqual({
      ingest: 'working',
      ingestStage: 'Embedding',
      ingestSubject: '/note.md',
      librarian: false,
      healing: false,
      pruning: false,
      forgetting: false,
      busy: true,
      activeJob: 'ingesting',
      activeJobLabel: 'Ingesting',
    });
  });

  it('updates when wiki-status-change fires with heal true', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({
        payload: {
          ingest: 'idle',
          librarian: false,
          healing: true,
          pruning: false,
          forgetting: false,
        },
      });
    });
    expect(result.current).toEqual({
      ingest: 'idle',
      ingestStage: null,
      ingestSubject: null,
      librarian: false,
      healing: true,
      pruning: false,
      forgetting: false,
      busy: true,
      activeJob: 'healing',
      activeJobLabel: 'Healing',
    });
  });

  it('isSystemBusy is true when any field is active', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({
        payload: {
          ingest: 'idle',
          librarian: true,
          healing: false,
          pruning: false,
          forgetting: false,
        },
      });
    });
    const { ingest, librarian, healing, pruning, forgetting } = result.current;
    expect(ingest !== 'idle' || librarian || healing || pruning || forgetting).toBe(true);
  });

  it('merges partial payload preserving prior state', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({
        payload: { ingest: 'working', librarian: false, healing: false, pruning: false, forgetting: false },
      });
    });
    await act(async () => {
      capturedCallback?.({ payload: { pruning: true } });
    });
    expect(result.current.ingest).toBe('working');
    expect(result.current.pruning).toBe(true);
    expect(result.current.busy).toBe(true);
  });

  it('normalizes legacy heal/prune keys', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { heal: true, prune: false } });
    });
    expect(result.current.healing).toBe(true);
    expect(result.current.pruning).toBe(false);
    expect(result.current.busy).toBe(true);
  });

  it('reports degraded ingest so the UI can show a banner instead of a spinner', async () => {
    const { result } = renderHook(() => useWikiStatus());

    await act(async () => {
      capturedCallback?.({
        payload: { ingest: 'degraded' },
      });
    });

    await waitFor(() => {
      expect(result.current.ingest).toBe('degraded');
      expect(result.current.activeJob).toBe('ingesting');
    });
  });

  it('treats idle ingest as not busy', async () => {
    const { result } = renderHook(() => useWikiStatus());

    await act(async () => {
      capturedCallback?.({
        payload: { ingest: 'idle' },
      });
    });

    await waitFor(() => {
      expect(result.current.busy).toBe(false);
    });
  });
});
