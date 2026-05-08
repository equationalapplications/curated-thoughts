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
  start_line: number;
  end_line: number;
  symbol_name: string | null;
  strategy: string;
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

export interface ReviewPage {
  id: number;
  path: string;
  source_doc_ids: string;
  generated_by: string;
}

export const getReviewQueue = (): Promise<ReviewPage[]> =>
  invoke("get_review_queue");

export const approveWikiPage = (
  id: number,
  content: string
): Promise<void> =>
  invoke("approve_wiki_page", { id, content });

export const rejectWikiPage = (id: number): Promise<void> =>
  invoke("reject_wiki_page", { id });

export interface FolderRule {
  id: number;
  folder_path: string;
  librarian_mode: "index" | "summarize" | "synthesize";
  auto_approve: boolean;
}

export const getFolderRules = (): Promise<FolderRule[]> =>
  invoke("get_folder_rules");

export const setFolderRule = (
  folderPath: string,
  librarianMode: string,
  autoApprove: boolean
): Promise<void> =>
  invoke("set_folder_rule", { folderPath, librarianMode, autoApprove });

export const deleteFolderRule = (id: number): Promise<void> =>
  invoke("delete_folder_rule", { id });

export const getProposedContent = (pageId: number): Promise<string> =>
  invoke("get_proposed_content", { pageId });

export const saveWikiPage = (path: string, content: string): Promise<void> =>
  invoke("save_wiki_page", { path, content });

export const copyToVault = (srcPath: string): Promise<string> =>
  invoke("copy_to_vault", { srcPath });

export const deleteVaultFile = (path: string): Promise<void> =>
  invoke("delete_vault_file", { path });
