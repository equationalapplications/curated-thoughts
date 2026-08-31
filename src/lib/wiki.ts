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

/**
 * Spec D6: switching ontology invalidates typed classifications. Persists
 * the new selection, reseeds every tier, and loops backfill until the
 * engine reports no remaining work. Confirmation UX lives in the caller
 * (the wizard skips it because the first run has no prior data).
 *
 * Shared by the Settings panel and the setup wizard so the contract is the
 * same regardless of which surface the user switches from — every caller
 * triggers D6 once a wiki instance is available.
 */
export async function applyOntologyChange(next: OntologySelection): Promise<void> {
  // Update the cached selection so any outbox transition that fires DURING
  // this loop's awaits rebuilds the wiki with the new manifest, not the
  // pre-switch one. Without this, the started/stopped listeners close over
  // the stale `_ontologySelection` and the loop's later iterations run
  // `setOntologyManifest` against a wiki seeded from the prior selection
  // (spec D6 step 5: "Hot-swap the wiki instance on next outbox
  // transition").
  _ontologySelection = next;
  const mode = modeFor(next);
  const manifest = manifestFor(next) ?? EMPTY_MANIFEST;
  for (const entityId of seededOntologyEntityIds()) {
    await wiki.setOntologyManifest(entityId, manifest, { mode });
    // `off` does not classify facts and the engine reports `remaining === 0`
    // immediately; skip the loop to avoid the no-op round-trip.
    if (mode === "off") continue;
    let remaining = Infinity;
    while (remaining > 0) {
      const result = await wiki.runOntologyBackfill(entityId);
      remaining = result.remaining;
    }
  }
}

export async function initWorkspaceId(vaultPath: string): Promise<void> {
  const requestId = ++_workspaceIdRequest;
  const id = await invoke<string>('get_workspace_id', { path: vaultPath });
  if (requestId === _workspaceIdRequest) {
    _workspaceId = id;
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
      p.ingesting ||
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
