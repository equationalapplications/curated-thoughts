import { createWiki, WikiBusyError, type WikiOptions } from "@equationalapplications/react-llm-wiki";
import type { GraphExpansionOptions } from './wikiGraphAdapter';
import { tauriGraphAdapter } from './wikiGraphAdapter';
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { tauriWikiAdapter } from "./wikiAdapter";
import { entityIdForPath } from "./wikiTiers";
import { getOntologySelection, type OntologySelection, type WikiStatusEventPayload } from "./tauri";
import { manifestFor, modeFor, ontologyConfigFor } from "./ontology";

let _workspaceId: string = 'tier_working::default';
let _workspaceIdRequest = 0;
// Tracks the in-flight `initWorkspaceId` promise so callers like
// `applyOntologyChange` can wait for the workspace entity to resolve before
// iterating tiers. Without this, a setup-wizard click that fires before
// `initWorkspaceId` settles would reseed `tier_working::default` instead of
// the real workspace tier.
let _workspaceIdInflight: Promise<void> | null = null;

/**
 * Empty manifest for selections without a fixed schema (`off`, `emergent`).
 * core-llm-wiki 6.2.0 requires a non-null `OntologyManifest` on
 * `setOntologyManifest`; an empty manifest signals "no typed schema" rather
 * than "schema mismatch".
 */
const EMPTY_MANIFEST = { node_types: [] as never[], edge_types: [] as never[] };

/**
 * Every tier that carries a seeded manifest (spec D5). Exposed so the
 * shared `applyOntologyChange` helper can iterate without each caller
 * duplicating the list.
 */
export function seededOntologyEntityIds(): string[] {
  return ["tier_fact", "tier_wisdom", getWorkspaceId()];
}

// core-llm-wiki 6.2.0 defaults `config.tablePrefix` to `llm_wiki_`; the app
// never overrides it (see `makeWikiOptions` below — no `tablePrefix` key).
// `setOntologyManifest`/`runOntologyBackfill` have no "clear existing
// classifications" API — `runOntologyBackfill` only fills `okf_type IS
// NULL` rows and never overwrites one that is already set — so clearing
// stale classifications before a schema switch requires reaching past the
// package's public surface to the tables it owns. If a future core-llm-wiki
// release adds a table-prefix accessor or a native clear API, prefer that
// over this constant.
const WIKI_TABLE_PREFIX = "llm_wiki_";

/**
 * Null out `okf_type` on every live entry/task for `entityId` and drop its
 * manifest-derived edges (the `edges` table holds only ontology-classified
 * edges — see `edgeRepo.addIgnoreDuplicate`, never other kinds of links).
 *
 * Required because `runOntologyBackfill` is additive-only: without this,
 * switching between disjoint schemas (e.g. `schema-org` →
 * `schema-software-org`) would leave every previously-typed fact stamped
 * with a node/edge type that no longer exists in the new manifest (spec D6,
 * verification item 15). Switching **to** `off` also routes through this —
 * it clears without reclassifying (skips the backfill loop below).
 */
async function clearTierTypedData(entityId: string): Promise<void> {
  const p = WIKI_TABLE_PREFIX;
  await tauriWikiAdapter.runAsync(
    `UPDATE ${p}entries SET okf_type = NULL WHERE entity_id = ? AND okf_type IS NOT NULL`,
    [entityId],
  );
  await tauriWikiAdapter.runAsync(
    `UPDATE ${p}tasks SET okf_type = NULL WHERE entity_id = ? AND okf_type IS NOT NULL`,
    [entityId],
  );
  await tauriWikiAdapter.runAsync(
    `DELETE FROM ${p}edges WHERE entity_id = ?`,
    [entityId],
  );
}

