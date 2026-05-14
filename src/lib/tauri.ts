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

export const startFileWatcher = (): Promise<void> =>
  invoke("start_file_watcher");

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
  // Phase 2: structural graph neighbors
  structural?: boolean;
  rel_type?: string;
}

export const searchVault = (query: string, limit = 10): Promise<SearchResult[]> =>
  invoke("search_vault", { query, limit });

export const getRelatedChunks = (docPath: string, limit = 5): Promise<SearchResult[]> =>
  invoke("get_related_chunks", { docPath, limit });

export const getStructuralNeighbors = (docPath: string, maxHops = 2): Promise<SearchResult[]> =>
  invoke("get_structural_neighbors", { docPath, maxHops });

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

export const deleteVaultFile = (path: string): Promise<void> =>
  invoke("delete_vault_file", { path });

export const switchVault = (newPath: string, restoreBackup: boolean): Promise<void> =>
  invoke("switch_vault", { newPath, restoreBackup });

export const backupVaultDb = (): Promise<string> =>
  invoke("backup_vault_db");

export const checkVaultBackup = (path: string): Promise<boolean> =>
  invoke("check_vault_backup", { path });

export const revealVault = (): Promise<void> =>
  invoke("reveal_vault");

export interface NeighborRow {
  chunk_id: number;
  depth: number;
  rel_type: string;
}

export const getChunkIdsForWikiEntry = (
  entryId: number,
  entityId: string,
): Promise<number[]> =>
  invoke('get_chunk_ids_for_wiki_entry', { entryId, entityId });

export const getImpactRadius = (
  rootChunkId: number,
  entityId: string,
  direction: 'callers' | 'callees' | 'both',
  maxHops: number,
): Promise<NeighborRow[]> =>
  invoke('get_impact_radius', { rootChunkId, entityId, direction, maxHops });
