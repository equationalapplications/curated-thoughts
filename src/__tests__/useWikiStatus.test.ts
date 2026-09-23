import { vi, describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import type { WikiStatusEventPayload, WikiStatusPayload } from '../lib/tauri';

type EventCallback = (e: { payload: WikiStatusEventPayload }) => void;
let capturedCallback: EventCallback | null = null;
let pendingSnapshot: { resolve?: (v: WikiStatusPayload) => void } = {};

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockImplementation(
    (_event: string, cb: EventCallback) => {
      capturedCallback = cb;
      return Promise.resolve(() => { capturedCallback = null; });
    }
  ),
}));

vi.mock('../lib/tauri', () => ({
  subscribeEntityStatus: vi.fn().mockImplementation(
    (cb: EventCallback) => {
      capturedCallback = cb;
      return Promise.resolve(() => { capturedCallback = null; });
    },
  ),
  getWikiStatus: vi.fn().mockImplementation(
    () => new Promise<WikiStatusPayload>((resolve) => { pendingSnapshot.resolve = resolve; }),
  ),
}));

import { useWikiStatus } from '../hooks/useWikiStatus';

describe('useWikiStatus', () => {
  beforeEach(() => {
    capturedCallback = null;
    pendingSnapshot = {};
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
      diagnosticErrors: 0,
      diagnosticWarnings: 0,
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
      diagnosticErrors: 0,
      diagnosticWarnings: 0,
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
      diagnosticErrors: 0,
      diagnosticWarnings: 0,
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

  it('does not report degraded ingest as busy, so vault switching stays reachable', async () => {
    // A degraded pipeline is parked, not working: the watchdog exhausted its
    // respawn cap and nothing is in flight. `busy` gates switchVault, and the
    // bounded switch_vault is the documented recovery from this exact state —
    // gating it here would leave force-quitting as the only way out.
    const { result } = renderHook(() => useWikiStatus());

    await act(async () => {
      capturedCallback?.({
        payload: { ingest: 'degraded' },
      });
    });

    await waitFor(() => {
      expect(result.current.ingest).toBe('degraded');
      expect(result.current.busy).toBe(false);
    });
  });

  it('still reports stalled ingest as busy while the watchdog is recovering', async () => {
    const { result } = renderHook(() => useWikiStatus());

    await act(async () => {
      capturedCallback?.({
        payload: { ingest: 'stalled' },
      });
    });

    await waitFor(() => {
      expect(result.current.busy).toBe(true);
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

  it('carries diagnostic counts from the status event and keeps them on partial events', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { diagnosticErrors: 2, diagnosticWarnings: 5 } });
    });
    expect(result.current.diagnosticErrors).toBe(2);
    expect(result.current.diagnosticWarnings).toBe(5);
    await act(async () => {
      capturedCallback?.({ payload: { librarian: true } });
    });
    expect(result.current.diagnosticErrors).toBe(2);
  });

  it('does not let a late getWikiStatus snapshot overwrite newer event state', async () => {
    // Race: event arrives while the snapshot RPC is still in flight.
    // The hook must keep the event's state and ignore the snapshot, since
    // the snapshot was captured before the event was emitted (CodeRabbit
    // review PRRT_kwDOSVmXas6k-qeG).
    const { result } = renderHook(() => useWikiStatus());

    // Event wins first.
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
          diagnosticErrors: 4,
          diagnosticWarnings: 1,
        },
      });
    });
    expect(result.current.ingest).toBe('working');
    expect(result.current.diagnosticErrors).toBe(4);

    // Now resolve the snapshot with OLDER counters — must be ignored.
    await act(async () => {
      pendingSnapshot.resolve?.({
        ingest: 'idle',
        ingestStage: null,
        ingestSubject: null,
        librarian: false,
        healing: false,
        pruning: false,
        forgetting: false,
        diagnosticErrors: 0,
        diagnosticWarnings: 0,
      });
    });
    // Event state preserved.
    expect(result.current.ingest).toBe('working');
    expect(result.current.diagnosticErrors).toBe(4);
  });

  it('applies the getWikiStatus snapshot when no event has fired yet', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      pendingSnapshot.resolve?.({
        ingest: 'degraded',
        ingestStage: null,
        ingestSubject: null,
        librarian: true,
        healing: false,
        pruning: false,
        forgetting: false,
        diagnosticErrors: 7,
        diagnosticWarnings: 3,
      });
    });
    expect(result.current.ingest).toBe('degraded');
    expect(result.current.librarian).toBe(true);
    expect(result.current.diagnosticErrors).toBe(7);
  });
});