/**
 * Spec D6: switching ontology invalidates typed classifications. Persists
 * the new selection, clears each tier's stale typed data and
 * manifest-derived edges, reseeds every tier, and loops backfill until the
 * engine reports no remaining work. Confirmation UX lives in the caller
 * (the wizard skips it because the first run has no prior data).
 *
 * Shared by the Settings panel and the setup wizard so the contract is the
 * same regardless of which surface the user switches from — every caller
 * triggers D6 once a wiki instance is available.
 *
 * Transactional across tiers: if a later tier's clear/reseed/backfill
 * fails, every tier already cleared for this attempt (including the one
 * that just failed) is rolled back to `prior`'s manifest before rethrowing,
 * so the cached selection and every tier's typed data stay mutually
 * consistent with whichever manifest is actually active.
 */
export async function applyOntologyChange(next: OntologySelection): Promise<void> {
  // Wait for any in-flight `initWorkspaceId` so the iteration sees a real
  // workspace id, not the seed `tier_working::default`. The settle is
  // idempotent: callers that already awaited init get a no-op, callers
  // racing init get the latest workspace id before the first
  // `setOntologyManifest` call.
  await _workspaceIdInflight;

  const prior = _ontologySelection;
  if (next === prior) return;

  const mode = modeFor(next);
  const manifest = manifestFor(next) ?? EMPTY_MANIFEST;
  const priorMode = modeFor(prior);
  const priorManifest = manifestFor(prior) ?? EMPTY_MANIFEST;
  // Every tier whose typed data has been cleared for this attempt — as soon
  // as a tier is cleared it needs a rollback path, even if the failure
  // happens on that same tier's reseed/backfill (the clear already ran).
  const clearedTiers: string[] = [];
  try {
    for (const entityId of seededOntologyEntityIds()) {
      await clearTierTypedData(entityId);
      clearedTiers.push(entityId);
      await wiki.setOntologyManifest(entityId, manifest, { mode });
      // `off` does not classify facts and the engine reports `remaining === 0`
      // immediately; skip the loop to avoid the no-op round-trip.
      if (mode !== "off") {
        let remaining = Infinity;
        while (remaining > 0) {
          const result = await wiki.runOntologyBackfill(entityId);
          remaining = result.remaining;
        }
      }
    }
    // All tiers committed: publish the new selection so any outbox
    // transition that fires from this point rebuilds the wiki with
    // `next`'s manifest (spec D6 step 5: "Hot-swap the wiki instance on
    // next outbox transition"). Persisting happens in the Tauri setter
    // before this helper runs, so on-disk and in-memory agree here.
    _ontologySelection = next;
  } catch (err) {
    // Roll back every tier already cleared for this attempt, then rethrow
    // so the caller can surface the error and (in the Settings panel)
    // restore the persisted selection. The wiki instance has not been
    // rebuilt, so the next read sees `prior`'s manifest once the rollback
    // completes. Rollback reclassifies from the just-cleared state via
    // backfill rather than restoring from a snapshot — the engine has no
    // snapshot/undo API for typed data.
    for (const entityId of clearedTiers) {
      try {
        await wiki.setOntologyManifest(entityId, priorManifest, { mode: priorMode });
        if (priorMode !== "off") {
          let remaining = Infinity;
          while (remaining > 0) {
            const result = await wiki.runOntologyBackfill(entityId);
            remaining = result.remaining;
          }
        }
      } catch (rollbackErr) {
        // Surface both: the original failure that triggered rollback
        // and the rollback failure itself, so the log captures why
        // state may be inconsistent.
        console.error(`[applyOntologyChange] rollback failed for ${entityId}:`, rollbackErr);
      }
    }
    throw err;
  }
}

export async function initWorkspaceId(vaultPath: string): Promise<void> {
  // Stash the in-flight promise so callers that race the resolve (e.g.
  // `applyOntologyChange` immediately after a wizard click) can await
  // the latest init instead of reading the `tier_working::default`
  // seed.
  const requestId = ++_workspaceIdRequest;
  const promise = (async () => {
    const id = await invoke<string>('get_workspace_id', { path: vaultPath });
    if (requestId === _workspaceIdRequest) {
      _workspaceId = id;
    }
  })();
  _workspaceIdInflight = promise;
  try {
    await promise;
  } finally {
    if (_workspaceIdInflight === promise) {
      _workspaceIdInflight = null;
    }
  }
}

