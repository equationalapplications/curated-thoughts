import { createWiki, WikiBusyError, type WikiOptions } from "@equationalapplications/react-llm-wiki";
import type { GraphExpansionOptions } from './wikiGraphAdapter';
import { tauriGraphAdapter } from './wikiGraphAdapter';
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { tauriWikiAdapter } from "./wikiAdapter";
import { entityIdForPath } from "./wikiTiers";
import type { WikiStatusEventPayload } from "./tauri";

let _workspaceId: string = 'tier_working::default';
let _workspaceIdRequest = 0;

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

function makeWikiOptions(enableOutbox: boolean): WikiOptions & Record<string, unknown> {
  return {
    llmProvider: {
      async generateText({ systemPrompt, userPrompt }: { systemPrompt: string; userPrompt: string }) {
        return invoke<string>("ollama_generate", { systemPrompt, userPrompt });
      },
      async embed(text: string): Promise<number[]> {
        return invoke<number[]>("embed_text", { text });
      },
    },
    config: {
      hybridWeight: 0.7,
      preFilterLimit: 50,
      ...(enableOutbox && { enableOutbox: true }),
    },
    onRetrievalFallback: (err: Error) => {
      console.warn("[wiki] embed unavailable, using keyword search:", err.message);
    },
    graphAdapter: tauriGraphAdapter,
  } as WikiOptions & Record<string, unknown>;
}

// Initialized in setupWiki(). The live binding is updated before the app renders,
// so all callers that access `wiki` after setupWiki() resolves see the correct instance.
export let wiki = createWiki(tauriWikiAdapter, makeWikiOptions(false));

export async function setupWiki() {
  // Register worker lifecycle listeners before running the initial wiki setup.
  // This prevents a race where the worker starts or stops during setup and the
  // module keeps a stale wiki instance based on the earlier outbox status value.
  let wikiUpdateGeneration = 0;

  const startedUnlisten = await listen<void>('outbox-worker-started', async () => {
    const gen = ++wikiUpdateGeneration;
    const updatedWiki = createWiki(tauriWikiAdapter, makeWikiOptions(true));
    await updatedWiki.setup();
    if (gen !== wikiUpdateGeneration) return;
    wiki = updatedWiki;
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new Event('wiki-updated'));
    }
  });

  const stoppedUnlisten = await listen<void>('outbox-worker-stopped', async () => {
    const gen = ++wikiUpdateGeneration;
    const updatedWiki = createWiki(tauriWikiAdapter, makeWikiOptions(false));
    await updatedWiki.setup();
    if (gen !== wikiUpdateGeneration) return;
    wiki = updatedWiki;
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new Event('wiki-updated'));
    }
  });

  const effectiveOutboxEnabled = await invoke<boolean>('outbox_is_configured').catch(() => false);
  const newWiki = createWiki(tauriWikiAdapter, makeWikiOptions(effectiveOutboxEnabled));
  await newWiki.setup();
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
