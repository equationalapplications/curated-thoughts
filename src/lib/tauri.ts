import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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

export const runWikiReindex = (): Promise<number> =>
  invoke("run_wiki_reindex");

export const startOllamaServer = (): Promise<void> =>
  invoke("start_ollama_server");

export const getRecommendedModel = (): Promise<string> =>
  invoke("get_recommended_model");

export const getBrainDir = (): Promise<string> => invoke("get_brain_dir");
export const getBinaryPath = (): Promise<string> => invoke("get_binary_path");

export interface GenerationConfig {
  provider: "unconfigured" | "sidecar" | "external";
  model_path: string | null;
  model_name: string | null;
  external_url: string | null;
  api_key: string | null;
}

export interface LlmConfig {
  generation: GenerationConfig;
  embedding: {
    provider: "fastembed" | "external";
    external_url: string | null;
  };
}

export const getProviderConfig = (): Promise<LlmConfig> =>
  invoke("get_provider_config");

export const updateProvider = (config: GenerationConfig): Promise<void> =>
  invoke("update_provider", { config });

export const initFastembed = (): Promise<void> => invoke("init_fastembed");

export const downloadSidecarEngine = (): Promise<void> =>
  invoke("download_sidecar_engine");

export const downloadModelWeights = (
  url: string,
  filename: string,
  expectedSha256: string,
): Promise<void> =>
  invoke("download_model_weights", { url, filename, expectedSha256 });

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
  // Phase 3: authoritative tier from chunks.entity_id (tier_fact | tier_wisdom | tier_working)
  entity_id?: string;
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

export interface ProposalItemCounts {
  total: number;
  facts: number;
  edges: number;
  tasks: number;
  summary_updates: number;
}

export interface ProposalSummary {
  id: string;
  kind: "new_entity" | "update_entity";
  target_name: string;
  entity_id?: string | null;
  source_doc_paths: string[];
  item_counts: ProposalItemCounts;
  created_at: number;
  age_secs: number;
  model: string;
}

export interface HydratedEvidenceChunk {
  chunk_id: number;
  quote: string;
  start_line: number;
  end_line: number;
  doc_path?: string | null;
  source_deleted: boolean;
}

export interface ProposalItem {
  id: string;
  item_type: string;
  target_id?: string | null;
  payload: Record<string, unknown>;
  evidence: HydratedEvidenceChunk[];
  status: string;
  edited_payload?: Record<string, unknown> | null;
}

export interface ProposalDetail {
  id: string;
  kind: "new_entity" | "update_entity";
  entity_id?: string | null;
  proposed_name?: string | null;
  proposed_type?: string | null;
  target_name: string;
  reasoning?: string | null;
  model: string;
  status: string;
  created_at: number;
  source_doc_paths: string[];
  items: ProposalItem[];
}

export interface ItemDecision {
  item_id: string;
  decision: "accept" | "reject";
  edited_payload?: Record<string, unknown> | null;
}

export interface CommitResult {
  committed: { item_id: string; table: string; record_id: string }[];
  conflicts: string[];
  dropped_edges: string[];
  proposal_status: string;
}

export const listProposals = (filter?: { status?: string }): Promise<ProposalSummary[]> =>
  invoke("list_proposals_cmd", { filter: filter ?? {} });

export const getProposalDetail = (proposalId: string): Promise<ProposalDetail | null> =>
  invoke("get_proposal_detail_cmd", { proposalId });

export const resolveProposal = (
  proposalId: string,
  decisions: ItemDecision[],
  rejectReason?: string,
  autoApprove?: boolean,
): Promise<CommitResult> =>
  invoke("resolve_proposal_cmd", {
    proposalId,
    decisions,
    rejectReason: rejectReason ?? null,
    autoApprove: autoApprove ?? false,
  });

export type EntitySort = "updated_desc" | "name_asc" | "name_desc" | "created_desc";

export interface EntityListFilter {
  entity_type?: string | null;
  include_archived?: boolean | null;
}

export interface EntitySummary {
  id: string;
  name: string;
  entity_type: string;
  summary_snippet: string;
  fact_count: number;
  open_task_count: number;
  created_at: number;
  updated_at: number;
}

export interface EntityFact {
  id: string;
  title: string;
  body: string;
  tags: string[];
  confidence: string;
  source_type: string;
  updated_at: number;
}

