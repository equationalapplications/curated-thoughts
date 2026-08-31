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

export const getVaultLayout = (): Promise<{
  immutableDir: string;
  wikiDir: string;
  labels: {
    immutableDir: string;
    wikiDir: string;
  };
}> => invoke("get_vault_layout");

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

/** Returns the resolved brain dir plus the source that produced it
 * (env override vs. home-dir default) so the AgentIntegrationPanel can
 * display a status line. */
export interface BrainDirInfo {
  brain_dir: string;
  source: "env" | "default";
}

export const getBrainDirInfo = (): Promise<BrainDirInfo> =>
  invoke("get_brain_dir_info");

export const getBinaryPath = (): Promise<string> => invoke("get_binary_path");

export const resolveChunkOverlay = (
  path: string,
  hash: string,
): Promise<{ startLine: number; endLine: number } | null> =>
  invoke("resolve_chunk_overlay", { path, hash });

/** Phase 8 Plan B: fetch the raw text of the chunk identified by
 * `(path, hash)` for the source-peek panel. Resolves `null` when the
 * hash no longer resolves ("source moved"); rejects on backend failure. */
export const fetchChunkContent = (path: string, hash: string): Promise<string | null> =>
  invoke("fetch_chunk_content", { path, hash });

/** Phase 9: gate query for the one-time chunk-hash migration. Returns
 * `true` while at least one chunk row lacks `content_hash` (i.e. the
 * migration still has work to do and the splash should mount). Mirrors
 * the check the backend uses internally in `lib.rs` to dispatch
 * `run_chunk_hash_migration` at startup. */
export const needsChunkHashMigration = (): Promise<boolean> =>
  invoke("needs_chunk_hash_migration");

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

/** Payload of the `config-malformed` event emitted by the desktop startup
 * hook when `BrainConfig::load_lenient` reports fatal diagnostics. */
export interface ConfigMalformedPayload {
  config_path: string;
  diagnostics: string[];
  remediation: string;
}

/** Non-destructive read of the most recent `config-malformed` payload
 * stashed by the setup hook. Called on `AppShell` mount because the setup
 * thread can emit the event before the React listener registers (Tauri
 * does not buffer events) — see `PendingConfigMalformed` in
 * `src-tauri/src/lib.rs`. Resolves to `null` if the startup hook never
 * produced a malformed-config report. Pairs with
 * `ackPendingConfigMalformed`, which the caller MUST invoke after
 * successfully rendering the payload — CodeRabbit #21, PR #120. The
 * earlier destructive `takePendingConfigMalformed` dropped the payload
 * whenever cleanup ran before the IPC `.then` resolved. */
export const peekPendingConfigMalformed = (): Promise<ConfigMalformedPayload | null> =>
  invoke("peek_pending_config_malformed");

/** Clears the stashed `config-malformed` payload after the caller has
 * successfully rendered it — pairs with `peekPendingConfigMalformed`.
 * No-op if no payload is pending. Safe to call multiple times. */
export const ackPendingConfigMalformed = (): Promise<void> =>
  invoke("ack_pending_config_malformed");

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

export interface SourceDocRef {
  path: string;
  chunkId: string | null;
}

export interface EntityFact {
  id: string;
  title: string;
  body: string;
  tags: string[];
  confidence: string;
  source_type: string;
  source_docs: SourceDocRef[];
  updated_at: number;
  lifecycle_status: string;
  stale_after?: number | null;
  generated_by?: string | null;
  okf_sources: OkfSourceEntry[];
  okf_verified: OkfVerifiedEntry[];
  okf_usage_window?: OkfUsageWindow | null;
  last_verified_at?: number | null;
  last_verified_by?: string | null;
}

export interface OkfSourceEntry {
  id?: string | null;
  resource: string;
  title?: string | null;
  author?: string | null;
  usage_count?: number | null;
  last_modified?: string | null;
}

export interface OkfVerifiedEntry {
  by: string;
  at: number;
}

