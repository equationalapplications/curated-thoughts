import { vi, describe, it, expect, beforeEach } from 'vitest';

const { setOntologyManifest, runOntologyBackfill } = vi.hoisted(() => ({
  setOntologyManifest: vi.fn().mockResolvedValue(undefined),
  runOntologyBackfill: vi.fn().mockResolvedValue({ remaining: 0, typed: 0, scanned: 0 }),
}));

vi.mock('@equationalapplications/react-llm-wiki', () => ({
  createWiki: vi.fn().mockReturnValue({
    setup: vi.fn().mockResolvedValue(undefined),
    setOntologyManifest,
    runOntologyBackfill,
  }),
  WikiBusyError: class WikiBusyError extends Error {},
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((cmd: string) => {
    if (cmd === 'get_workspace_id') {
      return Promise.resolve('tier_working::abc123deadbeef01');
    }
    return Promise.resolve(undefined);
  }),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('../lib/wikiAdapter', () => ({
  tauriWikiAdapter: {},
}));

vi.mock('../hooks/useWikiStatus', () => ({
  useWikiStatus: vi.fn(),
}));

import { initWorkspaceId, applyOntologyChange } from '../lib/wiki';

describe('applyOntologyChange', () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    await initWorkspaceId('/Users/foo/Vault');
  });

  it('reseeds every tier and loops backfill until remaining drains', async () => {
    runOntologyBackfill
      .mockResolvedValueOnce({ remaining: 3, typed: 1, scanned: 4 })
      .mockResolvedValueOnce({ remaining: 1, typed: 1, scanned: 2 })
      .mockResolvedValue({ remaining: 0, typed: 1, scanned: 1 });

    await applyOntologyChange('schema-software-org');

    const seededEntities = setOntologyManifest.mock.calls.map((c) => c[0]);
    expect(seededEntities).toEqual([
      'tier_fact',
      'tier_wisdom',
      expect.stringMatching(/^tier_working::/),
    ]);
    // First tier ran 2 calls (remaining 3 → 1 → 0), remaining tiers ran 1 each.
    expect(runOntologyBackfill.mock.calls.length).toBeGreaterThanOrEqual(3);
  });

  it('passes an empty OntologyManifest with mode for selections without a schema', async () => {
    await applyOntologyChange('off');
    const firstCall = setOntologyManifest.mock.calls[0];
    expect(firstCall?.[1]).toEqual({ node_types: [], edge_types: [] });
    expect(firstCall?.[2]).toEqual({ mode: 'off' });
    expect(runOntologyBackfill).not.toHaveBeenCalled();
  });

  it('passes the published manifest with strict mode for schema selections', async () => {
    await applyOntologyChange('schema-org');
    const firstCall = setOntologyManifest.mock.calls[0];
    expect(firstCall?.[1]).not.toEqual({ node_types: [], edge_types: [] });
    expect(firstCall?.[2]).toEqual({ mode: 'strict' });
  });
});
