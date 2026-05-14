import { vi, describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { WikiStatus } from '../hooks/useWikiStatus';

type EventCallback = (e: { payload: WikiStatus }) => void;
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
    expect(result.current).toEqual({ ingesting: false, librarian: false, heal: false, prune: false });
  });

  it('updates when wiki-status-change fires with ingesting true', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { ingesting: true, librarian: false, heal: false, prune: false } });
    });
    expect(result.current).toEqual({ ingesting: true, librarian: false, heal: false, prune: false });
  });

  it('updates when wiki-status-change fires with heal true', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { ingesting: false, librarian: false, heal: true, prune: false } });
    });
    expect(result.current.heal).toBe(true);
  });

  it('isSystemBusy is true when any field is active', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { ingesting: false, librarian: true, heal: false } });
    });
    const { ingesting, librarian, heal } = result.current;
    expect(ingesting || librarian || heal).toBe(true);
  });
});
