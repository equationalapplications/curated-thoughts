import { describe, it, expect, vi } from 'vitest';
import { seedManifestsIfAbsent } from '../lib/ontologySeed';
import { manifestFor, modeFor } from '../lib/ontology';

type Entry = { entityId: string; manifest: unknown; mode?: string };

function fakeWiki(opts: { existing?: string[]; failOn?: string } = {}) {
  const existing = new Set(opts.existing ?? []);
  const written: string[] = [];
  return {
    written,
    setOntologyManifests: vi.fn(
      async (entries: Entry[], o?: { ifAbsent?: boolean }) => {
        // One transaction: a failing entry aborts the whole batch, so nothing
        // is recorded as written.
        if (opts.failOn && entries.some((e) => e.entityId === opts.failOn)) {
          throw new Error('write failed');
        }
        const skipped: string[] = [];
        const batch: string[] = [];
        for (const e of entries) {
          if (o?.ifAbsent && existing.has(e.entityId)) skipped.push(e.entityId);
          else batch.push(e.entityId);
        }
        written.push(...batch);
        return { written: batch, skipped };
      },
    ),
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
    expect(wiki.written).not.toContain('tier_fact');
  });

  it('passes ifAbsent so the check and the write are one atomic step', async () => {
    const wiki = fakeWiki();
    await seedManifestsIfAbsent(wiki as never, 'schema-software-org', ['tier_fact']);
    expect(wiki.setOntologyManifests).toHaveBeenCalledTimes(1);
    expect(wiki.setOntologyManifests).toHaveBeenCalledWith(
      [{ entityId: 'tier_fact', manifest: manifestFor('schema-software-org'), mode: modeFor('schema-software-org') }],
      { ifAbsent: true },
    );
  });

  it('leaves no partial set when the batch fails', async () => {
    const wiki = fakeWiki({ failOn: 'tier_wisdom' });
    const out = await seedManifestsIfAbsent(wiki as never, 'schema-software-org', ['tier_fact', 'tier_wisdom']);
    expect(out.failed).toBe(true);
    // The engine owns the transaction, so no compensating rollback is needed —
    // and tier_fact must never have been recorded as written.
    expect(wiki.written).toEqual([]);
    expect(wiki.setOntologyManifests).toHaveBeenCalledTimes(1);
  });

  it('never throws — a failure degrades to mode off (PR #78)', async () => {
    const wiki = fakeWiki({ failOn: 'tier_fact' });
    await expect(seedManifestsIfAbsent(wiki as never, 'schema-software-org', ['tier_fact'])).resolves.toMatchObject({ failed: true });
  });

  it('seeds nothing for a selection that carries no manifest', async () => {
    const wiki = fakeWiki();
    const out = await seedManifestsIfAbsent(wiki as never, 'off', ['tier_fact']);
    expect(out).toEqual({ seeded: [], skipped: ['tier_fact'], failed: false });
    expect(wiki.setOntologyManifests).not.toHaveBeenCalled();
  });

  /**
   * LATENT reachability pin (issue #158 audit, 2026-09-04). The single
   * `manifestFor(selection)` and `modeFor(selection)` result is reused for
   * every entity id in one batch — `seedManifestsIfAbsent` does not allow
   * per-id divergence. This is what makes
   * `db::commit::resolve_strict_edge_vocabulary`'s `Ok(_) if !is_last =>
   * continue` branch and the hardcoded `[entity_id, "tier_fact"]` lookup
   * order unreachable today: every partition sees the same effective
   * manifest, so any "absent" lookup result is functionally equivalent to
   * "non-strict", and the tier_fact fallback only matters as the canonical
   * seeded partition. The test below pins the shape — if a future change
   * adds per-id manifests here, it MUST also revisit the resolver.
   */
  it('passes the same manifest and mode to every entity id in one batch', async () => {
    const wiki = fakeWiki();
    await seedManifestsIfAbsent(wiki as never, 'schema-software-org', [
      'tier_fact',
      'tier_wisdom',
      'tier_working::late',
    ]);
    const call = vi.mocked(wiki.setOntologyManifests).mock.calls[0]!;
    const [entries, opts] = call as [Entry[], { ifAbsent?: boolean }];
    expect(opts?.ifAbsent).toBe(true);
    // Every entry points at the same object — structural identity, not
    // just deep equality. Per-id divergence is not expressible through
    // this surface today.
    const firstManifest = entries[0]!.manifest;
    const firstMode = entries[0]!.mode;
    for (const e of entries) {
      expect(e.manifest).toBe(firstManifest);
      expect(e.mode).toBe(firstMode);
    }
  });
});


