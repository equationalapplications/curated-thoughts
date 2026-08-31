import { vi, describe, it, expect, beforeEach } from 'vitest';

const { setOntologyManifest, runOntologyBackfill, wikiAdapterRunAsync } = vi.hoisted(() => ({
  setOntologyManifest: vi.fn().mockResolvedValue(undefined),
  runOntologyBackfill: vi.fn().mockResolvedValue({ remaining: 0, typed: 0, scanned: 0 }),
  wikiAdapterRunAsync: vi.fn().mockResolvedValue({ changes: 0, lastInsertRowId: 0 }),
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
  tauriWikiAdapter: { runAsync: wikiAdapterRunAsync },
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

  it('clears stale okf_type + manifest-derived edges for every tier before reseeding', async () => {
    await applyOntologyChange('schema-software-org');

    // Every tier gets an entries-clear, a tasks-clear, and an edges-delete,
    // each scoped by entity_id — runOntologyBackfill only fills okf_type IS
    // NULL rows, so switching between disjoint schemas would otherwise
    // leave stale classifications from the old manifest (spec D6 item 15).
    const clearedEntries = wikiAdapterRunAsync.mock.calls.filter(
      (c) => typeof c[0] === 'string' && c[0].includes('SET okf_type = NULL') && c[0].includes('entries'),
    );
    const clearedTasks = wikiAdapterRunAsync.mock.calls.filter(
      (c) => typeof c[0] === 'string' && c[0].includes('SET okf_type = NULL') && c[0].includes('tasks'),
    );
    const clearedEdges = wikiAdapterRunAsync.mock.calls.filter(
      (c) => typeof c[0] === 'string' && c[0].includes('DELETE FROM') && c[0].includes('edges'),
    );
    expect(clearedEntries).toHaveLength(3);
    expect(clearedTasks).toHaveLength(3);
    expect(clearedEdges).toHaveLength(3);
    // Clearing must happen before the corresponding setOntologyManifest
    // call for the same tier, or the fresh backfill classification could
    // race the clear and get wiped.
    const firstClearCallOrder = wikiAdapterRunAsync.mock.invocationCallOrder[0];
    const firstManifestCallOrder = setOntologyManifest.mock.invocationCallOrder[0];
    expect(firstClearCallOrder).toBeLessThan(firstManifestCallOrder);
  });

  it('rolls back every touched tier to the prior manifest when a later tier fails', async () => {
    // Entering this test, the cached selection is 'schema-software-org'
    // (left by the previous test) — switch to 'off' so this is a real
    // transition. tier_fact succeeds; tier_wisdom's setOntologyManifest
    // rejects.
    setOntologyManifest
      .mockResolvedValueOnce(undefined) // tier_fact: new ('off') manifest
      .mockRejectedValueOnce(new Error('manifest write failed')) // tier_wisdom: new manifest
      .mockResolvedValue(undefined); // rollback calls

    await expect(applyOntologyChange('off')).rejects.toThrow('manifest write failed');

    // Rollback re-applies the PRIOR manifest (schema-software-org, strict)
    // for both cleared tiers (tier_fact fully committed; tier_wisdom was
    // cleared before its failing setOntologyManifest call).
    const rollbackCalls = setOntologyManifest.mock.calls.slice(2);
    const rolledBackEntities = rollbackCalls.map((c) => c[0]);
    expect(rolledBackEntities).toEqual(['tier_fact', 'tier_wisdom']);
    for (const call of rollbackCalls) {
      expect(call[2]).toEqual({ mode: 'strict' }); // schema-software-org's mode
    }
  });
});
