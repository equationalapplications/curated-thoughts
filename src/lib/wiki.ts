import { createWiki } from "@equationalapplications/react-llm-wiki";
import { invoke } from "@tauri-apps/api/core";
import { tauriWikiAdapter } from "./wikiAdapter";

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
