import { invoke } from "@tauri-apps/api/core";

export interface OllamaStatus {
  installed: boolean;
  running: boolean;
  models: string[];
}

export const getVaultPath = (): Promise<string | null> =>
  invoke("get_vault_path");

export const setVaultPath = (path: string): Promise<void> =>
  invoke("set_vault_path", { path });

export const checkOllama = (): Promise<OllamaStatus> =>
  invoke("check_ollama");

export const listLocalModels = (): Promise<string[]> =>
  invoke("list_local_models");

export const pullModel = (modelId: string): Promise<void> =>
  invoke("pull_model", { modelId });

export const startFileWatcher = (vaultPath: string): Promise<void> =>
  invoke("start_file_watcher", { vaultPath });

export const startOllamaServer = (): Promise<void> =>
  invoke("start_ollama_server");

export const getRecommendedModel = (): Promise<string> =>
  invoke("get_recommended_model");

export interface IndexingStatus {
  indexed: number;
  pending: number;
}

export const getIndexingStatus = (): Promise<IndexingStatus> =>
  invoke("get_indexing_status");

export interface SearchResult {
  doc_path: string;
  chunk_text: string;
  chunk_position: number;
  score: number;
}

export const searchVault = (query: string, limit = 10): Promise<SearchResult[]> =>
  invoke("search_vault", { query, limit });

export const getRelatedChunks = (docPath: string, limit = 5): Promise<SearchResult[]> =>
  invoke("get_related_chunks", { docPath, limit });

export interface VaultFile {
  path: string;
  name: string;
  tier: "user_doc" | "wiki";
}

export const listVaultFiles = (): Promise<VaultFile[]> =>
  invoke("list_vault_files");

export const readDocument = (path: string): Promise<string> =>
  invoke("read_document", { path });
