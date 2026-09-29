// src/__tests__/AppShell.dragdrop.test.tsx
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { getCurrentWindow } from '@tauri-apps/api/window';

// Explicit resolving stubs for exactly what AppShell + its hooks call at
// mount. NO Proxy — a Proxy mock answers `then` (thenable trap) and returns
// `undefined` for needsChunkHashMigration/startFileWatcher, whose .then
// calls throw during mount.
vi.mock('../lib/tauri', () => ({
  needsChunkHashMigration: vi.fn(() => Promise.resolve(false)),
  startFileWatcher: vi.fn(() => Promise.resolve(() => {})),
  // add stubs for anything else the mount trace touches — extend this
  // object, never switch to a Proxy.
  peekPendingConfigMalformed: vi.fn(() => Promise.resolve(null)),
  ackPendingConfigMalformed: vi.fn(() => Promise.resolve()),
  listProposals: vi.fn(() => Promise.resolve([])),
  getPrivacyMode: vi.fn(() => Promise.resolve('standard')),
  listEntities: vi.fn(() => Promise.resolve([])),
  getProviderConfig: vi.fn(() =>
    Promise.resolve({
      provider: 'ollama',
      model: 'llama3',
      base_url: 'http://localhost:11434',
      embedding_model: null,
      embedding_base_url: null,
    }),
  ),
  getIndexingStatus: vi.fn(() =>
    Promise.resolve({
      state: 'idle',
      pending: 0,
      processed: 0,
    }),
  ),
  getWikiStatus: vi.fn(() =>
    Promise.resolve({
      ingest: { state: 'idle' },
      librarian: false,
      healing: false,
      pruning: false,
      forgetting: false,
    }),
  ),
  subscribeEntityStatus: vi.fn(() => Promise.resolve(() => {})),
}));

import { AppShell } from '../components/shell/AppShell';

describe('AppShell drag-drop registration failure', () => {
  beforeEach(() => vi.clearAllMocks());

  it('logs exactly one warning and does not throw unhandled when onDragDropEvent rejects', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    // Mock at the SOURCE (getCurrentWindow), not on a captured object —
    // test-setup returns a fresh object per call, so per-object
    // mockReturnValueOnce never reaches the component.
    vi.mocked(getCurrentWindow).mockImplementation(() => ({
      onDragDropEvent: vi.fn(() => Promise.reject(new Error('no drag drop'))),
      setTitle: vi.fn(() => Promise.resolve()),
    }) as never);

    render(
      <AppShell
        vaultPath="/v"
        onVaultChanged={() => {}}
        needsSetup={false}
      />,
    );
    await new Promise((r) => setTimeout(r, 0));

    // Review m6: assert the COUNT on warns filtered by the prefix — a bare
    // toHaveBeenCalledWith does not prove "exactly one", and the
    // config-malformed drain can also warn (AppShell.tsx:232).
    const eventsWarns = warn.mock.calls.filter(([m]) =>
      String(m).startsWith('[events] listen failed'),
    );
    expect(eventsWarns).toHaveLength(1);
    expect(eventsWarns[0][0]).toBe('[events] listen failed (drag-drop)');
    expect(eventsWarns[0][1]).toBeInstanceOf(Error);
    warn.mockRestore();
  });
});
