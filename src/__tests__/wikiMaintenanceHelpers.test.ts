import { vi, describe, it, expect, beforeEach } from 'vitest';

const { runOntologyBackfill, lint } = vi.hoisted(() => ({
  runOntologyBackfill: vi.fn(),
  lint: vi.fn(),
}));

vi.mock('@equationalapplications/react-llm-wiki', () => ({
  createWiki: vi.fn().mockReturnValue({
    setup: vi.fn().mockResolvedValue(undefined),
    runOntologyBackfill,
    lint,
  }),
  WikiBusyError: class WikiBusyError extends Error {},
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('../lib/wikiAdapter', () => ({ tauriWikiAdapter: {} }));

import { typeUntypedFacts, lintSeededTiers } from '../lib/wiki';

const result = (typed: number, remaining: number) => ({
  scanned: typed, typed, failedValidation: 0, edgesAdded: 0, remaining, deferred: 0,
});

describe('typeUntypedFacts', () => {
  beforeEach(() => vi.clearAllMocks());

  it('loops classifier auto per tier until remaining drains', async () => {
    runOntologyBackfill
      .mockResolvedValueOnce(result(5, 3))
      .mockResolvedValueOnce(result(3, 0))
      .mockResolvedValue(result(0, 0));
    const out = await typeUntypedFacts();
    expect(out).toEqual({ typed: 8, remaining: 0 });
    for (const call of runOntologyBackfill.mock.calls) {
      expect(call[1]).toEqual({ classifier: 'auto' });
    }
    // tier_fact twice, tier_wisdom once, workspace tier once.
    expect(runOntologyBackfill).toHaveBeenCalledTimes(4);
  });

  it('stops a tier on a no-progress pass instead of spinning', async () => {
    runOntologyBackfill.mockResolvedValueOnce(result(0, 7)).mockResolvedValue(result(0, 0));
    const out = await typeUntypedFacts();
    expect(out).toEqual({ typed: 0, remaining: 7 });
    expect(runOntologyBackfill).toHaveBeenCalledTimes(3);
  });
});

describe('lintSeededTiers', () => {
  it('lints every seeded tier', async () => {
    lint.mockResolvedValue({ danglingEdges: 1 });
    const out = await lintSeededTiers();
    expect(out.map((r) => r.entityId)).toEqual(['tier_fact', 'tier_wisdom', 'tier_working::default']);
    expect(out[0].report).toEqual({ danglingEdges: 1 });
  });
});
