import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../lib/tauri', () => ({
  getChunkIdsForWikiEntry: vi.fn(),
  getImpactRadius: vi.fn(),
}));

import { tauriGraphAdapter } from '../lib/wikiGraphAdapter';
import { getChunkIdsForWikiEntry, getImpactRadius } from '../lib/tauri';

describe('tauriGraphAdapter', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('resolves wiki entry chunk IDs and merges duplicate impact-radius rows by minimum depth', async () => {
    vi.mocked(getChunkIdsForWikiEntry).mockResolvedValue([101, 102]);
    vi.mocked(getImpactRadius)
      .mockResolvedValueOnce([
        { chunk_id: 201, depth: 2, rel_type: 'CALLS' },
        { chunk_id: 202, depth: 3, rel_type: 'CALLS' },
      ])
      .mockResolvedValueOnce([
        { chunk_id: 201, depth: 1, rel_type: 'CALLS' },
        { chunk_id: 203, depth: 4, rel_type: 'IMPORTS' },
      ]);

    const result = await tauriGraphAdapter.getNeighbors('fact_abc', 'tier_fact', 'both', 3);

    expect(getChunkIdsForWikiEntry).toHaveBeenCalledWith('fact_abc', 'tier_fact');
    expect(getImpactRadius).toHaveBeenCalledTimes(2);
    expect(result).toEqual([
      { chunkId: 201, depth: 1, relType: 'CALLS' },
      { chunkId: 202, depth: 3, relType: 'CALLS' },
      { chunkId: 203, depth: 4, relType: 'IMPORTS' },
    ]);
  });

  it('returns no neighbors when no wiki entry chunk IDs resolve', async () => {
    vi.mocked(getChunkIdsForWikiEntry).mockResolvedValue([]);
    vi.mocked(getImpactRadius).mockResolvedValue([
      { chunk_id: 300, depth: 1, rel_type: 'CALLS' },
    ]);

    const result = await tauriGraphAdapter.getNeighbors('fact_ne', 'tier_fact', 'callers', 2);

    expect(getChunkIdsForWikiEntry).toHaveBeenCalledWith('fact_ne', 'tier_fact');
    // No anchors resolved => no neighbors. An entry id must never be passed
    // into getImpactRadius's chunk-id parameter (wrong-namespace query).
    expect(getImpactRadius).not.toHaveBeenCalled();
    expect(result).toEqual([]);
  });
});