export function getWorkspaceId(): string {
  return _workspaceId;
}

export function getEntityRoutingForPath(vaultRelativePath: string) {
  return entityIdForPath(vaultRelativePath, _workspaceId);
}

/**
 * Ingest any vault-relative path using canonical tier routing.
 * entityId is derived from the path: documents/ → tier_fact, wiki/ → tier_wisdom,
 * everything else → tier_working::<hash>. The package infers source_type from entityId.
 */
export async function ingestDocumentByPath(
  vaultRelativePath: string,
  params: {
    sourceRef: string;
    sourceHash: string;
    documentChunk: string;
    maxChunkLength?: number;
    chunkOverlap?: number;
    chunkConcurrency?: number;
  },
) {
  const { entityId } = getEntityRoutingForPath(vaultRelativePath);
  return wiki.ingestDocument(entityId, params);
}

function makeWikiOptions(enableOutbox: boolean, selection: OntologySelection): WikiOptions & Record<string, unknown> {
  return {
    llmProvider: {
      async generateText({ systemPrompt, userPrompt }: { systemPrompt: string; userPrompt: string }) {
        try {
          return await invoke<string>("generate_text", { systemPrompt, userPrompt });
        } catch (error) {
          const message = typeof error === "string"
            ? error
            : error instanceof Error
            ? error.message
            : String(error);
          if (message === "provider-not-ready") {
            throw new WikiBusyError("librarian", "provider-not-ready");
          }
          throw error;
        }
      },
      async embed(text: string): Promise<number[]> {
        return invoke<number[]>("embed_text", { text });
      },
    },
    config: {
      hybridWeight: 0.7,
      preFilterLimit: 50,
      ontology: ontologyConfigFor(selection, [
        'tier_fact',
        'tier_wisdom',
        getWorkspaceId(),
      ]),
      ...(enableOutbox && { enableOutbox: true }),
    },
    onRetrievalFallback: (err: Error) => {
      console.warn("[wiki] embed unavailable, using keyword search:", err.message);
    },
    graphAdapter: tauriGraphAdapter,
  } as WikiOptions & Record<string, unknown>;
}

// Desktop default until setupWiki() reads the persisted choice. The CLI writes
// its own default during --onboard, so an unreadable config here means a
// Desktop-first vault.
let _ontologySelection: OntologySelection = 'schema-org';

// Initialized in setupWiki(). The live binding is updated before the app renders,
// so all callers that access `wiki` after setupWiki() resolves see the correct instance.
export let wiki = createWiki(tauriWikiAdapter, makeWikiOptions(false, _ontologySelection));

