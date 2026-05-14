import { vi, describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { WikiStatus } from '../hooks/useWikiStatus';

type EventCallback = (e: { payload: Omit<WikiStatus, 'busy'> }) => void;
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
      ingesting: false,
      librarian: false,
      healing: false,
      pruning: false,
      forgetting: false,
      busy: false,
      activeJob: 'idle',
      activeJobLabel: null,
    });
  });

  it('updates when wiki-status-change fires with ingesting true', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({
        payload: {
          ingesting: true,
          librarian: false,
          healing: false,
          pruning: false,
          forgetting: false,
        },
      });
    });
    expect(result.current).toEqual({
      ingesting: true,
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
          ingesting: false,
          librarian: false,
          healing: true,
          pruning: false,
          forgetting: false,
        },
      });
    });
    expect(result.current).toEqual({
      ingesting: false,
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
          ingesting: false,
          librarian: true,
          healing: false,
          pruning: false,
          forgetting: false,
        },
      });
    });
    const { ingesting, librarian, healing, pruning, forgetting } = result.current;
    expect(ingesting || librarian || healing || pruning || forgetting).toBe(true);
  });
});
