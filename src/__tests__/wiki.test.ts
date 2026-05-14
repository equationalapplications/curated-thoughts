import * as React from 'react';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@equationalapplications/react-llm-wiki', () => ({
  createWiki: vi.fn().mockReturnValue({
    setup: vi.fn().mockResolvedValue(undefined),
    read: vi.fn().mockResolvedValue({ facts: [] }),
    runHeal: vi.fn().mockResolvedValue(undefined),
  }),
  WikiBusyError: class WikiBusyError extends Error {},
}));

vi.mock('../hooks/useWikiStatus', () => ({
  useWikiStatus: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('../lib/wikiAdapter', () => ({
  tauriWikiAdapter: {},
}));

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useWikiStatus } from '../hooks/useWikiStatus';
import { VaultPanel } from '../components/settings/VaultPanel';
import { initWorkspaceId, getWorkspaceId, tieredRead, startAutoHeal, getEntityRoutingForPath, wiki } from '../lib/wiki';
import { runWikiReindex } from '../lib/tauri';

describe('initWorkspaceId', () => {
  beforeEach(() => vi.clearAllMocks());

  it('calls get_workspace_id Tauri command with vault path', async () => {
    vi.mocked(invoke).mockResolvedValue('tier_working::abc123deadbeef01');
    await initWorkspaceId('/Users/foo/Vault');
    expect(invoke).toHaveBeenCalledWith('get_workspace_id', { path: '/Users/foo/Vault' });
  });

  it('updates getWorkspaceId() after init', async () => {
    vi.mocked(invoke).mockResolvedValue('tier_working::abc123deadbeef01');
    await initWorkspaceId('/Users/foo/Vault');
    expect(getWorkspaceId()).toBe('tier_working::abc123deadbeef01');
  });
});

describe('tieredRead', () => {
  beforeEach(() => vi.clearAllMocks());

  it('calls wiki.read with all three tier IDs and correct weights', async () => {
    vi.mocked(invoke).mockResolvedValue('tier_working::abc123deadbeef01');
    await initWorkspaceId('/Users/foo/Vault');

    const mockRead = vi.spyOn(wiki, 'read').mockResolvedValue({ facts: [], tasks: [], events: [] });
    await tieredRead('test query');

    expect(mockRead).toHaveBeenCalledWith(
      ['tier_fact', 'tier_wisdom', 'tier_working::abc123deadbeef01'],
      'test query',
      {
        tierWeights: {
          tier_fact: 1.5,
          tier_wisdom: 1.0,
          'tier_working::abc123deadbeef01': 0.6,
        },
      }
    );
  });

  it('forwards graphExpansion option when provided', async () => {
    await tieredRead('test query', { graphExpansion: { hops: 1 } });
    expect(wiki.read).toHaveBeenCalledWith(
      expect.any(Array),
      'test query',
      expect.objectContaining({ graphExpansion: { hops: 1 } })
    );
  });

  it('routes vault-relative paths through entityIdForPath for ingestion routing', () => {
    vi.mocked(invoke).mockResolvedValue('tier_working::abc123deadbeef01');
    return initWorkspaceId('/Users/foo/Vault').then(() => {
      expect(getEntityRoutingForPath('documents/api-ref.md')).toEqual({
        entityId: 'tier_fact',
        sourceType: 'immutable_document',
      });
      expect(getEntityRoutingForPath('wiki/auth-patterns.md')).toEqual({
        entityId: 'tier_wisdom',
        sourceType: 'user_confirmed',
      });
      expect(getEntityRoutingForPath('src/db/init.rs')).toEqual({
        entityId: 'tier_working::abc123deadbeef01',
        sourceType: 'librarian_inferred',
      });
    });
  });
});

describe('startAutoHeal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('subscribes to vault-event and returns cleanup', () => {
    const cleanup = startAutoHeal();
    expect(listen).toHaveBeenCalledWith('vault-event', expect.any(Function));
    expect(typeof cleanup).toBe('function');
  });

  it('invokes the Rust auto-heal command only for deleted vault events', async () => {
    vi.useFakeTimers();
    vi.mocked(invoke).mockResolvedValue(undefined);

    const cleanup = startAutoHeal();
    const callback = vi.mocked(listen).mock.calls[0][1] as (event: { payload: { kind?: string } }) => void;

    callback({ payload: { kind: 'Modified' } });
    await vi.advanceTimersByTimeAsync(3000);
    expect(vi.mocked(invoke)).not.toHaveBeenCalled();

    callback({ payload: { kind: 'Deleted' } });
    await vi.advanceTimersByTimeAsync(3000);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('run_wiki_heal');

    cleanup();
    vi.useRealTimers();
  });
});

describe('VaultPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(useWikiStatus).mockReturnValue({
      ingesting: false,
      librarian: false,
      heal: false,
      prune: true,
    });
  });

  it('blocks Change vault when a prune job is active', () => {
    render(React.createElement(VaultPanel, { vaultPath: '/Users/test/vault' }));
    expect(screen.getByRole('button', { name: /Change vault/i })).toBeDisabled();
  });
});

describe('runWikiReindex', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('forwards the command to Tauri', async () => {
    vi.mocked(invoke).mockResolvedValue(7);
    const result = await runWikiReindex();
    expect(invoke).toHaveBeenCalledWith('run_wiki_reindex');
    expect(result).toBe(7);
  });
});
