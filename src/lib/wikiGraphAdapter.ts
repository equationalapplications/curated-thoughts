import { getImpactRadius } from './tauri';

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
    rootChunkId: number,
    entityId: string,
    direction: 'callers' | 'callees' | 'both',
    maxHops: number,
  ): Promise<Array<{ chunkId: number; depth: number; relType: string }>>;
}

export const tauriGraphAdapter: GraphAdapter = {
  async getNeighbors(rootChunkId, entityId, direction, maxHops) {
    const rows = await getImpactRadius(rootChunkId, entityId, direction, maxHops);
    return rows.map(r => ({ chunkId: r.chunk_id, depth: r.depth, relType: r.rel_type }));
  },
};
