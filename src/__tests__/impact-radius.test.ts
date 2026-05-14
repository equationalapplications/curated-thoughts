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

    const result = await tauriGraphAdapter.getNeighbors(1, 'tier_fact', 'both', 3);

    expect(getChunkIdsForWikiEntry).toHaveBeenCalledWith(1, 'tier_fact');
    expect(getImpactRadius).toHaveBeenCalledTimes(2);
    expect(result).toEqual([
      { chunkId: 201, depth: 1, relType: 'CALLS' },
      { chunkId: 202, depth: 3, relType: 'CALLS' },
      { chunkId: 203, depth: 4, relType: 'IMPORTS' },
    ]);
  });

  it('falls back to the root chunk when no wiki entry chunk IDs exist', async () => {
    vi.mocked(getChunkIdsForWikiEntry).mockResolvedValue([]);
    vi.mocked(getImpactRadius).mockResolvedValue([
      { chunk_id: 300, depth: 1, rel_type: 'CALLS' },
    ]);

    const result = await tauriGraphAdapter.getNeighbors(5, 'tier_fact', 'callers', 2);

    expect(getChunkIdsForWikiEntry).toHaveBeenCalledWith(5, 'tier_fact');
    expect(getImpactRadius).toHaveBeenCalledWith(5, 'tier_fact', 'callers', 2);
    expect(result).toEqual([{ chunkId: 300, depth: 1, relType: 'CALLS' }]);
  });
});
