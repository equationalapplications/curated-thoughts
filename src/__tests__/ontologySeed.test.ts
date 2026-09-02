import { describe, it, expect, vi } from 'vitest';
import { seedManifestsIfAbsent } from '../lib/ontologySeed';

function fakeWiki(opts: { existing?: string[]; failOn?: string } = {}) {
  const existing = new Set(opts.existing ?? []);
  const written: string[] = [];
  return {
    written,
    getOntology: vi.fn(async (entityId: string) =>
      existing.has(entityId) ? { mode: 'strict', manifest: { node_types: ['Doc'], edge_types: ['CITES'] } } : { mode: 'off', manifest: null },
    ),
    setOntologyManifest: vi.fn(async (entityId: string) => {
      if (opts.failOn === entityId) throw new Error('write failed');
      written.push(entityId);
    }),
  };
}

describe('seedManifestsIfAbsent', () => {
  it('seeds every entity id when no manifest is present', async () => {
    const wiki = fakeWiki();
    const out = await seedManifestsIfAbsent(wiki as never, 'schema-software-org', ['tier_fact', 'tier_wisdom', 'ws1']);
    expect(out).toEqual({ seeded: ['tier_fact', 'tier_wisdom', 'ws1'], skipped: [], failed: false });
  });

  it('is once-per-DB: an entity that already has a manifest is skipped, not rewritten', async () => {
    const wiki = fakeWiki({ existing: ['tier_fact'] });
    const out = await seedManifestsIfAbsent(wiki as never, 'schema-software-org', ['tier_fact', 'tier_wisdom']);
    expect(out).toEqual({ seeded: ['tier_wisdom'], skipped: ['tier_fact'], failed: false });
    expect(wiki.setOntologyManifest).not.toHaveBeenCalledWith('tier_fact', expect.anything(), expect.anything());
  });

  it('rolls back every write when one entity fails, leaving no partial set', async () => {
    const wiki = fakeWiki({ failOn: 'tier_wisdom' });
    const out = await seedManifestsIfAbsent(wiki as never, 'schema-software-org', ['tier_fact', 'tier_wisdom']);
    expect(out.failed).toBe(true);
    // tier_fact was written then rolled back to the empty manifest
    expect(wiki.setOntologyManifest).toHaveBeenCalledWith('tier_fact', { node_types: [], edge_types: [] }, { mode: 'off' });
  });

  it('never throws — a failure degrades to mode off (PR #78)', async () => {
    const wiki = fakeWiki({ failOn: 'tier_fact' });
    await expect(seedManifestsIfAbsent(wiki as never, 'schema-software-org', ['tier_fact'])).resolves.toMatchObject({ failed: true });
  });

  it('seeds nothing for a selection that carries no manifest', async () => {
    const wiki = fakeWiki();
    const out = await seedManifestsIfAbsent(wiki as never, 'off', ['tier_fact']);
    expect(out).toEqual({ seeded: [], skipped: ['tier_fact'], failed: false });
    expect(wiki.setOntologyManifest).not.toHaveBeenCalled();
  });
});
