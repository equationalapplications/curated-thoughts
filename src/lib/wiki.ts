import { createWiki, WikiBusyError } from "@equationalapplications/react-llm-wiki";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { tauriWikiAdapter } from "./wikiAdapter";
import { entityIdForPath } from "./wikiTiers";

let _workspaceId: string = 'tier_working::default';

export async function initWorkspaceId(vaultPath: string): Promise<void> {
  _workspaceId = await invoke<string>('get_workspace_id', { path: vaultPath });
}

export function getWorkspaceId(): string {
  return _workspaceId;
}

export function getEntityRoutingForPath(vaultRelativePath: string) {
  return entityIdForPath(vaultRelativePath, _workspaceId);
}

/**
 * Ingest a vault-relative documents/ path using canonical tier routing.
 * This helper should be the public path for document ingestion.
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
  if (entityId !== "tier_fact") {
    throw new Error(
      `ingestDocumentByPath only supports documents/ paths, got ${vaultRelativePath}`
    );
  }
  return wiki.ingestDocument(entityId, params);
}

export const wiki = createWiki(tauriWikiAdapter, {
  llmProvider: {
    async generateText({ systemPrompt, userPrompt }) {
      return invoke<string>("ollama_generate", { systemPrompt, userPrompt });
    },
    async embed(text: string): Promise<number[]> {
      return invoke<number[]>("embed_text", { text });
    },
  },
  config: {
    hybridWeight: 0.7,
    preFilterLimit: 50,
  },
  onRetrievalFallback: (err) => {
    console.warn("[wiki] embed unavailable, using keyword search:", err.message);
  },
});

export async function setupWiki() {
  await wiki.setup();
}

/** Tiered read: Facts (1.5×) > Wisdom (1.0×) > Working (0.6×). */
export async function tieredRead(query: string) {
  return wiki.read(
    ['tier_fact', 'tier_wisdom', _workspaceId],
    query,
    {
      tierWeights: {
        tier_fact: 1.5,
        tier_wisdom: 1.0,
        [_workspaceId]: 0.6,
      },
    }
  );
}

export function startAutoHeal(): () => void {
  let debounce: ReturnType<typeof setTimeout> | null = null;
  const scheduleHeal = () => {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(async () => {
      try {
        await wiki.runHeal('tier_fact');
        await wiki.runHeal('tier_wisdom');
        await wiki.runHeal(_workspaceId);
      } catch (err) {
        if (!(err instanceof WikiBusyError)) console.error('[auto-heal]', err);
      }
    }, 3000);
  };

  const unsubscribers = [
    listen('vault-event', scheduleHeal),
    listen('vault-file-changed', scheduleHeal),
  ];

  return () => {
    if (debounce) {
      clearTimeout(debounce);
      debounce = null;
    }
    void Promise.all(unsubscribers).then((fns) => fns.forEach((fn) => fn()));
  };
}

export { WikiBusyError };