/**
 * A wiki whose manifests **persist**, so calling `seedManifestsIfAbsent` again
 * models reopening the same brain rather than starting a fresh one.
 *
 * The fake stores what it is given and hands back a structural clone, so a
 * later mutation of the caller's object cannot make a stale snapshot look
 * stable. That is the failure AC2 is written to catch.
 */
function persistentWiki() {
  const store = new Map<string, { mode: string; manifest: unknown }>();
  let writes = 0;
  return {
    store,
    get writes() {
      return writes;
    },
    setOntologyManifests: async (entries: Entry[], o?: { ifAbsent?: boolean }) => {
      const written: string[] = [];
      const skipped: string[] = [];
      for (const e of entries) {
        if (o?.ifAbsent && store.has(e.entityId)) {
          skipped.push(e.entityId);
          continue;
        }
        writes += 1;
        store.set(e.entityId, {
          mode: e.mode as string,
          manifest: structuredClone(e.manifest),
        });
        written.push(e.entityId);
      }
      return { written, skipped };
    },
  };
}

/** The full persisted state, canonicalized for comparison. */
function snapshot(wiki: ReturnType<typeof persistentWiki>): string {
  return JSON.stringify(
    [...wiki.store.entries()].sort(([a], [b]) => a.localeCompare(b)),
  );
}

describe('AC2 — seeded manifests are stable across restarts', () => {
  const TIERS = ['tier_fact', 'tier_wisdom', 'tier_working::abc123'];

  it('compares the full manifest content equal after each of two restarts', async () => {
    const wiki = persistentWiki();

    // First seed, against the real shipped manifest rather than a stub — a
    // stub would not catch a package whose content shifts between reads.
    await seedManifestsIfAbsent(wiki as never, 'schema-software-org', TIERS);
    const afterFirstSeed = snapshot(wiki);
    const writesAfterFirstSeed = wiki.writes;

    // The snapshot must carry real content, or "compares equal" is vacuous.
    const seeded = wiki.store.get('tier_fact')!;
    const manifest = seeded.manifest as {
      node_types: { type: string }[];
      edge_types: { type: string }[];
    };
    expect(seeded.mode).toBe(modeFor('schema-software-org'));
    expect(manifest.node_types.length).toBeGreaterThan(0);
    expect(manifest.edge_types.length).toBeGreaterThan(0);
    expect(manifest).toEqual(manifestFor('schema-software-org'));

    // Restart twice. Each restart is a fresh call against the same store.
    for (const restart of [1, 2]) {
      await seedManifestsIfAbsent(wiki as never, 'schema-software-org', TIERS);
      expect(snapshot(wiki), `snapshot changed on restart ${restart}`).toBe(
        afterFirstSeed,
      );
      expect(
        wiki.writes,
        `restart ${restart} rewrote a manifest that was already present`,
      ).toBe(writesAfterFirstSeed);
    }
  });

  it('compares node ids, node types and edge types — not merely counts', async () => {
    // AC2 is explicit that row-count stability is insufficient: ids or types
    // can change while the count holds. This pins that the assertion above
    // would actually catch such a swap.
    const wiki = persistentWiki();
    await seedManifestsIfAbsent(wiki as never, 'schema-software-org', TIERS);
    const before = snapshot(wiki);

    const tampered = wiki.store.get('tier_fact')!;
    const manifest = tampered.manifest as { node_types: { type: string }[] };
    const originalCount = manifest.node_types.length;
    // Rename one node type in place: the count is identical, the content is not.
    manifest.node_types[0] = { ...manifest.node_types[0], type: 'renamed_type' };

    expect(manifest.node_types.length).toBe(originalCount);
    expect(snapshot(wiki)).not.toBe(before);
  });

  it('seeds a tier that appears only after the first seed', async () => {
    // The workspace tier's id is not known until `initWorkspaceId` resolves, so
    // it reaches the seed later than the stable tiers. It must still be seeded,
    // and the tiers already present must not be rewritten.
    const wiki = persistentWiki();
    await seedManifestsIfAbsent(wiki as never, 'schema-software-org', [
      'tier_fact',
      'tier_wisdom',
    ]);
    const writesBefore = wiki.writes;

    const out = await seedManifestsIfAbsent(wiki as never, 'schema-software-org', [
      'tier_working::late',
    ]);

    expect(out).toEqual({
      seeded: ['tier_working::late'],
      skipped: [],
      failed: false,
    });
    expect(wiki.writes).toBe(writesBefore + 1);
    expect(wiki.store.has('tier_working::late')).toBe(true);
  });
});
