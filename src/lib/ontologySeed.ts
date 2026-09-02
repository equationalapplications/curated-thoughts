import type { OntologyManifest, OntologyMode } from '@equationalapplications/core-llm-wiki';
import { manifestFor, modeFor } from './ontology';
import type { OntologySelection } from './tauri';

export type SeedOutcome =
  | { seeded: string[]; skipped: string[]; failed: false }
  | { seeded: []; skipped: []; failed: true; reason: string };

/** Minimal surface this needs from the wiki engine — keeps the unit test honest. */
interface SeedableWiki {
  setOntologyManifests(
    entries: Array<{ entityId: string; manifest: OntologyManifest; mode?: OntologyMode }>,
    opts?: { ifAbsent?: boolean },
  ): Promise<{ written: string[]; skipped: string[] }>;
}

/**
 * Seed the ontology manifest for each entity id that has none (spec §2.1).
 *
 * All-or-nothing across the set (§2.2): core-llm-wiki 6.3.0's
 * `setOntologyManifests` writes every entry in one transaction it owns, and
 * `ifAbsent` makes the check-and-write atomic, so the read-then-write race and
 * the compensating rollback this used to carry are both gone.
 *
 * Sharp edge inherited from the engine: `ifAbsent` tests for a PERSISTED row,
 * not for an effective manifest. CT also passes the same manifests through
 * `WikiConfig.ontology.seedManifests` (see `ontology.ts`), so an entity that has
 * only the configured seed and no row yet is reported in `seeded` rather than
 * `skipped`. The content written is identical, so this materializes the row it
 * would otherwise have gained on first ingest.
 *
 * Never throws. A failure is reported to the caller for a health warning — it
 * must not block ingest or wiki tools (PR #78).
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

  try {
    const { written, skipped } = await wiki.setOntologyManifests(
      entityIds.map((entityId) => ({ entityId, manifest, mode })),
      { ifAbsent: true },
    );
    return { seeded: written, skipped, failed: false };
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    return { seeded: [], skipped: [], failed: true, reason };
  }
}