export interface OkfUsageWindow {
  from: string;
  to: string;
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

export interface EntityEdgeView {
  id: string;
  edge_type: string;
  source_id: string;
  source_label: string;
  target_id: string;
  target_label: string;
}

export interface EntityBacklink {
  entity_id: string;
  name: string;
  entity_type: string;
}

export interface EntityConnections {
  outgoing: EntityEdgeView[];
  backlinks: EntityBacklink[];
}

export const getEntityConnections = (entityId: string): Promise<EntityConnections> =>
  invoke("get_entity_connections_cmd", { entityId });

export const addEntityFact = (entityId: string, body: string): Promise<EntityFact> =>
  invoke("add_entity_fact_cmd", { entityId, body });

export const updateEntityFact = (
  entityId: string,
  factId: string,
  body: string,
): Promise<void> => invoke("update_entity_fact_cmd", { entityId, factId, body });

export const archiveEntityFact = (entityId: string, factId: string): Promise<void> =>
  invoke("archive_entity_fact_cmd", { entityId, factId });

export type TimelineKind =
  | "synthesized" | "approved" | "rejected" | "healed"
  | "imported" | "exported" | "agent_access" | "ingested" | "other";

export interface TimelineEvent {
  id: string;
  kind: TimelineKind;
  summary: string;
  entity_id?: string | null;
  entity_name?: string | null;
  doc_path?: string | null;
  raw_type: string;
  client?: string | null;
  created_at_ms: number;
}

export interface TimelineFilter {
  kinds?: TimelineKind[];
  entity_id?: string;
  since_ms?: number;
  until_ms?: number;
  before_ms?: number;
  /** Composite-cursor tie-breaker: when an event shares `before_ms`'s
   * timestamp, skip those whose id is >= this. */
  before_id?: string;
  limit?: number;
}

export const listEvents = (filter?: TimelineFilter): Promise<TimelineEvent[]> =>
  invoke("list_events_cmd", { filter: filter ?? {} });

export interface TaskRow {
  id: string;
  entity_id: string;
  entity_name: string;
  description: string;
  status: "pending" | "done";
  priority: number;
  created_at: number;
  resolved_at?: number | null;
}

export const listTasks = (
  status?: "pending" | "done",
  includeArchived?: boolean,
): Promise<TaskRow[]> =>
  invoke("list_tasks_cmd", { status, includeArchived });

export const createTask = (entityId: string, description: string): Promise<TaskRow> =>
  invoke("create_task_cmd", { entityId, description });

export const setTaskStatus = (taskId: string, status: "pending" | "done"): Promise<void> =>
  invoke("set_task_status_cmd", { taskId, status });

export const archiveTask = (taskId: string): Promise<void> =>
  invoke("archive_task_cmd", { taskId });

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

export type IngestHealth = 'idle' | 'working' | 'stalled' | 'degraded';

export interface WikiStatusPayload {
  ingest: IngestHealth;
  ingestStage?: string | null;
  ingestSubject?: string | null;
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

export type PrivacyMode = 'strict' | 'ephemeral' | 'connected';

export interface PrivacyState {
  mode: PrivacyMode;
  chosen: boolean;
  needs_migration_disclosure: boolean;
  ephemeral_disclosure_acknowledged: boolean;
}

export interface SetPrivacyModeResult {
  disconnected_bridge: boolean;
  state: PrivacyState;
}

export const getPrivacyMode = (): Promise<PrivacyState> => invoke('get_privacy_mode');

export const setPrivacyMode = (mode: PrivacyMode): Promise<SetPrivacyModeResult> =>
  invoke('set_privacy_mode', { mode });

export const acknowledgeMigrationDisclosure = (): Promise<void> =>
  invoke('acknowledge_migration_disclosure');

export const acknowledgeEphemeralDisclosure = (): Promise<void> =>
  invoke('acknowledge_ephemeral_disclosure');

export type OntologySelection =
  | 'schema-org'
  | 'schema-software-org'
  | 'emergent'
  | 'off';

export const getOntologySelection = (): Promise<OntologySelection> =>
  invoke('get_ontology_selection');

export const setOntologySelection = (selection: OntologySelection): Promise<void> =>
  invoke('set_ontology_selection', { selection });

/** A symlink under documents/ that resolves outside the vault and is not yet
 * approved by the trusted-links ledger. The Desktop review surface renders
 * one card per pending link so the user can approve or revoke in place. */
export interface PendingLink {
  link: string;
  target: string;
}

export const listPendingLinks = (): Promise<PendingLink[]> =>
  invoke('list_pending_links');

export const approveLink = (link: string): Promise<void> =>
  invoke('approve_link', { link });

export const revokeLink = (link: string): Promise<void> =>
  invoke('revoke_link', { link });

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
  profile: "llm-wiki/1" | "llm-wiki/2" | null = null,
): Promise<OkfExportSummary> =>
  invoke("okf_export_bundle_cmd", { destPath, entityIds, profile });

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

export const ingestDocument = (path: string): Promise<void> =>
  invoke("ingest_document_cmd", { path });