export async function setupWiki() {
  // A rejected read must surface — silently defaulting to `schema-org`
  // would seed strict manifests even when the persisted selection is
  // `off` or `emergent`, leaving typed data misclassified.
  _ontologySelection = await getOntologySelection();

  // Register worker lifecycle listeners before running the initial wiki setup.
  // This prevents a race where the worker starts or stops during setup and the
  // module keeps a stale wiki instance based on the earlier outbox status value.
  let wikiUpdateGeneration = 0;

  const startedUnlisten = await listen<void>('outbox-worker-started', async () => {
    const gen = ++wikiUpdateGeneration;
    const updatedWiki = createWiki(tauriWikiAdapter, makeWikiOptions(true, _ontologySelection));
    await updatedWiki.setup();
    if (gen !== wikiUpdateGeneration) return;
    wiki = updatedWiki;
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new Event('wiki-updated'));
    }
  });

  const stoppedUnlisten = await listen<void>('outbox-worker-stopped', async () => {
    const gen = ++wikiUpdateGeneration;
    const updatedWiki = createWiki(tauriWikiAdapter, makeWikiOptions(false, _ontologySelection));
    await updatedWiki.setup();
    if (gen !== wikiUpdateGeneration) return;
    wiki = updatedWiki;
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new Event('wiki-updated'));
    }
  });

  const effectiveOutboxEnabled = await invoke<boolean>('outbox_is_configured').catch(() => false);
  let newWiki;
  try {
    newWiki = createWiki(tauriWikiAdapter, makeWikiOptions(effectiveOutboxEnabled, _ontologySelection));
    await newWiki.setup();
  } catch (e) {
    // No fallback to an untyped engine: running untyped is indistinguishable
    // from a deliberate "off" selection, so the failure must reach the user.
    const detail = e instanceof Error ? e.message : String(e);
    throw new Error(
      `Knowledge schema "${_ontologySelection}" failed to load: ${detail}`,
    );
  }
  if (wikiUpdateGeneration === 0) {
    wiki = newWiki;
  }

  // Store unlisten if you need cleanup; for now the listeners live for the session.
  void startedUnlisten;
  void stoppedUnlisten;
}

/** Tiered read: Facts (1.5×) > Wisdom (1.0×) > Working (0.6×). */
export async function tieredRead(
  query: string,
  opts: { graphExpansion?: GraphExpansionOptions } = {}
) {
  return wiki.read(
    ['tier_fact', 'tier_wisdom', _workspaceId],
    query,
    {
      tierWeights: {
        tier_fact:     1.5,
        tier_wisdom:   1.0,
        [_workspaceId]: 0.6,
      },
      // graphExpansion passed through; handled by host-app layer when supported
      ...(opts.graphExpansion !== undefined && { graphExpansion: opts.graphExpansion }),
    } as Parameters<typeof wiki.read>[2] & { graphExpansion?: GraphExpansionOptions }
  );
}

type VaultEventPayload = {
  kind: 'Added' | 'Modified' | 'Deleted';
  path: string;
};

export function startAutoHeal(): () => void {
  let debounce: ReturnType<typeof setTimeout> | null = null;
  let active = true;
  const scheduleHeal = () => {
    if (!active) return;
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(async () => {
      if (!active) return;
      try {
        await invoke('run_wiki_heal');
      } catch (err) {
        if (!(err instanceof WikiBusyError)) console.error('[auto-heal]', err);
      }
    }, 3000);
  };

  const unsubscribers = [
    listen<VaultEventPayload>('vault-event', (event) => {
      if (!active) return;
      if (event.payload.kind === 'Deleted') {
        scheduleHeal();
      }
    }),
  ];

  return () => {
    active = false;
    if (debounce) {
      clearTimeout(debounce);
      debounce = null;
    }
    void Promise.all(unsubscribers).then((fns) => fns.forEach((fn) => fn()));
  };
}

export function startAutoMaintenance(): () => void {
  let isBusy = false;
  const handleStatusChange = (event: { payload: WikiStatusEventPayload }) => {
    const p = event.payload;
    isBusy = !!(
      (p.ingest && p.ingest !== "idle") ||
      p.librarian ||
      (p.healing ?? p.heal) ||
      (p.pruning ?? p.prune) ||
      p.forgetting
    );
  };

  const runPrune = async () => {
    if (isBusy) {
      console.info('[auto-maintenance] skipping prune while system is busy');
      return;
    }

    try {
      await invoke('run_wiki_prune');
    } catch (err) {
      console.error('[auto-maintenance] prune failed', err);
    }
  };

  const unsubscribers = [listen('wiki-status-change', handleStatusChange)];

  // Run a prune once at startup, then every 24 hours.
  void runPrune();
  const interval = window.setInterval(runPrune, 24 * 60 * 60 * 1000);

  return () => {
    window.clearInterval(interval);
    void Promise.all(unsubscribers).then((fns) => fns.forEach((fn) => fn()));
  };
}

export { WikiBusyError };
