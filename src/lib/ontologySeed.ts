import type { OntologyManifest } from '@equationalapplications/core-llm-wiki';
import { manifestFor, modeFor } from './ontology';
import type { OntologySelection } from './tauri';

const EMPTY_MANIFEST: OntologyManifest = { node_types: [], edge_types: [] };

export type SeedOutcome =
  | { seeded: string[]; skipped: string[]; failed: false }
  | { seeded: []; skipped: []; failed: true; reason: string };

/** Minimal surface this needs from the wiki engine — keeps the unit test honest. */
interface SeedableWiki {
  getOntology(entityId: string): Promise<{ manifest: unknown | null }>;
  setOntologyManifest(
    entityId: string,
    manifest: OntologyManifest,
    opts: { mode: string },
  ): Promise<void>;
}

/**
 * Seed the ontology manifest for each entity id that has none (spec §2.1).
 *
 * All-or-nothing across the whole set (§2.2): core-llm-wiki 6.2.0 persists
 * seedManifests per-entity on first access and does not span several entities
 * in one transaction, so a partial set is reachable through its own path. On
 * any failure every manifest written by THIS call is reverted to the empty
 * manifest at mode off, which is the same state the brain was in before.
 *
 * Never throws. A failure degrades to mode off and is reported to the caller
 * for a health warning — it must not block ingest or wiki tools (PR #78).
 */
export async function seedManifestsIfAbsent(
  wiki: SeedableWiki,
  selection: OntologySelection,
  entityIds: string[],
): Promise<SeedOutcome> {
  const manifest = manifestFor(selection);
  const mode = modeFor(selection);
  if (!manifest) {
    // `emergent` and `off` seed no manifest; every id is a skip, not a failure.
    return { seeded: [], skipped: [...entityIds], failed: false };
  }

  const seeded: string[] = [];
  const skipped: string[] = [];
  try {
    for (const entityId of entityIds) {
      // Once-per-DB: a present manifest is never rewritten or duplicated.
      const current = await wiki.getOntology(entityId);
      if (current.manifest) {
        skipped.push(entityId);
        continue;
      }
      await wiki.setOntologyManifest(entityId, manifest, { mode });
      seeded.push(entityId);
    }
    return { seeded, skipped, failed: false };
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    for (const entityId of seeded) {
      try {
        await wiki.setOntologyManifest(entityId, EMPTY_MANIFEST, { mode: 'off' });
      } catch (rollbackErr) {
        // Surface both: the failure that triggered rollback and the rollback
        // failure, so the log captures why state may be inconsistent.
        console.error(`[seedManifestsIfAbsent] rollback failed for ${entityId}:`, rollbackErr);
      }
    }
    return { seeded: [], skipped: [], failed: true, reason };
  }
}