export interface EntityTask {
  id: string;
  description: string;
  status: string;
  priority: number;
  created_at: number;
}

export interface EntityEvent {
  id: string;
  event_type: string;
  summary: string;
  related_entry_id?: string | null;
  created_at: number;
}

export interface EntityDetail {
  id: string;
  name: string;
  entity_type: string;
  summary: string;
  created_at: number;
  updated_at: number;
  deleted_at?: number | null;
  facts: EntityFact[];
  tasks: EntityTask[];
  events: EntityEvent[];
}

export interface CreateEntityInput {
  name: string;
  entity_type?: string | null;
  summary?: string | null;
}

export const listEntities = (
  sort?: EntitySort,
  filter?: EntityListFilter,
): Promise<EntitySummary[]> =>
  invoke("list_entities_cmd", { sort: sort ?? null, filter: filter ?? {} });

export const getEntity = (entityId: string): Promise<EntityDetail | null> =>
  invoke("get_entity_cmd", { entityId });

export const createEntity = (input: CreateEntityInput): Promise<EntityDetail> =>
  invoke("create_entity_cmd", { input });

export const updateEntitySummary = (
  entityId: string,
  summary: string,
): Promise<void> =>
  invoke("update_entity_summary_cmd", { entityId, summary });

export const archiveEntity = (entityId: string): Promise<void> =>
  invoke("archive_entity_cmd", { entityId });

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

export interface WikiStatusPayload {
  ingesting: boolean;
  librarian: boolean;
  healing: boolean;
  pruning: boolean;
  forgetting: boolean;
}

export type WikiStatusEventPayload = Partial<WikiStatusPayload> & {
  heal?: boolean;
  prune?: boolean;
};

export const subscribeEntityStatus = (
  callback: (event: { payload: WikiStatusEventPayload }) => void,
): Promise<UnlistenFn> => listen<WikiStatusEventPayload>('wiki-status-change', callback);

export const runWikiHeal = (): Promise<void> => invoke('run_wiki_heal');
export const runWikiPrune = (): Promise<void> => invoke('run_wiki_prune');
export const runWikiReembed = (): Promise<number> => invoke('run_wiki_reembed');
export const forgetWikiSource = (sourcePath: string): Promise<void> =>
  invoke('run_wiki_forget', { sourcePath });

export interface CloudBridgeStatus {
  configured: boolean;
  connection_status:
    | 'disconnected'
    | 'connecting'
    | 'authenticating'
    | 'connected'
    | 'reconnecting'
    | 'auth_rejected';
}

export const setCloudBridgePairingToken = (token: string): Promise<void> =>
  invoke('set_cloud_bridge_pairing_token', { token });

export const clearCloudBridgePairingToken = (): Promise<void> =>
  invoke('clear_cloud_bridge_pairing_token');

export const getCloudBridgeStatus = (): Promise<CloudBridgeStatus> =>
  invoke('get_cloud_bridge_status');

export const retryCloudBridgeNow = (): Promise<void> =>
  invoke('retry_cloud_bridge_now');

export interface OkfExportSummary {
  path: string;
  entities: number;
  files: number;
}

export interface OkfEntityImportPreview {
  entity_id: string;
  name: string;
  entity_exists: boolean;
  facts_new: number;
  facts_existing: number;
  tasks_new: number;
  tasks_existing: number;
  edges_total: number;
  events_new: number;
  events_duplicate: number;
  summary_action: string;
}

export interface OkfImportPreview {
  profile: string | null;
  entities: OkfEntityImportPreview[];
  warnings: string[];
}

export interface OkfImportResult {
  entities_touched: number;
  facts_added: number;
  facts_skipped: number;
  tasks_added: number;
  tasks_skipped: number;
  edges_added: number;
  events_added: number;
  events_skipped: number;
}

export type OkfImportMode = "merge" | "replace" | "clone";

export const exportOkfBundle = (
  destPath: string,
  entityIds: string[] | null = null,
): Promise<OkfExportSummary> =>
  invoke("okf_export_bundle_cmd", { destPath, entityIds });

export const previewOkfImport = (
  srcPath: string,
  mode: OkfImportMode,
): Promise<OkfImportPreview> =>
  invoke("okf_import_preview_cmd", { srcPath, mode });

export const applyOkfImport = (
  srcPath: string,
  mode: OkfImportMode,
): Promise<OkfImportResult> =>
  invoke("okf_import_apply_cmd", { srcPath, mode });
