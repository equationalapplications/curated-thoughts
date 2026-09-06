import { getChunkIdsForWikiEntry, getImpactRadius } from './tauri';

export interface GraphExpansionOptions {
  /** Maximum hops to walk from each seed chunk. Default: 1. Hard max enforced: 2. */
  hops?: 1 | 2;
  /** Include callees (dependencies) of seed chunks. Default: true. */
  includeCallees?: boolean;
  /** Include callers (impact radius) of seed chunks. Default: true. */
  includeCallers?: boolean;
  /** Maximum structural neighbors to inject per seed chunk. Default: 5. */
  neighborLimit?: number;
}

export interface GraphAdapter {
  getNeighbors(
    entryId: string,
    entityId: string,
    direction: 'callers' | 'callees' | 'both',
    maxHops: number,
  ): Promise<Array<{ chunkId: number; depth: number; relType: string }>>;
}

async function resolveRootChunkIds(entryId: string, entityId: string): Promise<number[]> {
  const chunkIds = await getChunkIdsForWikiEntry(entryId, entityId);
  return Array.isArray(chunkIds) ? chunkIds : [];
}

export const tauriGraphAdapter: GraphAdapter = {
  async getNeighbors(entryId, entityId, direction, maxHops) {
    const chunkIds = await resolveRootChunkIds(entryId, entityId);
    // No anchors resolved => no neighbors. Previously this fell through to
    // getImpactRadius(entryId, …), passing an entry id into a chunk-id
    // parameter — a wrong-namespace query, not a no-op. Spec §7.7.
    if (chunkIds.length === 0) return [];
    const rowSets = await Promise.all(
      chunkIds.map((chunkId) => getImpactRadius(chunkId, entityId, direction, maxHops)),
    );

    const merged = new Map<number, { chunk_id: number; depth: number; rel_type: string }>();
    for (const rows of rowSets) {
      for (const row of rows) {
        const existing = merged.get(row.chunk_id);
        if (!existing || row.depth < existing.depth) {
          merged.set(row.chunk_id, { chunk_id: row.chunk_id, depth: row.depth, rel_type: row.rel_type });
        }
      }
    }

    return Array.from(merged.values())
      .sort((a, b) => a.depth - b.depth)
      .map((row) => ({ chunkId: row.chunk_id, depth: row.depth, relType: row.rel_type }));
  },
};
