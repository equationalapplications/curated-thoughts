import { vi, describe, it, expect, beforeEach } from 'vitest';

vi.mock('@equationalapplications/react-llm-wiki', () => ({
  createWiki: vi.fn().mockReturnValue({
    setup: vi.fn().mockResolvedValue(undefined),
    read: vi.fn().mockResolvedValue({ facts: [] }),
    runHeal: vi.fn().mockResolvedValue(undefined),
  }),
  WikiBusyError: class WikiBusyError extends Error {},
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
import { initWorkspaceId, getWorkspaceId, tieredRead, startAutoHeal, getEntityRoutingForPath, wiki } from '../lib/wiki';

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

    const mockRead = vi.spyOn(wiki, 'read').mockResolvedValue({ facts: [] } as any);
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
  it('subscribes to vault-event and vault-file-changed events and returns cleanup', () => {
    const cleanup = startAutoHeal();
    expect(listen).toHaveBeenCalledWith('vault-event', expect.any(Function));
    expect(listen).toHaveBeenCalledWith('vault-file-changed', expect.any(Function));
    expect(typeof cleanup).toBe('function');
  });
});
