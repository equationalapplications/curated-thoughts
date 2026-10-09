//! Proposal resolution — commits accepted items to `llm_wiki_*` + outbox in one transaction.

use crate::db::outbox_format::{self, OutboxOperation, OutboxPushParams};
use crate::db::proposals::{ItemDecision, ItemDecisionKind, ProposalKind, StoredEvidenceChunk};
use crate::embedder::EmbedProfile;
use anyhow::{bail, Context, Result};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::entity_gate::ImmediateTx;

#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// When true, summary-update conflicts are skipped silently (auto-approve path).
    pub auto_approve: bool,
    /// When `Some`, entry embeddings are computed before the commit transaction
    /// opens and stored with the new facts. `None` disables write-time
    /// embedding — rows land with NULL and the sweep fills them later. Either
    /// way the fact is committed; this only affects how soon it is searchable.
    pub embed_profile: Option<EmbedProfile>,
    /// Pre-computed entry embeddings keyed by `LoadedItem::id`. When `Some`,
    /// the resolver skips its internal `precompute_entry_embeddings` call and
    /// uses this map directly. Used by callers that want to compute the
    /// embeddings OUTSIDE an app-level mutex (the provider round-trip is
    /// blocking and must not run while the lock is held). A missing key means
    /// "no embedding available" — the entry inserts with NULL and the sweep
    /// fills it later, matching the internal-compute contract.
    pub entry_embeddings: Option<EntryEmbeddings>,
    /// Tier stamped on entries whose evidence is certainly deposit-origin
    /// (spec §3.2). `None` falls back to the shipped default rather than
    /// skipping classification: a caller that forgets to plumb config through
    /// still gets spec-correct behaviour, and only an explicit
    /// `wiki.deposit_default_tier` changes the value.
    pub deposit_default_tier: Option<String>,
    /// Human Verification Gate (hvg): reviewer identity recorded on the
    /// proposal row when a human resolves it. `None` on the auto path keeps
    /// the column NULL (characterized by the auto-approve test).
    pub reviewed_by: Option<String>,
    /// Audit row written INSIDE the resolution transaction, so a curated tool's
    /// mutation and its `curated_agent_log` row commit or roll back together
    /// (spec §7 fail-closed audit). The `curated_*` write tools each log inside
    /// their own transaction; a resolve reached from one of them must do the
    /// same, or a crash between the two writes leaves a durable decision with
    /// no record of who made it. `None` on paths that are not curated tool
    /// calls (the desk, the CLI, the librarian) and log elsewhere or not at all.
    pub audit: Option<ResolveAudit>,
}

/// One `curated_agent_log` row, written in the resolution transaction.
///
/// Plain data rather than a callback so [`ResolveOptions`] stays `Clone` +
/// `Debug` and the audit cannot capture connection state.
#[derive(Debug, Clone)]
pub struct ResolveAudit {
    /// Calling client label (`ToolDispatchContext::client`).
    pub client: String,
    /// Tool name recorded in the log (e.g. `curated_proposal_decide`).
    pub tool: String,
    /// `read` or `write` — the audit table CHECKs this.
    pub operation: String,
}

/// Fail-closed curated-agent-log INSERT — the db-layer owner of the
/// `curated_agent_log` write contract (PR #201 review finding 9).
///
/// Lives here rather than in `tool_dispatch` so the writer layer
/// (`resolve_proposal`, via `ResolveOptions::audit`) does not reach UP into
/// the MCP dispatch layer for a schema it does not own; the dispatch-side
/// `log_agent_access_checked` is a thin delegate. Accepts `&Connection` or
/// `&Transaction` (deref coercion) so write tools audit inside the same
/// transaction as the mutation. A failed INSERT propagates: fail-closed,
/// unlike the best-effort legacy logger used by the pre-existing read tools.
pub fn log_agent_access_checked(
    conn: &Connection,
    client: &str,
    tool: &str,
    entity_id: Option<&str>,
    operation: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO curated_agent_log (client, tool, operation, entity_id, summary)
         VALUES (?1, ?2, ?3, ?4, NULL)",
        rusqlite::params![client, tool, operation, entity_id],
    )
    .map_err(|e| anyhow::anyhow!("audit log insert failed for {tool}: {e}"))?;
    Ok(())
}

/// A precomputed entry embedding together with the exact text it was derived
/// from.
///
/// The text is carried so the commit path can verify it before persisting.
/// `resolve_proposal_cmd` loads items, drops the `DbState` mutex to embed, then
/// re-acquires the mutex and re-loads items — so the body the commit is about
/// to write is not guaranteed to be the body that was embedded. Without the
/// check, a payload edited between those phases would persist a vector
/// describing the *old* text, and semantic search would surface the entry for
/// queries matching text it no longer contains. On a mismatch the vector is
/// discarded and the row lands NULL for the sweep to re-embed correctly.
#[derive(Debug, Clone, PartialEq)]
pub struct PrecomputedEmbedding {
    /// Exactly what was fed to the embedder — `embed_text_for_entry(title, body)`.
    pub embed_text: String,
    pub vector: Vec<f32>,
}

/// Precomputed entry embeddings keyed by `LoadedItem::id`.
pub type EntryEmbeddings = std::collections::HashMap<String, PrecomputedEmbedding>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommittedRef {
    pub item_id: String,
    pub table: String,
    pub record_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub committed: Vec<CommittedRef>,
    pub conflicts: Vec<String>,
    pub dropped_edges: Vec<String>,
    pub proposal_status: String,
    /// Phase-2 (spec §2.4): fact_add items skipped at insert time because
    /// their evidence anchored no surviving chunk.
    pub skipped_unanchored: usize,
}

pub(crate) struct LoadedProposal {
    pub(crate) id: String,
    pub(crate) kind: ProposalKind,
    pub(crate) entity_id: Option<String>,
    pub(crate) proposed_name: Option<String>,
    pub(crate) proposed_type: Option<String>,
    pub(crate) created_at: i64,
    pub(crate) status: String,
}

pub(crate) struct LoadedItem {
    pub(crate) id: String,
    pub(crate) item_type: String,
    pub(crate) target_id: Option<String>,
    pub(crate) payload: serde_json::Value,
    pub(crate) evidence: Vec<StoredEvidenceChunk>,
    pub(crate) edited_payload: Option<serde_json::Value>,
}

/// `pub(crate)` so the E2 purge probe ([`edge_write_gate_would_skip`]) can
/// hold ONE context across a whole sweep — the fields stay module-private.
pub(crate) struct CommitContext {
    proposal_id: String,
    proposal_created_at: i64,
    entity_id: String,
    entity_name: String,
    source_type: &'static str,
    now_secs: i64,
    now_ms: i64,
    committed: Vec<CommittedRef>,
    conflicts: Vec<String>,
    dropped_edges: Vec<String>,
    accepted_count: usize,
    rejected_count: usize,
    facts_added: usize,
    facts_updated: usize,
    facts_archived: usize,
    tasks_added: usize,
    facts_duplicated: usize,
    /// Phase-2 (spec §2.4): fact_add items skipped because their evidence
    /// anchored no surviving chunk. Counted so the resolution event can
    /// surface the drop rate even though nothing was written.
    skipped_unanchored: usize,
    /// Entry embeddings computed before the transaction opened, keyed by
    /// `LoadedItem::id`. A missing key means "no embedding available" — the
    /// entry is inserted with NULL and the sweep retries it. A key whose
    /// `embed_text` no longer matches the text being written is treated the
    /// same way; see [`PrecomputedEmbedding`].
    entry_embeddings: EntryEmbeddings,
    /// The tier a deposit-origin entry is stamped with in this commit. Already
    /// resolved against the shipped default, so it is never empty.
    deposit_default_tier: String,
    /// R2.3.0 per-proposal memo (spec: edge gating "resolves the endpoints'
    /// sources once per proposal, not per edge"): endpoint entity id → the
    /// strict edge vocabulary that endpoint's §2.3 ladder resolved to
    /// (`None` = the endpoint resolves not-strict / strict-with-no-vocabulary
    /// and contributes no gate). Populated lazily by the first `edge_add`
    /// item that names the endpoint and reused by every later edge in the
    /// SAME proposal.
    edge_endpoint_strict: std::collections::HashMap<String, Option<EdgeVocabulary>>,
    /// Per-proposal memo of rung-1a opt-out lookups for edge endpoints
    /// (review finding: each edge re-ran the cluster-closed recursive CTE
    /// for both endpoints and their owners inside the write lock). Only
    /// SUCCESSFUL lookups are memoized — a fault is re-tried, never cached.
    edge_endpoint_optout: std::collections::HashMap<String, bool>,
    /// The proposal entity's OWN strict edge vocabulary — the one the read
    /// filter and the off-manifest purge judge an edge row by, since the
    /// row is anchored to the proposal entity. Memoized once per proposal
    /// (`None` = not yet resolved).
    owner_edge_vocabulary: Option<Option<EdgeVocabulary>>,
    /// Human Verification Gate (hvg): reviewer identity to stamp on the
    /// proposal's final guarded UPDATE. Mirrored from `ResolveOptions` so
    /// `finalize_proposal_status_guarded` reads it from the ctx it already
    /// receives; `None` leaves the column NULL (auto path).
    reviewed_by: Option<String>,
}

/// The manifest edge-type vocabulary, keyed for case-insensitive lookup while
/// retaining each type's canonical spelling.
///
/// The lowercase-and-trim rule that decides membership lives here and nowhere
/// else. Before issue #189 this was a bare `HashSet<String>` of lowercased
/// names, and every one of the six call sites repeated
/// `vocab.contains(&edge_type.trim().to_lowercase())` — which is how the
/// writer came to match case-insensitively and then store the candidate's own
/// casing, producing case-variant duplicate rows under
/// `UNIQUE(entity_id, source_id, target_id, edge_type)`.
#[derive(Clone)]
pub(crate) struct EdgeVocabulary {
    /// lowercased+trimmed name → the manifest's canonical spelling
    by_key: std::collections::HashMap<String, String>,
}

impl EdgeVocabulary {
    /// The membership rule, in one place. `wiki_graph::WikiManifest` defers to
    /// it so a manifest never answers "is this declared?" by a different rule
    /// than the writer that gates on the answer.
    pub(crate) fn key(candidate: &str) -> String {
        candidate.trim().to_lowercase()
    }

    /// The single construction point for a vocabulary.
    ///
    /// Manifest order decides the winner when a manifest declares two
    /// case-variants of one type (`Knows` before `knows` keeps `Knows`):
    /// first-declared is the canonical spelling, matching
    /// `WikiManifest::edge_type_names`.
    pub(crate) fn from_manifest(manifest: &crate::wiki_graph::WikiManifest) -> Self {
        let mut by_key: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for edge in &manifest.edge_types {
            by_key
                .entry(Self::key(&edge.type_name))
                .or_insert_with(|| edge.type_name.trim().to_string());
        }
        Self { by_key }
    }

    /// Whether `candidate` names a declared edge type, ignoring case and
    /// surrounding whitespace.
    pub(crate) fn contains(&self, candidate: &str) -> bool {
        self.by_key.contains_key(&Self::key(candidate))
    }

    /// The manifest's spelling of `candidate`, or `None` when it is not
    /// declared. Callers write the returned value rather than the candidate.
    pub(crate) fn canonicalize(&self, candidate: &str) -> Option<&str> {
        self.by_key.get(&Self::key(candidate)).map(String::as_str)
    }

    /// Declared spellings, sorted — for diagnostics only.
    pub(crate) fn declared_sorted(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.by_key.values().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// The manifest edge-type vocabulary to gate writes against, or `None` when
/// writes are not gated for this entity.
///
/// Returns `None` — no gate — in three cases, each deliberate:
///
/// * **Mode is not `strict`.** `emergent` and `off` admit any edge type by
///   definition.
/// * **The ontology could not be read.** A manifest that fails to load is a
///   degraded brain, not a licence to reject the librarian's whole output; PR
///   #78's graceful-degradation contract says the wiki keeps working. Logged.
///
///   Readers degrade the same way, deliberately (#190 review): the
///   cross-partition walker once skipped an unresolvable partition instead,
///   which made the writer fail OPEN while the reader failed CLOSED — a
///   corrupt `tier_fact` would keep accepting edges while hiding the entire
///   graph from traversal. One contract for both sides is what makes that
///   class of divergence unrepresentable rather than merely absent (#158).
/// * **Strict mode declares zero edge types.** A gate needs a vocabulary to be
///   a gate. Mode `strict` with an empty `edge_types` is far more likely a
///   half-finished seed than a deliberate "no edges permitted" policy, and
///   silently dropping every edge of every proposal on such a brain would be a
///   severe and very hard-to-diagnose failure. Logged, and deliberately the
///   most permissive reading — a reviewer who disagrees should flip this one
///   branch, not the whole gate.
///
/// The proposal's `entity_id` is a **curated** id (`ent_<hash>`), not a
/// partition id (`tier_fact`, `tier_wisdom`, `tier_working::*`); manifests
/// are seeded against partitions, so a direct lookup against a curated id
/// almost always misses and would silently disable the gate on every
/// production proposal. The curated entity inherits its manifest from the
/// partition, so `wiki_get_ontology` is called against `tier_fact` — the
/// canonical seeded partition — **as the fallback when the curated-id
/// lookup does NOT find a strict manifest** (the typical production case).
/// Tests that install a manifest for a specific curated id (e.g. `ent-1`)
/// are unaffected: the curated-id lookup wins and the partition lookup is
/// skipped.
pub(crate) fn resolve_strict_edge_vocabulary(
    conn: &Connection,
    entity_id: &str,
) -> Option<EdgeVocabulary> {
    let mut cache = StrictVocabCache::default();
    resolve_strict_edge_vocabulary_with(conn, entity_id, &mut cache)
}

/// One leg of the two-lookup cascade, with the outcome the caller needs to
/// keep the original fall-through semantics exact.
enum OntologyLeg {
    /// `strict` resolved — `Some(vocab)`, or `None` for a strict manifest
    /// that declares zero edge types (already warned: the gate disarms).
    Strict(Option<EdgeVocabulary>),
    /// Present but not `strict` (or strict with no manifest row): fall
    /// through to the next lookup id. See the LATENT-ambiguity note in
    /// [`resolve_strict_edge_vocabulary_with`].
    NotStrict,
    Err(String),
}

fn ontology_leg(conn: &Connection, entity_id: &str, lookup: &str) -> OntologyLeg {
    match crate::wiki_graph::wiki_get_ontology(conn, lookup) {
        Ok(o) if o.mode == "strict" => match o.manifest {
            Some(manifest) => {
                let vocabulary = EdgeVocabulary::from_manifest(&manifest);
                if vocabulary.is_empty() {
                    warn_strict_manifest_declares_no_edge_types(entity_id, lookup);
                    OntologyLeg::Strict(None)
                } else {
                    OntologyLeg::Strict(Some(vocabulary))
                }
            }
            None => OntologyLeg::Strict(None),
        },
        Ok(_) => OntologyLeg::NotStrict,
        Err(e) => OntologyLeg::Err(format!("{lookup}: {e}")),
    }
}

/// Memoized variant of [`resolve_strict_edge_vocabulary`] for read paths that
/// resolve MANY entity ids in one pass (`cross_partition_traverse`, PR #201
/// review finding 7).
///
/// Every production partition id is a curated `ent_<hash>` with no manifest
/// row of its own, so each cascades to the SAME `tier_fact` manifest — an
/// uncached walk of a K-partition hub re-reads and re-parses that manifest
/// up to twice per partition (~2K `wiki_get_ontology` round-trips). The
/// writer already memoizes this per commit (`CommitContext::strict_edge_types`,
/// whose comment calls per-item re-parsing "pure waste"); this gives readers
/// the same treatment.
///
/// Semantics are unchanged: the entity's own manifest row is ALWAYS looked
/// up fresh (a per-entity strict manifest still wins), only the shared
/// `tier_fact` fallback leg is memoized — including its `None` results, so
/// an unresolvable or non-strict partition fallback is computed once per
/// traversal instead of once per partition. An unreadable `tier_fact`
/// manifest consequently warns once per traversal rather than K times.
#[derive(Default)]
pub(crate) struct StrictVocabCache {
    tier_fact: Option<Option<EdgeVocabulary>>,
}

pub(crate) fn resolve_strict_edge_vocabulary_with(
    conn: &Connection,
    entity_id: &str,
    cache: &mut StrictVocabCache,
) -> Option<EdgeVocabulary> {
    if entity_id.is_empty() {
        return None;
    }
    // Leg 1 — the entity's own manifest row, always a fresh lookup.
    match ontology_leg(conn, entity_id, entity_id) {
        OntologyLeg::Strict(v) => return v,
        // An entity leg that is missing or non-strict must NOT short-circuit
        // (e.g. the curated-id row is `mode: "off"` because manifests are
        // seeded against partitions, not curated ids) — fall through to the
        // partition fallback. Only the fallback failing to find a strict
        // manifest anywhere means "no gate".
        OntologyLeg::NotStrict | OntologyLeg::Err(_) => {}
    }
    // Leg 2 — the canonical `tier_fact` fallback, memoized per traversal.
    if cache.tier_fact.is_none() {
        cache.tier_fact = Some(match ontology_leg(conn, entity_id, "tier_fact") {
            OntologyLeg::Strict(v) => v,
            OntologyLeg::NotStrict => None,
            OntologyLeg::Err(e) => {
                warn_ontology_unreadable(entity_id, &[entity_id, "tier_fact"], &e);
                None
            }
        });
    }
    cache.tier_fact.clone().flatten()
}

/// Strict mode with an empty vocabulary disables the gate. Both variants below
/// keep the message text identical; the repo pattern is a `tracing` event under
/// the `mcp-server` feature (where a subscriber exists) and `eprintln!` in the
/// Tauri build, which has none — see `warn_source_ref_parse_error`.
#[cfg(feature = "mcp-server")]
fn warn_strict_manifest_declares_no_edge_types(entity_id: &str, lookup: &str) {
    tracing::warn!(
        target: "ct::commit",
        entity_id = %entity_id,
        via = %lookup,
        "ontology mode is 'strict' but the manifest declares no edge types; \
         edge types are not gated for this commit"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_strict_manifest_declares_no_edge_types(entity_id: &str, lookup: &str) {
    eprintln!(
        "[ct::commit WARN] entity {entity_id} (via {lookup}) is ontology mode 'strict' but \
         its manifest declares no edge types; edge types are not gated for this commit"
    );
}

/// An unreadable ontology also disables the gate (PR #78 graceful degradation).
#[cfg(feature = "mcp-server")]
fn warn_ontology_unreadable(entity_id: &str, lookup_ids: &[&str], last_error: &str) {
    tracing::warn!(
        target: "ct::commit",
        entity_id = %entity_id,
        fallback = ?lookup_ids,
        last_error = %last_error,
        "ontology unreadable; edge types are not gated"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_ontology_unreadable(entity_id: &str, lookup_ids: &[&str], last_error: &str) {
    eprintln!(
        "[ct::commit WARN] ontology unreadable for entity {entity_id} (fallback {lookup_ids:?}, \
         last error: {last_error}); edge types are not gated"
    );
}

/// A fact_add whose evidence anchored no surviving chunk was skipped, not
/// written (Phase-2 default, spec §2.4).
#[cfg(feature = "mcp-server")]
fn warn_unanchored_fact_skipped(proposal_id: &str, fact_title: &str) {
    tracing::warn!(
        target: "ct::commit",
        proposal_id = %proposal_id,
        fact_title = %fact_title,
        "fact_add skipped: evidence anchors no surviving chunk"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_unanchored_fact_skipped(proposal_id: &str, fact_title: &str) {
    eprintln!(
        "[ct::commit WARN] fact_add skipped for proposal {proposal_id}: evidence anchors no \
         surviving chunk (fact: {fact_title})"
    );
}

pub(crate) fn generate_llm_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

/// Normalizer-idempotent `source_ref` for a librarian-inferred entry.
///
/// The JS engine rewrites any `source_ref` its five-predicate selector matches
/// (dist/index.js:1454-1467) through `normalizeSourceRef`, which strips every
/// character outside `[A-Za-z0-9._- ]` and truncates to 255. This token is a
/// fixed point of that function, so the rewrite is a no-op for CT rows.
///
/// Derived by **hashing the entry id**, not by slicing the proposal id: the
/// old JSON refs differed between facts of one proposal (different evidence
/// subsets), so a per-proposal token would silently change dedupe/supersede
/// collision semantics. Spec §2.2.
pub fn librarian_source_ref_token(entry_id: &str) -> String {
    format!(
        "librarian-{}",
        &crate::hasher::hash_bytes(entry_id.as_bytes())[..32]
    )
}

/// Strict token-shape test: `^librarian-[0-9a-f]{32}$` (spec §2.2), the Rust
/// counterpart of `evidence_repair::TOKEN_GLOB`. Every `source_ref` routing
/// decision (`source_ref_is_still_grounded`, `source_docs_from_ref`,
/// `chunk_ids_for_entry`) must use this, not a `starts_with` prefix test — a
/// legacy vault path like `librarian-notes.md` shares the prefix but must keep
/// taking the document branch or it can never be purged.
pub(crate) fn is_librarian_source_ref_token(source_ref: &str) -> bool {
    let Some(hex) = source_ref.strip_prefix("librarian-") else {
        return false;
    };
    hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Insert the CT-owned evidence row for an entry. Callers must run this in the
/// same transaction as the `llm_wiki_entries` INSERT (spec §2.1) so a failure
/// rolls the fact back with it.
pub fn insert_librarian_evidence(
    conn: &Connection,
    entry_id: &str,
    proposal_id: &str,
    evidence_json: &str,
    unanchored: bool,
    now_ms: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO librarian_evidence
             (entry_id, proposal_id, evidence_json, unanchored, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            entry_id,
            proposal_id,
            evidence_json,
            if unanchored { 1i64 } else { 0i64 },
            now_ms
        ],
    )?;
    Ok(())
}

/// Explicit paired delete. FK CASCADE is NOT relied upon — SQLite enforces
/// foreign keys only with `PRAGMA foreign_keys=ON` per connection, and
/// brain.db has connections (Rust `DbState`, engine `wiki_exec`/`wiki_run`)
/// whose pragma state we cannot guarantee. Every path that deletes
/// `llm_wiki_entries` rows calls this alongside. Spec §2.1.
pub fn delete_librarian_evidence(conn: &Connection, entry_ids: &[String]) -> Result<usize> {
    if entry_ids.is_empty() {
        return Ok(0);
    }
    // Bounded batches: one `IN (...)` over every doomed id blows past
    // SQLITE_LIMIT_VARIABLE_NUMBER (32,766 by default) on a large forget and
    // fails the whole transaction. Same chunk shape as
    // `purge_edges_for_hard_deleted`, but additionally clamped to what THIS
    // connection allows — the limit is per-connection, and tests lower it.
    //
    // SAFETY: `sqlite3_limit` with a negative value queries without mutating;
    // `conn.handle()` is valid for the borrow of `conn`.
    let conn_limit = unsafe {
        rusqlite::ffi::sqlite3_limit(
            conn.handle(),
            rusqlite::ffi::SQLITE_LIMIT_VARIABLE_NUMBER,
            -1,
        )
    };
    let chunk_size = crate::db::edge_purge::BATCH_PURGE_CHUNK
        .min(usize::try_from(conn_limit).unwrap_or(1).max(1));

    let mut removed = 0;
    for chunk in entry_ids.chunks(chunk_size) {
        let placeholders: String = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM librarian_evidence WHERE entry_id IN ({placeholders})");
        removed += conn.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
    }
    Ok(removed)
}

/// The ONE hard-delete ceremony for `llm_wiki_entries` rows (PR #201 review
/// finding 8): per doomed id, an `OutboxOperation::Delete` push (keyed on the
/// entity the row itself names), the paired `librarian_evidence` delete and
/// the entry hard-DELETE, then a single batched
/// `purge_edges_for_hard_deleted` sweep — all inside the caller's open
/// transaction, so the arms commit or roll back together.
///
/// This exact four-step shape was hand-rolled in `evidence_repair`,
/// `wiki_forget`, the lib.rs prune and `evidence_regrade` before this fn
/// existed, cross-referenced only by "same shape as" comments — and the one
/// time the ceremony grew an arm (the outbox Delete, issue #132), a missed
/// copy left prisma-outbox replicas serving "forgotten" facts forever. When
/// the ceremony grows another arm (new joined table, read-marker cleanup),
/// it grows HERE once; every hard-delete site gets it by calling this.
///
/// The edge sweep must follow the deletes: edges anchored on a hard-deleted
/// id can never come back, and `purge_edges_for_entries` (the SOFT-delete
/// sweep) deliberately retains a half-live edge, which would strand them
/// forever (#158 contract).
///
/// `doomed` carries `(entry_id, entity_id)` pairs selected by the caller
/// under the same transaction — the outbox is keyed on entity, so pushing
/// the wrong partition would mis-attribute the delete.
pub fn hard_delete_entries(
    conn: &Connection,
    doomed: &[(String, String)],
    now_ms: i64,
) -> Result<()> {
    if doomed.is_empty() {
        return Ok(());
    }
    for (id, entity_id) in doomed {
        push_entries_outbox(
            conn,
            entity_id,
            id,
            crate::db::outbox_format::OutboxOperation::Delete,
            serde_json::json!({ "id": id }),
            now_ms,
        )?;
        conn.execute("DELETE FROM llm_wiki_entries WHERE id = ?1", [id])?;
    }
    let doomed_ids: Vec<String> = doomed.iter().map(|(id, _)| id.clone()).collect();
    delete_librarian_evidence(conn, &doomed_ids)?;
    crate::db::edge_purge::purge_edges_for_hard_deleted(conn, &doomed_ids)?;
    Ok(())
}

/// The evidence blob for an entry, or `None` when no row exists.
///
/// SQLite errors propagate (review round 5): `bundle_io::load_facts` sits on
/// the export path, and swallowing a transient `SQLITE_BUSY` here wrote
/// bundles whose librarian facts carry bare tokens with no paired evidence —
/// importing one elsewhere yields permanently provenance-less rows. Read-only
/// UI callers may degrade defensively, same policy as
/// [`evidence_has_live_chunk`]'s doc contract.
pub fn evidence_json_for_entry(
    conn: &Connection,
    entry_id: &str,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT evidence_json FROM librarian_evidence WHERE entry_id = ?1",
        [entry_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
}

/// Strict `proposal_id` extraction from an evidence blob: `Some` only when the
/// blob parses and carries a non-null, non-empty string `proposal_id`.
///
/// Single source of truth for the salvage-vs-drop routing decision (V18 repair
/// paths 4a/4b and bundle apply's legacy-ref salvage — review round 5, finding
/// 10): a JSON-null or absent id resolves by no path, and the strict posture
/// is export-and-delete — never a `proposal_id = ''` evidence row that no
/// retraction could ever match and no proposal could ever be re-attributed
/// through.
pub fn proposal_id_from_evidence_json(evidence_json: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(evidence_json).ok()?;
    let pid = value.get("proposal_id")?.as_str()?.trim();
    if pid.is_empty() {
        None
    } else {
        Some(pid.to_string())
    }
}

/// True iff the evidence blob anchors at least one chunk that still exists.
///
/// Prefers `content_hash` (stable across re-chunks) and falls back to the
/// legacy `chunk_id` rowid **only when the entry carries no usable hash** —
/// a hash lookup that merely finds no row means the content is gone, and
/// falling through to the rowid there would false-positive on rowid reuse.
/// Empty evidence is **not** anchored — that is the Phase-1 `unanchored`
/// condition, not an error. Spec §2.4.
///
/// SQLite errors propagate: callers on destructive paths (the V18 repair)
/// must halt on a DB fault rather than read it as "no live chunk", and the
/// heal path in `source_ref_is_still_grounded` downgrades the error to its
/// own fail-safe policy.
pub fn evidence_has_live_chunk(conn: &Connection, evidence_json: &str) -> rusqlite::Result<bool> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(evidence_json) else {
        return Ok(false);
    };
    let Some(evidence) = value.get("evidence").and_then(|v| v.as_array()) else {
        return Ok(false);
    };
    for entry in evidence {
        let hash = entry
            .get("content_hash")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if let Some(hash) = hash {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM chunks WHERE content_hash = ?1 LIMIT 1",
                    [hash],
                    |r| r.get(0),
                )
                .optional()?;
            if found.is_some() {
                return Ok(true);
            }
        } else if let Some(cid) = entry.get("chunk_id").and_then(|v| v.as_i64()) {
            let found: Option<i64> = conn
                .query_row("SELECT 1 FROM chunks WHERE id = ?1 LIMIT 1", [cid], |r| {
                    r.get(0)
                })
                .optional()?;
            if found.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn now_timestamps() -> (i64, i64) {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs() as i64, dur.as_millis() as i64)
}

/// Millisecond-precision "now" used by the two `llm_wiki_entries.deleted_at`
/// heal writers (`lib.rs:400` and `lib.rs:1414`) so they match the convention
/// every other writer in the schema uses for this column. Returns `0` if the
/// system clock is somehow before the Unix epoch (would only happen on a
/// pathological test harness).
pub fn ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Returns true iff `source_ref` represents a chunk/fact that still exists in
/// the vault. `source_ref` can be either a vault-relative path (legacy
/// producer contract) or the JSON `{"proposal_id":..., "evidence":[...]}`
/// shape produced by `evidence_json_with_hashes` since commit c30f141.
///
/// Empty / null / parse-error → returns `true` (no-op). The heal policy is
/// "soft-delete if the reference is *demonstrably* stale", and a row that
/// can't be parsed isn't demonstrably stale — it's a legacy path or a future
/// producer we don't know about yet. Logging is the right response, not
/// deletion. Same defensive policy applies to DB lookup errors — a failing
/// lookup is *not* the same thing as a demonstrably-stale reference, and
/// silently soft-deleting on a broken DB would amplify outages. (This is the
/// contract that the six D-tests in commit.rs lock in.)
/// True while the V20 re-grade's destructive phase has not completed.
///
/// `migrate()` deliberately leaves version 20 unstamped when the re-grade
/// skips its destructive phase (blocked export dir, brain-incomplete), so
/// `MAX(version) < 20` doubles as the durable "recovery pending" marker the
/// heal path defers to. A failing lookup is treated as pending — a broken
/// read is not a demonstrably-stale reference (the same defensive policy as
/// every other DB-error branch in `source_ref_is_still_grounded`).
fn v20_regrade_pending(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) < 20 FROM schema_version",
        [],
        |r| r.get(0),
    )
    .unwrap_or(true)
}

pub fn source_ref_is_still_grounded(conn: &Connection, source_ref: &str) -> bool {
    let trimmed = source_ref.trim();
    if trimmed.is_empty() {
        return true;
    }
    // Token rows (librarian_inferred): evidence lives in `librarian_evidence`,
    // not in the ref. Spec §2.3. Strict shape match — see
    // `is_librarian_source_ref_token` for why a prefix test is not enough.
    if is_librarian_source_ref_token(trimmed) {
        let entry_id: Option<String> = conn
            .query_row(
                "SELECT id FROM llm_wiki_entries WHERE source_ref = ?1 LIMIT 1",
                [trimmed],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);
        let Some(entry_id) = entry_id else {
            warn_source_ref_missing_evidence(trimmed, "no entry for token");
            return true;
        };
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT evidence_json, unanchored FROM librarian_evidence WHERE entry_id = ?1",
                [&entry_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap_or(None);
        return match row {
            // Missing evidence row: defensive, same as the parse-error and
            // DB-error branches below. Never auto-purge. Spec §2.3.
            None => {
                warn_source_ref_missing_evidence(trimmed, "no librarian_evidence row");
                true
            }
            // Phase-1 carve-out REMOVED (Phase-2, spec §2.3): flagged rows
            // are grounded strictly by live-chunk evidence again — EXCEPT
            // while the V20 recovery is pending (PR #201 review finding 1):
            // a flagged row whose exporting purge has not completed must not
            // be soft-deleted here, or the doomed stock disappears from
            // regrade's `deleted_at IS NULL` selection before the export
            // that is its only provenance backup can run (the 7-day prune
            // would then hard-delete it un-exported). migrate() deliberately
            // leaves version 20 unstamped on a skipped destructive phase, so
            // `MAX(version) < 20` is the durable pending marker. Once the
            // regrade settles (or on a brain already at 20+), flagged rows
            // ground strictly again, exactly as §2.3 specifies.
            Some((json, unanchored)) => {
                if unanchored == 1 && v20_regrade_pending(conn) {
                    return true;
                }
                match evidence_has_live_chunk(conn, &json) {
                    Ok(live) => live,
                    // Same defensive policy as every other DB-error branch in
                    // this function: a failing lookup is not a demonstrably-stale
                    // reference, so the heal path treats it as still-grounded.
                    Err(err) => {
                        warn_source_ref_db_error(trimmed, "librarian_evidence", &err);
                        true
                    }
                }
            }
        };
    }
    // Legacy contract: a plain vault-relative path. Existence-check against
    // `documents.path = ?1` with `status='indexed'`. The legacy producer never
    // started its value with `{`, so the leading-byte test is sufficient.
    //
    // Defensive error handling: if the lookup itself errors (DB I/O,
    // corrupted schema, lock contention), we treat the reference as
    // *still-grounded* — same policy as the parse-error branch below. A DB
    // failure is not the same thing as a demonstrably-stale reference; the
    // heal policy is "soft-delete iff the reference is *demonstrably*
    // stale", and silently soft-deleting on a broken DB would amplify
    // outages. `unwrap_or(None)` previously collapsed the two cases
    // (NoRows vs Err) and could over-delete during DB faults. The operator
    // sees a warning either way.
    if !trimmed.starts_with('{') {
        match conn.query_row(
            "SELECT 1 FROM documents WHERE path = ?1 AND status = 'indexed' LIMIT 1",
            [trimmed],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(_) => return true,
            Err(rusqlite::Error::QueryReturnedNoRows) => return false,
            Err(err) => {
                warn_source_ref_db_error(trimmed, "documents.path", &err);
                return true;
            }
        }
    }
    // New contract: JSON shape. Defensive parse — see comment above.
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(err) => {
            warn_source_ref_parse_error(trimmed, &err);
            return true;
        }
    };
    let evidence = match value.get("evidence").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return true,
    };
    // Collect chunk_ids; if the entry has no evidence (e.g. an empty-evidence
    // JSON blob — user_stated facts now carry NULL refs instead) it's not
    // librarian-grounded and we leave it alone.
    let chunk_ids: Vec<i64> = evidence
        .iter()
        .filter_map(|entry| entry.get("chunk_id").and_then(|v| v.as_i64()))
        .collect();
    if chunk_ids.is_empty() {
        return true;
    }
    // Any surviving chunk keeps the fact partially grounded; the soft-delete
    // policy only fires when *every* underlying chunk is gone. Same
    // defensive-on-error rule as the legacy branch above.
    for chunk_id in &chunk_ids {
        match conn.query_row(
            "SELECT 1 FROM chunks WHERE id = ?1 LIMIT 1",
            [chunk_id],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(_) => return true,
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(err) => {
                warn_source_ref_db_error(&format!("chunk_id={chunk_id}"), "chunks.id", &err);
                return true;
            }
        }
    }
    false
}

#[cfg(feature = "mcp-server")]
fn warn_source_ref_parse_error(source_ref: &str, err: &serde_json::Error) {
    tracing::warn!(
        target: "ct::heal",
        source_ref = %source_ref,
        error = %err,
        "source_ref is JSON-looking but unparseable; treating as still-grounded (defensive)"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_source_ref_parse_error(source_ref: &str, err: &serde_json::Error) {
    eprintln!(
        "[ct::heal WARN] source_ref is JSON-looking but unparseable; treating as still-grounded: source_ref={source_ref:?} error={err}"
    );
}

#[cfg(feature = "mcp-server")]
fn warn_source_ref_missing_evidence(source_ref: &str, reason: &str) {
    tracing::warn!(
        target: "ct::heal",
        source_ref = %source_ref,
        reason = %reason,
        "librarian token has no evidence row; treating as still-grounded (defensive)"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_source_ref_missing_evidence(source_ref: &str, reason: &str) {
    eprintln!(
        "[ct::heal WARN] librarian token has no evidence row; treating as still-grounded: \
         source_ref={source_ref:?} reason={reason}"
    );
}

/// Logs a heal-path DB lookup failure so operators can see when the
/// grounding check ran into an error (vs. a definitive NoRows). Same
/// defensive "still-grounded" policy as the parse-error branch — we
/// refuse to soft-delete when the lookup itself didn't succeed.
#[cfg(feature = "mcp-server")]
fn warn_source_ref_db_error(source_ref: &str, lookup: &str, err: &rusqlite::Error) {
    tracing::warn!(
        target: "ct::heal",
        source_ref = %source_ref,
        lookup = %lookup,
        error = %err,
        "source_ref DB lookup failed; treating as still-grounded (defensive)"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_source_ref_db_error(source_ref: &str, lookup: &str, err: &rusqlite::Error) {
    eprintln!(
        "[ct::heal WARN] source_ref DB lookup failed; treating as still-grounded: source_ref={source_ref:?} lookup={lookup:?} error={err}"
    );
}

fn effective_payload(item: &LoadedItem, decision: &ItemDecision) -> serde_json::Value {
    decision
        .edited_payload
        .clone()
        .or(item.edited_payload.clone())
        .unwrap_or_else(|| item.payload.clone())
}

pub(crate) fn load_proposal(conn: &Connection, proposal_id: &str) -> Result<LoadedProposal> {
    let row = conn
        .query_row(
            "SELECT kind, entity_id, proposed_name, proposed_type, created_at, status
             FROM curated_proposals WHERE id = ?1",
            [proposal_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .context("proposal not found")?;
    let (kind_str, entity_id, proposed_name, proposed_type, created_at, status) = row;
    Ok(LoadedProposal {
        id: proposal_id.to_string(),
        kind: ProposalKind::from_db(&kind_str)?,
        entity_id,
        proposed_name,
        proposed_type,
        created_at,
        status,
    })
}

pub(crate) fn load_items(conn: &Connection, proposal_id: &str) -> Result<Vec<LoadedItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, item_type, target_id, payload, evidence, edited_payload
         FROM curated_proposal_items
         WHERE proposal_id = ?1
         ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map([proposal_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut items = Vec::new();
    for row in rows {
        let (id, item_type, target_id, payload_raw, evidence_raw, edited_raw) = row?;
        let payload: serde_json::Value = serde_json::from_str(&payload_raw)
            .with_context(|| format!("invalid payload JSON on item {id}"))?;
        let evidence: Vec<StoredEvidenceChunk> = serde_json::from_str(&evidence_raw)
            .with_context(|| format!("invalid evidence JSON on item {id}"))?;
        let edited_payload = edited_raw
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .with_context(|| format!("invalid edited_payload JSON on item {id}"))?;
        items.push(LoadedItem {
            id,
            item_type,
            target_id,
            payload,
            evidence,
            edited_payload,
        });
    }
    Ok(items)
}

fn entity_display_name(conn: &Connection, entity_id: &str) -> Result<String> {
    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM curated_entities WHERE id = ?1",
            [entity_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(name.unwrap_or_else(|| entity_id.to_string()))
}

/// The `curated_entities` names for both endpoints of one edge, `None` for an
/// id that names no curated entity — which includes every `llm_wiki_entries`
/// endpoint, since an edge endpoint may live in either space
/// (`resolve_edge_ref`).
///
/// Tombstones are deliberately INCLUDED, as defence in depth. Since the
/// endpoint-liveness check in `resolve_edge_ref`, a tombstoned endpoint is
/// dropped before it ever reaches this guard, so on the commit path the
/// filter would make no difference today. It stays permissive anyway:
/// filtering tombstones out here would return `(None, None)` for a same-name
/// pair of soft-deleted entities and let the #189 guard fall through on
/// exactly the rows it exists to catch, which is the wrong failure mode to
/// build in if a future caller reaches this helper by another route. A
/// same-name pair is a dedupe artifact whether or not either half is
/// tombstoned.
///
/// Both endpoints are read in one statement: the guard always probes both,
/// and a self-edge (`source == target`) legitimately matches a single row,
/// which both names then take.
fn curated_entity_names(
    conn: &Connection,
    source_id: &str,
    target_id: &str,
) -> Result<(Option<String>, Option<String>)> {
    let mut stmt = conn.prepare("SELECT id, name FROM curated_entities WHERE id IN (?1, ?2)")?;
    let mut by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let rows = stmt.query_map(params![source_id, target_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, name) = row?;
        by_id.insert(id, name);
    }
    Ok((by_id.get(source_id).cloned(), by_id.get(target_id).cloned()))
}

fn trigger_source_label(conn: &Connection, proposal_id: &str) -> Result<String> {
    let basename = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let path: Option<String> = conn
        .query_row(
            "SELECT d.path
             FROM curated_proposal_sources s
             JOIN documents d ON d.id = s.doc_id
             WHERE s.proposal_id = ?1 AND s.role = 'trigger'
             LIMIT 1",
            [proposal_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(p) = path {
        return Ok(basename(&p));
    }
    let fallback: Option<String> = conn
        .query_row(
            "SELECT d.path
             FROM curated_proposal_sources s
             JOIN documents d ON d.id = s.doc_id
             WHERE s.proposal_id = ?1
             LIMIT 1",
            [proposal_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(p) = fallback {
        return Ok(basename(&p));
    }
    // Issue #211 spec D3: a source deleted while the proposal was pending is
    // still named before giving up. The shared helper orders trigger first.
    Ok(
        crate::db::proposals::deleted_source_paths_for_proposal(conn, proposal_id)?
            .first()
            .map(|p| basename(p))
            .unwrap_or_else(|| "unknown source".into()),
    )
}

pub(crate) fn fact_title_from_body(body: &str) -> String {
    let line = body.lines().next().unwrap_or(body).trim();
    if line.is_empty() {
        return "Untitled fact".into();
    }
    if line.chars().count() > 120 {
        let truncated: String = line.chars().take(117).collect();
        format!("{truncated}...")
    } else {
        line.to_string()
    }
}

/// Build a `source_ref` payload where each evidence entry's `content_hash`
/// is resolved from the `chunks` table. The chunk row is the authoritative
/// source of truth — the proposal's in-memory value (which may be empty for
/// legacy fixtures) is preferred only when non-empty, otherwise we look up
/// the chunk row. Returns an empty string when the chunk row is missing so
/// stale proposals don't surface a bogus hash. Real SQLite errors from the
/// lookup propagate as `Err` rather than being silently swallowed — a
/// poisoned connection or schema drift should fail the commit, not write
/// an empty hash into `source_ref`.
/// The tier to stamp on an entry with this evidence, or `None` to leave it
/// unclassified (the working-entry posture).
///
/// Deposit provenance is the only classification a writer can make with
/// certainty, matching the backfill's scope (spec §3.3): an entry is
/// deposit-origin when any evidence chunk resolves to a document under the
/// agent deposit directory. Anything else stays NULL rather than guessing.
///
/// Evidence resolves by `content_hash` when present and by `chunk_id`
/// otherwise, mirroring `evidence_json_with_hashes`'s own precedence — a
/// post-migration write may carry only the hash, and a `chunk_id` surviving
/// a rechunk can be stale while the content_hash is what the source_ref
/// actually persists. Hash-first keeps write-time classification consistent
/// with the persisted provenance.
fn deposit_origin_tier(
    conn: &Connection,
    evidence: &[StoredEvidenceChunk],
    deposit_tier: &str,
) -> Result<Option<String>> {
    for e in evidence {
        let path: Option<String> = if !e.content_hash.is_empty() {
            conn.query_row(
                "SELECT d.path FROM chunks c
                   JOIN documents d ON d.id = c.doc_id
                  WHERE c.content_hash = ?1",
                [&e.content_hash],
                |r| r.get(0),
            )
            .optional()?
        } else if let Some(cid) = e.chunk_id {
            conn.query_row(
                "SELECT d.path FROM chunks c
                   JOIN documents d ON d.id = c.doc_id
                  WHERE c.id = ?1",
                [cid],
                |r| r.get(0),
            )
            .optional()?
        } else {
            None
        };
        if path
            .as_deref()
            .is_some_and(crate::vault::safe_path::is_deposit_path)
        {
            return Ok(Some(deposit_tier.to_string()));
        }
    }
    Ok(None)
}

fn evidence_json_with_hashes(
    conn: &Connection,
    proposal_id: &str,
    evidence: &[StoredEvidenceChunk],
) -> Result<String> {
    let mut entries = Vec::with_capacity(evidence.len());
    for e in evidence {
        // Always prefer the chunk row's content_hash (truth on disk)
        // over the proposal's in-memory value (which may be empty).
        let resolved_hash: String = if !e.content_hash.is_empty() {
            e.content_hash.clone()
        } else if let Some(cid) = e.chunk_id {
            // `.optional()` collapses the "row missing" case to `Ok(None)`
            // (legitimate — stale proposals reference chunks that no
            // longer exist) while letting rusqlite errors propagate so a
            // poisoned connection or schema drift fails the commit.
            let from_row: Option<String> = conn
                .query_row(
                    "SELECT content_hash FROM chunks WHERE id = ?1",
                    [cid],
                    |r| r.get(0),
                )
                .optional()?;
            from_row.unwrap_or_default()
        } else {
            String::new()
        };
        entries.push(serde_json::json!({
            "chunk_id": e.chunk_id,
            "content_hash": resolved_hash,
            "quote": e.quote,
            "start_line": e.start_line,
            "end_line": e.end_line,
            "source_kind": e.source_kind,
        }));
    }
    Ok(serde_json::json!({
        "proposal_id": proposal_id,
        "evidence": entries,
    })
    .to_string())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wiki_fact_outbox_payload(
    id: &str,
    entity_id: &str,
    title: &str,
    body: &str,
    tags: &[String],
    confidence: &str,
    source_type: &str,
    source_hash: Option<&str>,
    source_ref: &str,
    okf_type: Option<&str>,
    okf_sources: Option<&str>,
    okf_verified: Option<&str>,
    okf_usage_window: Option<&str>,
    created_at: i64,
    updated_at: i64,
    deleted_at: Option<i64>,
    lifecycle_status: Option<&str>,
    stale_after: Option<i64>,
    generated_by: Option<&str>,
    last_verified_at: Option<i64>,
    last_verified_by: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "entity_id": entity_id,
        "title": title,
        "body": body,
        "tags": tags,
        "confidence": confidence,
        "source_type": source_type,
        "source_hash": source_hash,
        "source_ref": source_ref,
        "okf_type": okf_type,
        "okf_sources": okf_sources,
        "okf_verified": okf_verified,
        "okf_usage_window": okf_usage_window,
        "lifecycle_status": lifecycle_status,
        "stale_after": stale_after,
        "generated_by": generated_by,
        "last_verified_at": last_verified_at,
        "last_verified_by": last_verified_by,
        "created_at": created_at,
        "updated_at": updated_at,
        "last_accessed_at": null,
        "access_count": 0,
        "deleted_at": deleted_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wiki_task_outbox_payload(
    id: &str,
    entity_id: &str,
    description: &str,
    status: &str,
    priority: i64,
    created_at: i64,
    updated_at: i64,
    resolved_at: Option<i64>,
    deleted_at: Option<i64>,
    okf_type: Option<&str>,
    okf_sources: Option<&str>,
    okf_verified: Option<&str>,
    okf_usage_window: Option<&str>,
    lifecycle_status: Option<&str>,
    stale_after: Option<i64>,
    generated_by: Option<&str>,
    last_verified_at: Option<i64>,
    last_verified_by: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "entity_id": entity_id,
        "description": description,
        "status": status,
        "priority": priority,
        "created_at": created_at,
        "updated_at": updated_at,
        "resolved_at": resolved_at,
        "deleted_at": deleted_at,
        "okf_type": okf_type,
        "okf_sources": okf_sources,
        "okf_verified": okf_verified,
        "okf_usage_window": okf_usage_window,
        "lifecycle_status": lifecycle_status,
        "stale_after": stale_after,
        "generated_by": generated_by,
        "last_verified_at": last_verified_at,
        "last_verified_by": last_verified_by,
    })
}

pub(crate) fn push_entries_outbox(
    conn: &Connection,
    entity_id: &str,
    record_id: &str,
    operation: OutboxOperation,
    payload: serde_json::Value,
    created_at_ms: i64,
) -> Result<()> {
    outbox_format::push_outbox_row(
        conn,
        &OutboxPushParams {
            entity_id: entity_id.into(),
            table_name: "entries".into(),
            record_id: record_id.into(),
            operation,
            payload,
        },
        Some(created_at_ms),
    )?;
    Ok(())
}

pub(crate) fn push_tasks_outbox(
    conn: &Connection,
    entity_id: &str,
    record_id: &str,
    operation: OutboxOperation,
    payload: serde_json::Value,
    created_at_ms: i64,
) -> Result<()> {
    outbox_format::push_outbox_row(
        conn,
        &OutboxPushParams {
            entity_id: entity_id.into(),
            table_name: "tasks".into(),
            record_id: record_id.into(),
            operation,
            payload,
        },
        Some(created_at_ms),
    )?;
    Ok(())
}

fn parse_string_field(payload: &serde_json::Value, field: &str) -> Result<String> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .with_context(|| format!("missing or invalid `{field}` in payload"))
}

fn parse_tags(payload: &serde_json::Value) -> Vec<String> {
    payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn create_entity_if_needed(
    tx: &crate::db::entity_gate::ImmediateTx<'_>,
    policy: &crate::config::IngestPolicy,
    proposal: &LoadedProposal,
    accepted_any: bool,
    now_secs: i64,
) -> Result<(Option<String>, bool)> {
    if !accepted_any || proposal.kind != ProposalKind::NewEntity {
        return Ok((proposal.entity_id.clone(), false));
    }
    if proposal.entity_id.is_some() {
        return Ok((proposal.entity_id.clone(), false));
    }
    let name = proposal
        .proposed_name
        .clone()
        .context("new_entity proposal missing proposed_name")?;
    let proposed_type = proposal.proposed_type.clone();
    let entity_id = generate_llm_id("ent_");

    // Resolve the gate for this NEW-entity mint (LLM synthesis path,
    // spec R2.4.2 / §2.5). Source paths come from the proposal's
    // `curated_proposal_sources` rows; rung 2's `folder_ontology` lookup
    // walks every path and strict-wins across them (R2.3.3).
    let conn: &Connection = tx;
    let source_paths = load_proposal_source_paths(conn, &proposal.id)?;
    let (decision, gate) =
        crate::db::entity_gate::resolve_production_gate(tx, policy, &entity_id, &source_paths);
    let proposed_label = proposed_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // The helper does the gate check + the insert (or refusal). For the
    // LLM synthesis path the SKIP-path behavior is today's: the proposed
    // label lands verbatim (no vocabulary to violate, r2-M2a — the
    // `'concept'` literal only for a label-less proposal) AND a
    // `gate_skipped` ledger row records the origin (R2.4.6 r21). The
    // helper does NOT insert on Skip — we land the row ourselves here so
    // the proposal's `entity_id` is stamped and the existing flow
    // proceeds.
    let outcome = crate::db::entity_gate::shared_insert_entity(
        tx,
        Some(&entity_id),
        &name,
        proposed_label,
        "",
        now_secs,
        decision.clone(),
        false,
    )?;

    match &outcome {
        crate::db::entity_gate::AdmitOutcome::Skipped { .. } => {
            // SKIP path: no gate ran, so there is no vocabulary to
            // violate — today's behavior stands: the proposed label lands
            // verbatim, the `'concept'` literal only when there is none.
            // The helper did NOT insert; land the row so the proposal flow
            // proceeds. The ledger row goes after (the SKIP branch of
            // `write_origin_ledger_for_outcome`).
            crate::db::entity_gate::land_skipped_entity(
                tx,
                &entity_id,
                &name,
                proposed_label,
                "",
                now_secs,
            )?;
        }
        crate::db::entity_gate::AdmitOutcome::Held { reason, .. } => {
            // SG6 (spec §2.4.5): a Held mint FAILS the resolution with a
            // Held-specific error naming the ontology-gate cause. The
            // erroring `?`/return drops the `ImmediateTx`, whose rollback
            // undoes anything this resolution wrote — so the proposal
            // STAYS `pending` and its facts are NOT dropped (they re-enter
            // when the manifest names a fallback / declares node types).
            // Important-3 (final review): the pre-fix code returned
            // `Ok((None, false))` and relied on the generic
            // "proposal has no entity_id" bail further down — safety by
            // accident, with no §2.4.5 diagnostic.
            bail!(
                "ontology gate held (§2.4.5) the new-entity mint for proposal {} ({}): {reason}. \
                 The proposal stays pending and its facts are kept; retry once the \
                 cause is fixed",
                proposal.id,
                proposal.proposed_name.as_deref().unwrap_or("unnamed")
            );
        }
        _ => {
            // Helper inserted (AdmittedDeclared / Aliased / DegradedToFallback).
        }
    }

    // Ledger row — first-origin-wins via INSERT OR IGNORE. The skip row's
    // `source_directory` is the `off` folder when rung 2 caused the SKIP
    // (R2.4.6 r21), carried on the ladder's NodeGateDecision.
    crate::db::entity_gate::write_gate_origin_ledger(
        tx,
        &entity_id,
        &outcome,
        decision,
        gate.source_directory.as_deref(),
    )?;

    tx.execute(
        "UPDATE curated_proposals SET entity_id = ?1 WHERE id = ?2",
        params![entity_id, proposal.id],
    )?;
    Ok((Some(entity_id), true))
}

/// Load the source document paths for an LLM proposal. The gate walks every
/// path and strict-wins across them (R2.3.3 — same asymmetry as edges).
fn load_proposal_source_paths(conn: &Connection, proposal_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT d.path
         FROM curated_proposal_sources s
         JOIN documents d ON d.id = s.doc_id
         WHERE s.proposal_id = ?1",
    )?;
    let rows: Vec<String> = stmt
        .query_map([proposal_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn resolve_edge_ref(
    conn: &Connection,
    value: &serde_json::Value,
    entity_id: &str,
) -> Result<Option<String>> {
    if value == "self" || value.as_str() == Some("self") {
        // `"self"` names the entity the commit is writing to, but that entity
        // is not automatically live. `ctx.entity_id` falls back to
        // `proposal.entity_id` — the id stored on the pending proposal row —
        // and nothing between the proposal being raised and being resolved
        // keeps that entity alive. A proposal that outlives a soft-delete of
        // its own entity therefore reaches here naming a tombstone.
        //
        // Returning it verbatim minted exactly the edge the `existing_id`
        // branch below refuses: paired with a live endpoint it is half-live,
        // and `edge_purge` retains a half-live edge on purpose (module docs),
        // so no later cascade ever collects it.
        //
        // `unwrap_or_default()` at the `CommitContext` construction also makes
        // `entity_id` the empty string when a proposal carries no entity at
        // all; no row has that id, so the same check drops those too.
        if !crate::db::edge_purge::endpoint_is_live(conn, entity_id)? {
            eprintln!(
                "[commit] edge endpoint \"self\" resolves to entity {entity_id:?}, which names \
                 no live row in llm_wiki_entries, curated_entities, or llm_wiki_tasks; dropping \
                 the edge item. A dangling endpoint is never written."
            );
            return Ok(None);
        }
        return Ok(Some(entity_id.to_string()));
    }
    if let Some(id) = value.get("existing_id").and_then(|v| v.as_str()) {
        // An endpoint the purge path would call dead must not be written.
        //
        // This branch used to return the id verbatim — no existence check, no
        // `deleted_at` check — while the `new_name` branch below has always
        // refused a missing-or-tombstoned name. That asymmetry let a proposal
        // naming a hallucinated or soft-deleted endpoint mint an edge, and
        // `edge_purge` retains a **half-live** edge on purpose (module docs),
        // so such an edge dangles for as long as its live partner survives —
        // no later cascade ever collects it. The okf-backend-migration design
        // states the contract this restores: "an unresolved ref auto-rejects
        // that item with a recorded reason — a dangling id is never written."
        //
        // `None` drops the item into `dropped_edges`, matching every other
        // unresolvable-endpoint branch; the log makes the drop attributable,
        // since a dead id is otherwise indistinguishable from a live one in
        // the proposal payload.
        // Task 7 (r15-m4): a candidate echoed back by id may name a
        // merged-away loser — resolve to the survivor first so the edge
        // anchors on the entity recall shows.
        let resolved_id = crate::db::entities::resolve_entity_id(conn, id)?;
        if !crate::db::edge_purge::endpoint_is_live(conn, &resolved_id)? {
            eprintln!(
                "[commit] edge endpoint {id:?} names no live row in llm_wiki_entries, \
                 curated_entities, or llm_wiki_tasks; dropping the edge item for entity \
                 {entity_id}. A dangling endpoint is never written."
            );
            return Ok(None);
        }
        return Ok(Some(resolved_id));
    }
    if let Some(name) = value.get("new_name").and_then(|v| v.as_str()) {
        // Deterministic (review finding: a bare `LIMIT 1` let SQLite pick
        // any of several same-name duplicates). An exact-case match wins;
        // otherwise a case-insensitive one (the merge scan case-folds names,
        // so "Adrian"/"adrian" are the same duplicate group). Ties break on
        // the byte-wise lowest id — the survivor the merge pass itself
        // would pick (R2.7.3), so the edge anchors where the merge lands.
        let resolved: Option<String> = conn
            .query_row(
                "SELECT id FROM live_entities
                 WHERE name = ?1 COLLATE NOCASE AND deleted_at IS NULL
                 ORDER BY (name = ?1) DESC, id
                 LIMIT 1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        return Ok(resolved);
    }
    bail!("unsupported edge endpoint reference: {value}");
}

fn commit_fact_add(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<FactAddOutcome> {
    let body = parse_string_field(payload, "body")?;
    let confidence = payload
        .get("confidence")
        .and_then(|v| v.as_str())
        .unwrap_or("inferred");
    let tags = parse_tags(payload);

    // Phase-1 dedupe: exact match on normalized body, scoped to the target
    // entity's redirect cluster (a merge moves no facts, so a pre-merge fact
    // can still be keyed to a loser — R2.7.5, same closure as fact
    // update/archive). No fuzzy/similarity matching.
    let normalized = normalize_fact_body(&body);
    // TODO(pr-followup): loading every non-deleted body for the entity into
    // memory is O(N) per fact_add and unbounded as the entity grows. Consider
    // a precomputed index or `SELECT body WHERE normalized_body = ?1` if
    // entities routinely accumulate hundreds of facts. Flagged by
    // aws-cloud-agent-pr-review on PR #84 as a theoretical perf concern;
    // not blocking this PR. Filed in procedures/curated-thoughts-improvement-backlog.md.
    let existing_bodies: Vec<String> = {
        let cluster = crate::db::entities::cluster_ids(conn, &ctx.entity_id)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT body FROM llm_wiki_entries
             WHERE entity_id IN ({}) AND deleted_at IS NULL",
            crate::db::entities::in_placeholders(&cluster)
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(cluster.iter()), |r| {
            r.get::<_, String>(0)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if existing_bodies
        .iter()
        .any(|existing| normalize_fact_body(existing) == normalized)
    {
        return Ok(FactAddOutcome::Duplicate);
    }

    let fact_id = generate_llm_id("fact_");
    let title = fact_title_from_body(&body);
    // Evidence is CT-owned and lives in `librarian_evidence`; `source_ref`
    // carries only a normalizer-idempotent token so the engine's setup-time
    // rewrite (dist/index.js:7782-7791) is a no-op for our rows. Spec §2.2.
    let evidence_json = evidence_json_with_hashes(conn, &ctx.proposal_id, &item.evidence)?;
    let source_ref = librarian_source_ref_token(&fact_id);

    // Phase-2 strict gate (spec §2.4): a fact whose evidence anchors no
    // surviving chunk is NOT written. The skip is logged (cfg-gated warn
    // helper) and counted so the resolution summary can surface the drop
    // rate. Fail-closed: an unreadable evidence payload counts as unanchored.
    // Computed once here and reused for the evidence-row flag below (plan
    // Task 1 Step 3: no duplicate evaluation).
    let unanchored = !evidence_has_live_chunk(conn, &evidence_json)?;
    if unanchored {
        warn_unanchored_fact_skipped(&ctx.proposal_id, &title);
        ctx.skipped_unanchored += 1;
        return Ok(FactAddOutcome::SkippedUnanchored);
    }

    // Use the precomputed vector only if it describes the text we are about
    // to write — see `PrecomputedEmbedding`. A stale vector is worse than no
    // vector: NULL is retried by the sweep, a wrong vector is not. The
    // comparison is against the WRITE-scheme text function — the same
    // function the phase-2 pre-embed fed the provider (spec §4).
    let embedding_blob: Option<Vec<u8>> = ctx
        .entry_embeddings
        .get(&item.id)
        .filter(|e| e.embed_text == crate::embed_sweep::embed_text_for_entry(&title, &body))
        .map(|e| crate::wiki_graph::f32_vec_to_blob(&e.vector));

    // Stored tier (spec §3.2). Deposit-origin evidence takes the configured
    // deposit default; everything else stays NULL, which is the
    // working/unclassified posture the librarian falls back to chunk
    // heuristics for.
    let tier = deposit_origin_tier(conn, &item.evidence, &ctx.deposit_default_tier)?;

    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embed_scheme, embedding, tier
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?9, NULL, 0, NULL, ?10, ?11, NULL, ?12)",
        params![
            fact_id,
            ctx.entity_id,
            title,
            body,
            serde_json::to_string(&tags)?,
            confidence,
            ctx.source_type,
            source_ref,
            ctx.now_ms,
            embedding_blob,
            crate::embed_scheme::WRITE_SCHEME,
            tier,
        ],
    )?;

    // Same transaction as the entry INSERT: if this fails the fact is not
    // written. Fail-closed, consistent with the json_valid CHECK. Spec §5.
    //
    // Phase 2 policy (spec §2.4): unanchored facts are skipped above, so a
    // written fact always has a live anchor and this flag is 0 for new rows.
    // The column stays populated (0) to keep the schema and the historic
    // Phase-1 rows meaningful. The binding is the gate's computation above.
    insert_librarian_evidence(
        conn,
        &fact_id,
        &ctx.proposal_id,
        &evidence_json,
        unanchored,
        ctx.now_ms,
    )?;

    push_entries_outbox(
        conn,
        &ctx.entity_id,
        &fact_id,
        OutboxOperation::Insert,
        wiki_fact_outbox_payload(
            &fact_id,
            &ctx.entity_id,
            &title,
            &body,
            &tags,
            confidence,
            ctx.source_type,
            None,
            &source_ref,
            None,
            None,
            None,
            None,
            ctx.now_ms,
            ctx.now_ms,
            None,
            // Proposal-created facts default to the stable lifecycle; the
            // OKF v0.2 fields populate on import / verified annotation.
            Some("stable"),
            None,
            None,
            None,
            None,
        ),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entries".into(),
        record_id: fact_id,
    });
    ctx.facts_added += 1;
    Ok(FactAddOutcome::Applied)
}

fn commit_fact_update(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<()> {
    let fact_id = item
        .target_id
        .as_deref()
        .context("fact_update requires target_id")?;
    let body = parse_string_field(payload, "body")?;
    let confidence = payload
        .get("confidence")
        .and_then(|v| v.as_str())
        .unwrap_or("inferred");
    let tags = parse_tags(payload);

    // (source_ref, created_at, body, source_hash, okf_type, okf_sources,
    //  okf_verified, okf_usage_window, lifecycle_status, stale_after,
    //  generated_by, last_verified_at, last_verified_by)
    type WikiFactRow = Option<(
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<String>,
    )>;

    // Transitive fact closure (r13-m3, review finding): the target row may
    // still be keyed to a redirected loser from before a merge — candidate
    // facts include loser-keyed ids, so match the CLUSTER (same rule as
    // `update_wisdom_in_tx`) and rekey the row to the survivor in the same
    // UPDATE. A bare `entity_id = ?` bail here would roll back the whole
    // resolution and wedge the auto-approve retry loop on every later run.
    let cluster = crate::db::entities::cluster_ids(conn, &ctx.entity_id)?;
    let cluster_placeholders = crate::db::entities::in_placeholders(&cluster);
    let survivor = ctx.entity_id.clone();

    let existing: WikiFactRow = conn
        .query_row(
            &format!(
                // COALESCE handles imported facts with a NULL source_ref so
                // the r.get::<_, String>(0) deserializer doesn't bail before
                // the update and outbox write can proceed.
                "SELECT COALESCE(source_ref, ''), created_at,
                        body,
                        source_hash, okf_type, okf_sources, okf_verified, okf_usage_window,
                        lifecycle_status, stale_after, generated_by,
                        last_verified_at, last_verified_by
                 FROM llm_wiki_entries
                 WHERE id = ? AND entity_id IN ({cluster_placeholders}) AND deleted_at IS NULL"
            ),
            rusqlite::params_from_iter(
                std::iter::once(fact_id.to_string()).chain(cluster.iter().cloned()),
            ),
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                ))
            },
        )
        .optional()?;
    let Some((
        existing_source_ref,
        created_at,
        previous_body,
        existing_source_hash,
        existing_okf_type,
        existing_okf_sources,
        existing_okf_verified,
        existing_okf_usage_window,
        existing_lifecycle_status,
        existing_stale_after,
        existing_generated_by,
        existing_last_verified_at,
        existing_last_verified_by,
    )) = existing
    else {
        bail!("fact_update target not found: {fact_id}");
    };
    let body_changed = previous_body != body;

    let title = fact_title_from_body(&body);

    // Write the freshly computed embedding if the pre-pass produced one for
    // exactly this text; otherwise NULL so the sweep re-embeds — never leave a
    // vector describing text the entry no longer contains. The text check
    // matters here specifically: the caller embeds a body loaded before the
    // `DbState` mutex was dropped, and `resolve_proposal` re-loads items after
    // re-acquiring it (see `PrecomputedEmbedding`). The comparison is against
    // the WRITE-scheme text function — the same function the pre-embed fed
    // the provider (spec §4).
    let embedding_blob: Option<Vec<u8>> = ctx
        .entry_embeddings
        .get(&item.id)
        .filter(|e| e.embed_text == crate::embed_sweep::embed_text_for_entry(&title, &body))
        .map(|e| crate::wiki_graph::f32_vec_to_blob(&e.vector));

    // Issue #265: the scheme stamp rides in the same statement as the blob —
    // a vector must never exist under a stale or unknown scheme. It binds as
    // ?9 below; the body-unchanged arm stamps via CASE, only when a fresh
    // blob actually lands (never blind-write the stamp over a blob we did
    // not write).
    //
    // Shared shape for both UPDATE arms below: ?1..?5 common SET columns,
    // ?6 the (possibly COALESCEd) embedding, ?7 the fact id, ?8 the
    // survivor rekey, ?9 the embed-scheme stamp, ?10.. the cluster `IN`
    // list. Positional binding keeps the numbered and anonymous placeholders
    // aligned (the anonymous tail starts past the highest explicit number).
    let mut update_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(title.clone()),
        Box::new(body.clone()),
        Box::new(serde_json::to_string(&tags)?),
        Box::new(confidence.to_string()),
        Box::new(ctx.now_ms),
        Box::new(embedding_blob),
        Box::new(fact_id.to_string()),
        Box::new(survivor.clone()),
        Box::new(crate::embed_scheme::WRITE_SCHEME),
    ];
    for id in &cluster {
        update_params.push(Box::new(id.clone()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        update_params.iter().map(|p| p.as_ref()).collect();

    if body_changed {
        conn.execute(
            &format!(
                "UPDATE llm_wiki_entries
                 SET title = ?1, body = ?2, tags = ?3, confidence = ?4, updated_at = ?5,
                     embedding_blob = ?6, embed_scheme = ?9, entity_id = ?8
                 WHERE id = ?7 AND entity_id IN ({cluster_placeholders})"
            ),
            param_refs.as_slice(),
        )?;
    } else {
        // Body unchanged, so any stored vector still describes this text and
        // must be preserved — but if the row is NULL (an earlier write-time
        // embed failed and the sweep has not run yet) this is the moment to
        // fill it: we already paid for the vector and it matches the body.
        // COALESCE does both: overwrite when we have one, keep otherwise. The
        // scheme stamp follows the blob: a fresh vector re-stamps, an
        // untouched row keeps whatever scheme its surviving blob was written
        // under (never blind-write the stamp over a blob we did not write).
        conn.execute(
            &format!(
                "UPDATE llm_wiki_entries
                 SET title = ?1, body = ?2, tags = ?3, confidence = ?4, updated_at = ?5,
                     embedding_blob = COALESCE(?6, embedding_blob),
                     embed_scheme = CASE WHEN ?6 IS NOT NULL THEN ?9 ELSE embed_scheme END,
                     entity_id = ?8
                 WHERE id = ?7 AND entity_id IN ({cluster_placeholders})"
            ),
            param_refs.as_slice(),
        )?;
    }

    push_entries_outbox(
        conn,
        &ctx.entity_id,
        fact_id,
        OutboxOperation::Update,
        wiki_fact_outbox_payload(
            fact_id,
            &ctx.entity_id,
            &title,
            &body,
            &tags,
            confidence,
            ctx.source_type,
            existing_source_hash.as_deref(),
            &existing_source_ref,
            existing_okf_type.as_deref(),
            existing_okf_sources.as_deref(),
            existing_okf_verified.as_deref(),
            existing_okf_usage_window.as_deref(),
            created_at,
            ctx.now_ms,
            None,
            Some(existing_lifecycle_status.as_str()),
            existing_stale_after,
            existing_generated_by.as_deref(),
            existing_last_verified_at,
            existing_last_verified_by.as_deref(),
        ),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entries".into(),
        record_id: fact_id.to_string(),
    });
    ctx.facts_updated += 1;
    Ok(())
}

fn commit_fact_archive(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
) -> Result<()> {
    let fact_id = item
        .target_id
        .as_deref()
        .context("fact_archive requires target_id")?;

    // Transitive fact closure (r13-m3, review finding): the target row may
    // still be keyed to a redirected loser from before a merge — match the
    // cluster (same rule as `archive_wisdom_in_tx`) instead of bailing,
    // which would roll back the whole resolution — and rekey the row to the
    // survivor in the same UPDATE, so the archived row and the Delete
    // outbox payload (`entity_id` = survivor) agree on the owner (#132).
    let cluster = crate::db::entities::cluster_ids(conn, &ctx.entity_id)?;
    let cluster_placeholders = crate::db::entities::in_placeholders(&cluster);
    // ?1 timestamp, ?2 fact id, ?3 survivor rekey, ?4.. the cluster list.
    let mut archive_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(ctx.now_ms),
        Box::new(fact_id.to_string()),
        Box::new(ctx.entity_id.clone()),
    ];
    for id in &cluster {
        archive_params.push(Box::new(id.clone()));
    }
    let archive_param_refs: Vec<&dyn rusqlite::types::ToSql> =
        archive_params.iter().map(|p| p.as_ref()).collect();
    let changes = conn.execute(
        &format!(
            "UPDATE llm_wiki_entries
             SET deleted_at = ?1, updated_at = ?1, entity_id = ?3
             WHERE id = ?2 AND entity_id IN ({cluster_placeholders}) AND deleted_at IS NULL"
        ),
        archive_param_refs.as_slice(),
    )?;
    if changes == 0 {
        bail!("fact_archive target not found: {fact_id}");
    }

    // Edges die with their endpoints, in the same transaction (design spec §2).
    // No outbox rows: edges are not replicated.
    crate::db::edge_purge::purge_edges_for_entry(conn, fact_id)?;

    push_entries_outbox(
        conn,
        &ctx.entity_id,
        fact_id,
        OutboxOperation::Delete,
        serde_json::json!({
            "id": fact_id,
            "entity_id": ctx.entity_id,
            "deleted_at": ctx.now_ms,
        }),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entries".into(),
        record_id: fact_id.to_string(),
    });
    ctx.facts_archived += 1;
    Ok(())
}

fn commit_summary_update(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
    auto_approve: bool,
) -> Result<SummaryUpdateOutcome> {
    let summary = parse_string_field(payload, "summary")?;
    let entity_updated_at: i64 = conn.query_row(
        "SELECT updated_at FROM curated_entities WHERE id = ?1 AND deleted_at IS NULL",
        [&ctx.entity_id],
        |r| r.get(0),
    )?;

    if entity_updated_at > ctx.proposal_created_at {
        if auto_approve {
            return Ok(SummaryUpdateOutcome::SkippedSilent);
        }
        ctx.conflicts.push(item.id.clone());
        return Ok(SummaryUpdateOutcome::Conflict);
    }

    conn.execute(
        "UPDATE curated_entities SET summary = ?1, updated_at = ?2 WHERE id = ?3",
        params![summary, ctx.now_secs, ctx.entity_id],
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entities".into(),
        record_id: ctx.entity_id.clone(),
    });
    Ok(SummaryUpdateOutcome::Applied)
}

enum SummaryUpdateOutcome {
    Applied,
    Conflict,
    SkippedSilent,
}

enum ItemCommitOutcome {
    Applied,
    Rejected,
}

enum FactAddOutcome {
    Applied,
    /// Normalized body exactly matches an existing fact on the same entity.
    Duplicate,
    /// Phase-2 (spec §2.4): the evidence anchored no surviving chunk, so the
    /// fact was not written. Logged via the cfg-gated warn helper and counted
    /// in `CommitContext::skipped_unanchored`.
    SkippedUnanchored,
}

/// Normalize a fact body for exact-match dedupe: trim edges and collapse
/// internal whitespace runs to single spaces.
fn normalize_fact_body(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn commit_task_add(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<()> {
    let description = parse_string_field(payload, "description")?;
    let priority = payload
        .get("priority")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let task_id = generate_llm_id("task_");

    conn.execute(
        "INSERT INTO llm_wiki_tasks (
            id, entity_id, description, status, priority,
            created_at, updated_at, resolved_at, deleted_at
         ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5, NULL, NULL)",
        params![task_id, ctx.entity_id, description, priority, ctx.now_ms],
    )?;

    push_tasks_outbox(
        conn,
        &ctx.entity_id,
        &task_id,
        OutboxOperation::Insert,
        wiki_task_outbox_payload(
            &task_id,
            &ctx.entity_id,
            &description,
            "pending",
            priority,
            ctx.now_ms,
            ctx.now_ms,
            None,
            None,
            None,
            None,
            None,
            None,
            // Proposal-created tasks default to the stable lifecycle; the
            // OKF v0.2 fields populate on import / verified annotation.
            Some("stable"),
            None,
            None,
            None,
            None,
        ),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "tasks".into(),
        record_id: task_id,
    });
    ctx.tasks_added += 1;
    Ok(())
}

fn commit_edge_add(
    conn: &Connection,
    gate: &crate::db::entity_gate::GateResolutionContext<'_>,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<()> {
    let edge_type = parse_string_field(payload, "edge_type")?;
    let source_ref = payload
        .get("source")
        .context("edge_add payload missing source")?;
    let target_ref = payload
        .get("target")
        .context("edge_add payload missing target")?;

    let source_id = match resolve_edge_ref(conn, source_ref, &ctx.entity_id)? {
        Some(id) => id,
        None => {
            ctx.dropped_edges.push(item.id.clone());
            return Ok(());
        }
    };
    let target_id = match resolve_edge_ref(conn, target_ref, &ctx.entity_id)? {
        Some(id) => id,
        None => {
            ctx.dropped_edges.push(item.id.clone());
            return Ok(());
        }
    };

    // Strict-mode write boundary (spec §2.3). `llm_wiki_edges` is the semantic
    // knowledge graph, so in strict mode an `edge_type` absent from the
    // manifest is not written.
    //
    // Dropped rather than raised, matching the unresolvable-endpoint branches
    // above: an off-manifest edge type is one bad item in an otherwise good
    // proposal, and failing the commit would make a single hallucinated type
    // discard a batch of good facts. The item is marked rejected and reported
    // in `dropped_edges`, so the drop is visible rather than silent.
    //
    // Reads stay untyped-tolerant: this is a write-time gate only, and rows
    // written before the manifest existed remain readable and traversable.
    //
    // R2.3.0 (Task 3, final wave): strict-wins across ENDPOINTS. If either
    // endpoint resolves strict (its own strict manifest row, or its source
    // path rung-2/3 strict verdict), the EDGE is gated under the strict
    // vocabulary that fires FIRST. The §2.3 rung-1a's entity-level opt-out
    // (a deliberate `ct_entity_optouts` row) short-circuits the edge gate
    // on that endpoint (§2.1, r12-M1 restated).
    let proposal_entity_id = ctx.entity_id.clone();
    let resolved_vocabulary = resolve_edge_endpoint_vocabulary(
        conn,
        &proposal_entity_id,
        &source_id,
        &target_id,
        ctx,
        gate,
    )?;
    let edge_type = match resolved_vocabulary {
        Some(vocabulary) => match vocabulary.canonicalize(&edge_type) {
            // Issue #189: write the manifest's spelling, not the candidate's.
            Some(canonical) => {
                // The row is anchored to the proposal entity, and the read
                // filter and the off-manifest purge judge it by THAT
                // entity's strict vocabulary. When the endpoint gate fired
                // under a different (endpoint-owner) vocabulary, the type
                // must also be admissible under the anchor's, or the write
                // gate admits an edge that is hidden on read and destroyed
                // by the next sweep. Conjunctive: never loosens the gate.
                let owner = ctx.owner_edge_vocabulary.get_or_insert_with(|| {
                    resolve_strict_edge_vocabulary(conn, &proposal_entity_id)
                });
                if let Some(owner) = owner {
                    if owner.canonicalize(canonical).is_none() {
                        eprintln!(
                            "[commit] edge_type {canonical:?} is declared by an endpoint's \
                             ontology manifest but not by the anchoring entity {entity}'s \
                             (declared: {declared:?}); dropping edge item {item}",
                            entity = ctx.entity_id,
                            declared = owner.declared_sorted(),
                            item = item.id,
                        );
                        ctx.dropped_edges.push(item.id.clone());
                        return Ok(());
                    }
                }
                canonical.to_string()
            }
            None => {
                let declared = vocabulary.declared_sorted();
                eprintln!(
                    "[commit] edge_type {edge_type:?} is not declared by the ontology manifest \
                     for entity {entity}; dropping edge item {item}. Declared types: {declared:?}",
                    entity = ctx.entity_id,
                    item = item.id,
                );
                ctx.dropped_edges.push(item.id.clone());
                return Ok(());
            }
        },
        // No strict vocabulary: nothing to canonicalize against, write
        // verbatim. R2.3.0: both endpoints off / no-manifest (or a rung-1a
        // opt-out) → SKIP. The r22 anchor-vocabulary conjunction applies
        // only when the endpoint gate FIRES (the Some arm above); applying
        // it here would re-gate opted-out and all-off edges under
        // `tier_fact`, undoing D8. That such SKIP rows are still judged by
        // the anchor's vocabulary on read and purge is the open E2 design
        // call, not a write-gate bug.
        None => edge_type,
    };

    // Issue #189: the librarian's dedupe artifacts arrive as edges between
    // two curated entities that render as the same name — three `supersedes`
    // self-edges on "Curated Thoughts" in the Sep 6 run. They carry no
    // semantic value, and the duplication they encode is the entity-merge
    // pass's problem, not the graph's.
    //
    // Ordered AFTER the strict gate deliberately: an off-manifest edge_type
    // between two same-name endpoints is BOTH a dedupe artifact and a
    // manifest violation, and the manifest violation is the one an operator
    // must see — it is the signal that the librarian is inventing types.
    // Running the cheap in-memory gate first also spares the two endpoint
    // lookups below for every edge the gate already rejects.
    //
    // Both endpoints must be curated entities for the comparison to mean
    // anything: an `llm_wiki_entries` endpoint has no `curated_entities` row,
    // and two facts sharing a title is ordinary. Dropped and reported,
    // matching the unresolvable-endpoint branches above.
    let (source_name, target_name) = curated_entity_names(conn, &source_id, &target_id)?;
    if let (Some(s), Some(t)) = (&source_name, &target_name) {
        if s.trim() == t.trim() {
            eprintln!(
                "[commit] same-name curated endpoints {source_id:?} and {target_id:?} \
                 (both named {s:?}); dropping {edge_type:?} edge item {item}. \
                 Duplicate entities are resolved by the entity-merge pass, not by edges.",
                item = item.id,
            );
            ctx.dropped_edges.push(item.id.clone());
            return Ok(());
        }
    }

    let edge_id = generate_llm_id("edge_");
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            edge_id,
            ctx.entity_id,
            source_id,
            target_id,
            edge_type,
            ctx.now_ms,
        ],
    )?;

    if inserted > 0 {
        ctx.committed.push(CommittedRef {
            item_id: item.id.clone(),
            table: "edges".into(),
            record_id: edge_id,
        });
    }
    Ok(())
}

/// R2.3.0 (Task 3): strict-wins across EDGE endpoints. If EITHER
/// `source_id` or `target_id` has a deliberate `ct_entity_optouts` row,
/// the edge cascade short-circuits (§2.1, r12-m1 restated) and the gate
/// disarms (return `None` → no vocabulary → write verbatim). Otherwise
/// EACH endpoint's §2.3 mode is resolved — rung 1b (its own manifest row),
/// rungs 2–3 (its source directories via `folder_ontology` /
/// `ontology_default`), rung 4 (`tier_fact`) — and the edge is GATED if
/// EITHER endpoint resolves strict, under the strict side's vocabulary:
/// the endpoint's own manifest row when that row is the strict rung, else
/// the `tier_fact` vocabulary (the mode-vs-vocabulary rule, r9-M1 — rungs
/// 2–3 supply a mode, never a vocabulary). The SOURCE endpoint's strict
/// vocabulary fires first when both are strict.
///
/// Per-proposal memoization (spec R2.3.0: "resolves the endpoints' sources
/// once per proposal, not per edge"): each endpoint's ladder — including
/// the source-document resolution rung 2 walks — runs at most once per
/// proposal, memoized in [`CommitContext::edge_endpoint_strict`].
/// E2 resolution (controller ruling 2026-10-08, spec r24): would the
/// WRITE gate write an edge between these endpoints verbatim today — i.e.
/// does [`resolve_edge_endpoint_vocabulary`] resolve NO strict vocabulary
/// (both endpoints off / no-manifest, a rung-1a opt-out, or a strict
/// manifest that declares no edge types)? The retroactive off-manifest
/// purge must not destroy rows that are the write gate's deliberate
/// output ("off means off", D8): such rows stay hidden by the
/// anchor-vocabulary read filter but are RECOVERABLE (declare the type /
/// change the mode), which deletion is not.
///
/// Purely a read probe: the caller supplies the (shareable) probe context
/// ([`CommitContext::purge_probe`]), `entity_id` is `""` (never a real
/// ladder id, so the mid-commit proposal-source fallback cannot fire) and
/// `proposal_id` matches no proposal row. One context may serve MANY
/// probes: the memo maps key on ladder ids and nothing a purge mutates
/// (`llm_wiki_edges` rows only) feeds the ladders, so the per-endpoint
/// walks memoize across a whole sweep instead of re-running per doomed
/// edge (review finding: the write gate memoizes per proposal for exactly
/// this reason).
pub(crate) fn edge_write_gate_would_skip(
    conn: &Connection,
    source_id: &str,
    target_id: &str,
    gate: &crate::db::entity_gate::GateResolutionContext<'_>,
    probe: &mut CommitContext,
) -> Result<bool> {
    Ok(resolve_edge_endpoint_vocabulary(conn, "", source_id, target_id, probe, gate)?.is_none())
}

impl CommitContext {
    /// Fresh read-probe context for the E2 purge spare check (see
    /// [`edge_write_gate_would_skip`]): no proposal, no entity, empty memo
    /// maps. Callers probing many edges share ONE context so each
    /// endpoint's ladder — manifest reads, cluster expansion, per-fact
    /// source resolution — runs once per distinct endpoint, not once per
    /// edge.
    pub(crate) fn purge_probe() -> Self {
        CommitContext {
            proposal_id: String::new(),
            proposal_created_at: 0,
            entity_id: String::new(),
            entity_name: String::new(),
            source_type: "purge_probe",
            now_secs: 0,
            now_ms: 0,
            committed: Vec::new(),
            conflicts: Vec::new(),
            dropped_edges: Vec::new(),
            accepted_count: 0,
            rejected_count: 0,
            facts_added: 0,
            facts_updated: 0,
            facts_archived: 0,
            tasks_added: 0,
            facts_duplicated: 0,
            skipped_unanchored: 0,
            entry_embeddings: std::collections::HashMap::new(),
            deposit_default_tier: crate::config::DEFAULT_DEPOSIT_TIER.to_string(),
            edge_endpoint_strict: std::collections::HashMap::new(),
            edge_endpoint_optout: std::collections::HashMap::new(),
            owner_edge_vocabulary: None,
            reviewed_by: None,
        }
    }
}

fn resolve_edge_endpoint_vocabulary(
    conn: &Connection,
    entity_id: &str,
    source_id: &str,
    target_id: &str,
    ctx: &mut CommitContext,
    gate: &crate::db::entity_gate::GateResolutionContext<'_>,
) -> Result<Option<EdgeVocabulary>> {
    // Rung 1a — explicit opt-out on either endpoint → skip the gate. A read
    // FAULT never reads as "no row" in the disarming direction (D8): it
    // warns and falls through to the ladder — the fail-safe direction, more
    // gating, never less.
    //
    // A fact/task endpoint's opt-out lives on its OWNING entity (the same
    // mapping the ladder below walks), so check the owner as well as the
    // raw endpoint id. Each endpoint's ladder id is computed ONCE here and
    // reused by the vocabulary walk below (review finding: the loop and
    // `endpoint_edge_vocabulary` each ran `endpoint_ladder_id` — one to
    // three queries — per endpoint, per edge, inside the write lock).
    let source_ladder = endpoint_ladder_id(conn, source_id)?;
    let target_ladder = endpoint_ladder_id(conn, target_id)?;
    for (eid, ladder_id) in [
        (source_id, source_ladder.as_str()),
        (target_id, target_ladder.as_str()),
    ] {
        let mut ids = vec![eid];
        if ladder_id != eid {
            ids.push(ladder_id);
        }
        for id in ids {
            let lookup = match ctx.edge_endpoint_optout.get(id) {
                Some(&cached) => Ok(cached),
                None => crate::db::entity_gate::entity_has_optout(conn, id).inspect(|&v| {
                    ctx.edge_endpoint_optout.insert(id.to_string(), v);
                }),
            };
            match lookup {
                Ok(true) => return Ok(None),
                Ok(false) => {}
                Err(e) => {
                    eprintln!(
                        "[commit] opt-out lookup failed for endpoint {id}: {e}; \
                         continuing with the edge ladder (fail-safe)"
                    );
                }
            }
        }
    }
    let source = endpoint_edge_vocabulary_for(conn, &source_ladder, entity_id, gate, ctx)?;
    if source.is_some() {
        // Strict-wins across endpoints: the strict source side gates, under
        // its own vocabulary.
        return Ok(source);
    }
    // The source endpoint resolves not-strict; a STRICT target still gates
    // (an off directory shields its own entities from CONTRIBUTING
    // obligations but never downgrades an edge the strict side makes
    // checkable — same asymmetry as §2.3.3).
    endpoint_edge_vocabulary_for(conn, &target_ladder, entity_id, gate, ctx)
}

/// Resolve ONE endpoint's §2.3 ladder to the edge vocabulary it contributes
/// (`None` = not-strict / no usable vocabulary → contributes no gate), with
/// the per-proposal memo. `ladder_id` is the endpoint's precomputed
/// [`endpoint_ladder_id`] (the owning entity for a fact/task endpoint).
fn endpoint_edge_vocabulary_for(
    conn: &Connection,
    ladder_id: &str,
    proposal_entity_id: &str,
    gate: &crate::db::entity_gate::GateResolutionContext<'_>,
    ctx: &mut CommitContext,
) -> Result<Option<EdgeVocabulary>> {
    // Memoized on the LADDER id (the owning entity for a fact/task
    // endpoint): N fact endpoints of one hub entity resolve its ladder —
    // and walk its facts' sources — once, not N times inside the write lock.
    if let Some(cached) = ctx.edge_endpoint_strict.get(ladder_id) {
        return Ok(cached.clone());
    }
    let resolved =
        resolve_endpoint_ladder(conn, ladder_id, proposal_entity_id, gate, &ctx.proposal_id)?;
    ctx.edge_endpoint_strict
        .insert(ladder_id.to_string(), resolved.clone());
    Ok(resolved)
}

/// The per-endpoint §2.3 ladder, edge flavor:
///
///   * rung 1b — the endpoint's own manifest row. A STRICT row gates under
///     ITS edge vocabulary (a strict row declaring zero edge types disarms,
///     per §2.1's warn-and-disarm; `ontology_leg` owns the warning). An
///     unmarked row climbs; an UNREADABLE row falls through WITHOUT a
///     warning — §2.3 rung 1(c) keeps today's edge behavior (only nodes
///     and heal get report-or-hold).
///   * rungs 2–3 — the endpoint's source directories. The vocabulary comes
///     from `tier_fact` (the mode-vs-vocabulary rule). Strict-wins across
///     paths; an `off` path loses to any strict path, but when EVERY
///     path resolves off the endpoint is off (`None`) — a strict rung-4
///     tier_fact never overrides it (R2.3.0 / §2.3). A rung-2/3 strict
///     verdict with no strict `tier_fact` vocabulary disarms (mode = GATE +
///     no vocabulary row → SKIP + census warning, mirroring §2.1).
///   * rung 4 — `tier_fact` itself (unmarked/off row → `None`, §2.3.1).
///
/// `ladder_id` is the endpoint's [`endpoint_ladder_id`] — review finding
/// (fact/task endpoints): edge endpoints are frequently
/// `llm_wiki_entries`/`llm_wiki_tasks` ids, not entity ids, and the
/// ladder's rungs are ENTITY-keyed, so running them against a fact id finds
/// no manifest row and no source paths and falls to rung 4, ungating edges
/// the old proposal-entity gate refused. The caller maps a fact/task
/// endpoint to its OWNING entity (resolved to the merge survivor); the
/// owner's fact source paths subsume the endpoint fact's.
fn resolve_endpoint_ladder(
    conn: &Connection,
    ladder_id: &str,
    proposal_entity_id: &str,
    gate: &crate::db::entity_gate::GateResolutionContext<'_>,
    proposal_id: &str,
) -> Result<Option<EdgeVocabulary>> {
    let ladder_id = ladder_id.to_string();

    // Rung 1b — the endpoint's own manifest row.
    match ontology_leg(conn, &ladder_id, &ladder_id) {
        OntologyLeg::Strict(v) => return Ok(v),
        // Unmarked (rung 1d) climbs; Err keeps today's edge fall-through.
        OntologyLeg::NotStrict | OntologyLeg::Err(_) => {}
    }

    // Rungs 2–3 — the endpoint's source directories. For an EXISTING
    // entity these are its live facts' resolved document paths (the same
    // shared core, `resolve_source_core`, the heal pass walks). The
    // proposal's OWN entity is mid-commit — its facts may not have landed
    // yet — so when it has no resolvable fact sources its rung-2 inputs
    // are the proposal's TRIGGER document paths, the same paths the mint
    // gate walked for it (R2.3.4).
    let mut paths = endpoint_fact_source_paths(conn, &ladder_id)?;
    if paths.is_empty() && ladder_id == proposal_entity_id {
        // DB faults PROPAGATE (review finding / R2.3.2a, same rule as
        // `endpoint_fact_source_paths`): swallowing one here would leave
        // the proposal entity's rung-2 inputs empty and silently disarm the
        // strict edge gate.
        paths = load_proposal_source_paths(conn, proposal_id)?;
    }
    // §2.3 "first hit decides" / R2.3.0: an endpoint whose EVERY source
    // resolves `off` is off — a strict rung-4 tier_fact never overrides it.
    // Any climbing (or held) source reaches rung 4, so strict-wins there.
    //
    // R2.3.4: an endpoint with NO resolvable sources climbs from rung 3
    // (the host default) — the same pathless resolution the node gate's
    // GUI/bundle mint arm uses — so a host-wide `ontology_default: "off"`
    // shields a fact-less endpoint exactly as it shields the mint.
    let lookups = gate.source_lookups(&paths);
    let mut off_found = false;
    let mut reaches_rung_4 = false;
    for (_, lookup) in lookups {
        match lookup {
            crate::config::OntologyLookup::Mode(crate::config::OntologyMode::Strict) => {
                // Vocabulary from `tier_fact` (mode-vs-vocabulary rule).
                return Ok(match ontology_leg(conn, &ladder_id, "tier_fact") {
                    OntologyLeg::Strict(v) => v,
                    OntologyLeg::NotStrict => None,
                    OntologyLeg::Err(e) => {
                        warn_ontology_unreadable(&ladder_id, &[&ladder_id, "tier_fact"], &e);
                        None
                    }
                });
            }
            crate::config::OntologyLookup::Mode(crate::config::OntologyMode::Off) => {
                // Off loses to a strict rung (R2.3.3) — keep looking.
                off_found = true;
                continue;
            }
            // Degraded-config Hold: the edge gate keeps today's posture
            // (edges never consulted the degraded state pre-R2.3.0); the
            // §2.2.4 scoped hold is the node gate's. Falls through to
            // rung 4, which still gates under `tier_fact` when strict.
            crate::config::OntologyLookup::Hold | crate::config::OntologyLookup::Climb => {
                reaches_rung_4 = true;
                continue;
            }
        }
    }
    if off_found && !reaches_rung_4 {
        return Ok(None);
    }

    // Rung 4 — tier_fact itself.
    Ok(match ontology_leg(conn, &ladder_id, "tier_fact") {
        OntologyLeg::Strict(v) => v,
        OntologyLeg::NotStrict => None,
        OntologyLeg::Err(e) => {
            warn_ontology_unreadable(&ladder_id, &[&ladder_id, "tier_fact"], &e);
            None
        }
    })
}

/// The entity id an edge endpoint's §2.3 ladder (and rung-1a opt-out) is
/// keyed on: a fact/task endpoint maps to its owning entity, resolved to the
/// merge survivor; any other id is used as-is.
fn endpoint_ladder_id(conn: &Connection, endpoint_id: &str) -> Result<String> {
    Ok(match endpoint_owner_entity(conn, endpoint_id)? {
        Some(owner) => crate::db::entities::resolve_entity_id(conn, &owner)?,
        None => endpoint_id.to_string(),
    })
}

/// A fact/task endpoint id → its owning entity id, or `Ok(None)` when the id
/// is not a fact/task row (an entity id — the normal case). DB faults
/// PROPAGATE (R2.3.2a): swallowing one here would silently disarm the strict
/// edge gate, the exact silent-mutation direction the spec forbids.
fn endpoint_owner_entity(conn: &Connection, endpoint_id: &str) -> Result<Option<String>> {
    let owner: Option<String> = conn
        .query_row(
            "SELECT entity_id FROM llm_wiki_entries WHERE id = ?1
             UNION ALL
             SELECT entity_id FROM llm_wiki_tasks WHERE id = ?1
             LIMIT 1",
            [endpoint_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(owner)
}

/// An EXISTING endpoint entity's rung-2 source paths: every live fact's
/// `source_ref` resolved through the shared core, deduplicated by path.
/// DB faults PROPAGATE (review finding / R2.3.2a): `.unwrap_or_default()`
/// here turned a transient fault (e.g. SQLITE_BUSY) into empty rung-2
/// source paths and silently disarmed the strict edge gate. An unresolvable
/// SOURCE still contributes no path — that is a data state, not a fault (the
/// R2.3.2 report-only rule scopes to heal; for a write-time edge gate an
/// unresolvable source can only ever REMOVE a checkable strict folder — the
/// direction that drops a visible `dropped_edges` item, never a silent
/// mutation).
fn endpoint_fact_source_paths(conn: &Connection, endpoint_id: &str) -> Result<Vec<String>> {
    // After a merge, facts can stay keyed to the LOSER id (the survivor's
    // ladder must see them) — expand to the whole redirect cluster, the same
    // coverage `commit_fact_update`/`commit_fact_archive`/`get_entity` use.
    let cluster = crate::db::entities::cluster_ids(conn, endpoint_id)?;
    let cluster_placeholders = crate::db::entities::in_placeholders(&cluster);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, source_ref FROM llm_wiki_entries
         WHERE entity_id IN ({cluster_placeholders}) AND deleted_at IS NULL"
    ))?;
    let facts: Vec<(String, Option<String>)> = stmt
        .query_map(rusqlite::params_from_iter(cluster.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    // Ordered output + a HashSet shadow for O(1) dedup (review finding: a
    // `Vec::contains` scan made this quadratic inside the IMMEDIATE tx).
    let mut paths: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (entry_id, source_ref) in &facts {
        if let crate::db::entities::SourceResolution::Resolved(resolved) =
            crate::db::entities::resolve_source_core(conn, entry_id, source_ref.as_deref())?
        {
            for (path, _) in resolved {
                if seen.insert(path.clone()) {
                    paths.push(path);
                }
            }
        }
    }
    Ok(paths)
}

/// The single place the final proposal status write lives (hvg Task 2).
///
/// The `WHERE ... AND status = 'pending'` clause is the in-transaction half of
/// the double-resolve defence: `resolve_proposal`'s :2023 pre-check runs
/// before `BEGIN IMMEDIATE`, so a second resolver that passed the pre-check
/// while another commit held the write lock would otherwise silently overwrite
/// the winner's resolution. Racing on the pending row itself makes exactly one
/// resolution land; any loser sees `rows_affected == 0` and bails before
/// `tx.commit()`, so its resolution event, item updates and edges roll back.
///
/// `ctx` is `&mut` only so the loser branch can be extended with counters
/// without touching every call site.
fn finalize_proposal_status_guarded(
    tx: &rusqlite::Transaction,
    ctx: &mut CommitContext,
    proposal_status: &str,
    now_secs: i64,
    reject_reason: Option<&str>,
    proposal_id: &str,
) -> Result<()> {
    let changed = tx.execute(
        "UPDATE curated_proposals
         SET status = ?1, resolved_at = ?2, reject_reason = ?3, reviewed_by = ?4
         WHERE id = ?5 AND status = 'pending'",
        params![
            proposal_status,
            now_secs,
            reject_reason,
            ctx.reviewed_by,
            proposal_id
        ],
    )?;
    if changed != 1 {
        bail!("proposal {proposal_id} already resolved (concurrent or repeated decision)");
    }
    Ok(())
}

fn write_resolution_event(
    conn: &Connection,
    ctx: &CommitContext,
    proposal_status: &str,
    source_label: &str,
) -> Result<()> {
    let event_type = match proposal_status {
        "rejected" => "rejected",
        _ => "approved",
    };
    let mut parts = Vec::new();
    if ctx.facts_added > 0 {
        parts.push(format!("{} fact(s) added", ctx.facts_added));
    }
    if ctx.facts_updated > 0 {
        parts.push(format!("{} fact(s) updated", ctx.facts_updated));
    }
    if ctx.facts_archived > 0 {
        parts.push(format!("{} fact(s) archived", ctx.facts_archived));
    }
    if ctx.tasks_added > 0 {
        parts.push(format!("{} task(s) added", ctx.tasks_added));
    }
    if ctx.facts_duplicated > 0 {
        parts.push(format!(
            "{} duplicate fact(s) skipped",
            ctx.facts_duplicated
        ));
    }
    if ctx.skipped_unanchored > 0 {
        parts.push(format!(
            "{} unanchored fact(s) skipped",
            ctx.skipped_unanchored
        ));
    }

    let summary = if proposal_status == "rejected" {
        if parts.is_empty() {
            format!(
                "Rejected proposal for *{}* from *{}*",
                ctx.entity_name, source_label
            )
        } else {
            format!(
                "Rejected proposal for *{}* from *{}*: {}",
                ctx.entity_name,
                source_label,
                parts.join(", ")
            )
        }
    } else if parts.is_empty() {
        format!(
            "Approved proposal for *{}* from *{}*",
            ctx.entity_name, source_label
        )
    } else {
        format!(
            "Approved: {} to *{}* from *{}*",
            parts.join(", "),
            ctx.entity_name,
            source_label
        )
    };

    let event_id = generate_llm_id("evt_");
    conn.execute(
        "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![event_id, ctx.entity_id, event_type, summary, ctx.now_ms],
    )?;
    Ok(())
}

fn finalize_proposal_status(accepted: usize, rejected: usize) -> &'static str {
    if accepted == 0 {
        "rejected"
    } else if rejected == 0 {
        "approved"
    } else {
        "partial"
    }
}

/// Embed the bodies of every accepted `fact_add` in one batch.
///
/// Returns an empty map when no profile is configured or the provider fails —
/// an embedding is a derived artifact and must never fail a commit. The
/// null-embedding sweep picks up whatever is missing.
///
/// `pub(crate)` so the Tauri commands in `proposals_api.rs` can hoist this
/// call outside the app-level `DbState` mutex. Inside `resolve_proposal`
/// itself, the precompute runs while holding the SQLite IMMEDIATE
/// transaction (acquired a few lines later), but the OUTER app-level mutex
/// is what blocks other Tauri commands — and that is what this helper
/// exists to escape.
pub(crate) fn precompute_entry_embeddings(
    items: &[LoadedItem],
    decisions: &[ItemDecision],
    profile: Option<&EmbedProfile>,
) -> EntryEmbeddings {
    use std::collections::HashMap;

    let Some(profile) = profile else {
        return HashMap::new();
    };

    let accepted: std::collections::HashSet<&str> = decisions
        .iter()
        .filter(|d| d.decision == ItemDecisionKind::Accept)
        .map(|d| d.item_id.as_str())
        .collect();

    let mut ids: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for item in items {
        if !matches!(item.item_type.as_str(), "fact_add" | "fact_update")
            || !accepted.contains(item.id.as_str())
        {
            continue;
        }
        let decision = decisions.iter().find(|d| d.item_id == item.id);
        let payload = match decision {
            Some(d) => effective_payload(item, d),
            None => continue,
        };
        let Some(body) = payload.get("body").and_then(|v| v.as_str()) else {
            continue;
        };
        let title = fact_title_from_body(body);
        ids.push(item.id.clone());
        texts.push(crate::embed_sweep::embed_text_for_entry(&title, body));
    }

    if ids.is_empty() {
        return HashMap::new();
    }

    // Cap the batch the same way the sweep does.
    let mut out = HashMap::new();
    for (id_chunk, text_chunk) in ids
        .chunks(crate::embed_sweep::SWEEP_BATCH_SIZE)
        .zip(texts.chunks(crate::embed_sweep::SWEEP_BATCH_SIZE))
    {
        match crate::embedder::embed_batch(profile, text_chunk.to_vec()) {
            Ok(vectors) => {
                // R6 zip-truncation guard: a provider that drops a row would
                // silently mis-pair the surviving vectors with the wrong ids
                // via the zip below. Skip this chunk — those rows land with
                // NULL embeddings and the sweep fills them later.
                if vectors.len() != id_chunk.len() {
                    // codeql[rust/cleartext-logging]: vectors.len() and id_chunk.len()
                    // are local usize counts; the API key resolved inside
                    // embed_batch never reaches this string. Collapsed to a single
                    // line so CodeQL doesn't flag a per-argument sub-sink.
                    eprintln!(
                        "precompute_entry_embeddings: provider returned a mismatched \
                         number of vectors; skipping chunk to avoid mis-pairing"
                    );
                    continue;
                }
                for ((id, embed_text), vector) in
                    id_chunk.iter().zip(text_chunk.iter()).zip(vectors)
                {
                    out.insert(
                        id.clone(),
                        PrecomputedEmbedding {
                            embed_text: embed_text.clone(),
                            vector,
                        },
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "commit: entry embedding failed for {} items, leaving NULL for the sweep: {e}",
                    id_chunk.len()
                );
            }
        }
    }
    out
}

/// Resolve a pending proposal inside `BEGIN IMMEDIATE` — all mutations and outbox rows roll back together on failure.
pub fn resolve_proposal(
    conn: &mut Connection,
    proposal_id: &str,
    decisions: &[ItemDecision],
    reject_reason: Option<&str>,
    options: ResolveOptions,
) -> Result<CommitResult> {
    let proposal = load_proposal(conn, proposal_id)?;
    if proposal.status != "pending" {
        bail!("proposal is not pending: {}", proposal.status);
    }

    let items = load_items(conn, proposal_id)?;
    if items.is_empty() {
        bail!("proposal has no items");
    }

    let decisions_by_id: std::collections::HashMap<&str, &ItemDecision> =
        decisions.iter().map(|d| (d.item_id.as_str(), d)).collect();

    let accepted_any = items.iter().any(|item| {
        decisions_by_id
            .get(item.id.as_str())
            .is_some_and(|d| d.decision == ItemDecisionKind::Accept)
    });

    let (now_secs, now_ms) = now_timestamps();
    let source_type = if options.auto_approve {
        "librarian_inferred"
    } else {
        "user_confirmed"
    };

    // Compute entry embeddings BEFORE opening the transaction: embed_batch is a
    // blocking network call and must never run while a write lock is held.
    //
    // This necessarily happens before the dedupe check (which needs the
    // transaction), so a duplicate fact_add burns a provider call. Accepted:
    // batches are small, duplicates are rare, and hoisting dedupe out of the
    // transaction would trade that for a TOCTOU race.
    //
    // When the caller supplies `entry_embeddings`, the work was already done
    // outside the app-level `DbState` mutex — used by `resolve_proposal_cmd`
    // so the blocking round-trip does not gate every other Tauri command.
    let entry_embeddings = options.entry_embeddings.clone().unwrap_or_else(|| {
        precompute_entry_embeddings(&items, decisions, options.embed_profile.as_ref())
    });

    // r21 hold-time rule (R15/R14): the ingest policy is FILESYSTEM I/O —
    // load it BEFORE the IMMEDIATE transaction opens. One load serves both
    // the new-entity mint gate (`create_entity_if_needed`) and the R2.3.0
    // per-endpoint edge gate (`commit_edge_add`).
    let gate_policy = crate::config::ingest_policy_for_db(conn.path());
    let gate_degraded = gate_policy.ontology_degraded_state();
    let gate_ctx = crate::db::entity_gate::GateResolutionContext {
        ingest: &gate_policy.tiers,
        degraded: &gate_degraded,
        schema: gate_policy.ontology_selection,
        schema_unparseable: gate_policy.ontology_unparseable,
        vault_root: gate_policy.vault_root.as_deref(),
    };

    let tx = ImmediateTx::begin(conn)?;

    let (minted_entity, entity_was_created_here) =
        create_entity_if_needed(&tx, &gate_policy, &proposal, accepted_any, now_secs)?;
    // Task 7 (r13-MAJOR-1 / r15-m4): a proposal naming a merged-away loser
    // (or a model echoing back a stale candidate id) resolves to the
    // survivor BEFORE any item commits — every fact/task/edge/summary
    // mutation below keys on ctx.entity_id, so this one resolution covers
    // the whole commit. A redirect cycle errors the commit loudly.
    let mut entity_id = match minted_entity.or(proposal.entity_id.clone()) {
        Some(eid) => Some(crate::db::entities::resolve_entity_id(&tx, &eid)?),
        None => None,
    };

    let mut ctx = CommitContext {
        proposal_id: proposal_id.to_string(),
        proposal_created_at: proposal.created_at,
        entity_id: entity_id.clone().unwrap_or_default(),
        entity_name: proposal
            .proposed_name
            .clone()
            .unwrap_or_else(|| entity_id.clone().unwrap_or_else(|| "Unknown".into())),
        source_type,
        now_secs,
        now_ms,
        committed: Vec::new(),
        conflicts: Vec::new(),
        dropped_edges: Vec::new(),
        accepted_count: 0,
        rejected_count: 0,
        facts_added: 0,
        facts_updated: 0,
        facts_archived: 0,
        tasks_added: 0,
        facts_duplicated: 0,
        skipped_unanchored: 0,
        entry_embeddings,
        deposit_default_tier: options
            .deposit_default_tier
            .clone()
            .unwrap_or_else(|| crate::config::DEFAULT_DEPOSIT_TIER.to_string()),
        edge_endpoint_strict: std::collections::HashMap::new(),
        edge_endpoint_optout: std::collections::HashMap::new(),
        owner_edge_vocabulary: None,
        reviewed_by: options.reviewed_by.clone(),
    };

    if let Some(eid) = entity_id.as_deref() {
        ctx.entity_name = if proposal.kind == ProposalKind::NewEntity {
            proposal
                .proposed_name
                .clone()
                .unwrap_or_else(|| eid.to_string())
        } else {
            entity_display_name(&tx, eid)?
        };
        ctx.entity_id = eid.to_string();
    }

    for item in &items {
        let Some(decision) = decisions_by_id.get(item.id.as_str()) else {
            ctx.rejected_count += 1;
            tx.execute(
                "UPDATE curated_proposal_items SET status = 'rejected' WHERE id = ?1",
                [&item.id],
            )?;
            continue;
        };

        if decision.decision == ItemDecisionKind::Reject {
            ctx.rejected_count += 1;
            tx.execute(
                "UPDATE curated_proposal_items SET status = 'rejected' WHERE id = ?1",
                [&item.id],
            )?;
            continue;
        }

        if entity_id.is_none() {
            let _ = tx.rollback();
            bail!("proposal has no entity_id");
        }

        let payload = effective_payload(item, decision);
        let item_outcome: Result<ItemCommitOutcome> =
            match item.item_type.as_str() {
                "fact_add" => {
                    commit_fact_add(&tx, &mut ctx, item, &payload).map(|outcome| match outcome {
                        FactAddOutcome::Applied => ItemCommitOutcome::Applied,
                        FactAddOutcome::Duplicate => {
                            ctx.facts_duplicated += 1;
                            ItemCommitOutcome::Rejected
                        }
                        FactAddOutcome::SkippedUnanchored => {
                            // Counted in `commit_fact_add` itself so direct
                            // callers see the skip too.
                            ItemCommitOutcome::Rejected
                        }
                    })
                }
                "fact_update" => commit_fact_update(&tx, &mut ctx, item, &payload)
                    .map(|_| ItemCommitOutcome::Applied),
                "fact_archive" => {
                    commit_fact_archive(&tx, &mut ctx, item).map(|_| ItemCommitOutcome::Applied)
                }
                "summary_update" => {
                    commit_summary_update(&tx, &mut ctx, item, &payload, options.auto_approve).map(
                        |outcome| match outcome {
                            SummaryUpdateOutcome::Applied => ItemCommitOutcome::Applied,
                            SummaryUpdateOutcome::Conflict
                            | SummaryUpdateOutcome::SkippedSilent => ItemCommitOutcome::Rejected,
                        },
                    )
                }
                "task_add" => commit_task_add(&tx, &mut ctx, item, &payload)
                    .map(|_| ItemCommitOutcome::Applied),
                "edge_add" => commit_edge_add(&tx, &gate_ctx, &mut ctx, item, &payload).map(|_| {
                    if ctx.dropped_edges.iter().any(|id| id == &item.id) {
                        ItemCommitOutcome::Rejected
                    } else {
                        ItemCommitOutcome::Applied
                    }
                }),
                other => bail!("unsupported item_type: {other}"),
            };

        match item_outcome {
            Ok(ItemCommitOutcome::Applied) => {
                ctx.accepted_count += 1;
                let edited_json = decision
                    .edited_payload
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                tx.execute(
                    "UPDATE curated_proposal_items
                     SET status = 'accepted', edited_payload = COALESCE(?2, edited_payload)
                     WHERE id = ?1",
                    params![item.id, edited_json],
                )?;
            }
            Ok(ItemCommitOutcome::Rejected) => {
                ctx.rejected_count += 1;
                tx.execute(
                    "UPDATE curated_proposal_items SET status = 'rejected' WHERE id = ?1",
                    [&item.id],
                )?;
            }
            Err(e) => {
                let _ = tx.rollback();
                return Err(e);
            }
        }
    }

    // Empty-shell rollback (PR #201 review finding 2): `accepted_any` is
    // DECISION-based (it gates entity creation before any item commits), so
    // an approved new_entity whose every fact_add is skipped by the Phase-2
    // unanchored gate — or lands as a duplicate — resolves to 'rejected'
    // while the entity INSERT and the proposals.entity_id stamp already
    // happened. Left in place, the reviewer's approval permanently mints a
    // zero-entry curated_entities shell (visible in the entity list/graph,
    // never pruned) AND writes a resolution event for an entity whose only
    // content was refused. Undo both, in this transaction, when nothing
    // landed for an entity this resolution created: the proposal reads as a
    // plain rejection, exactly like the all-reject path that never created
    // one (locked by `review_reject_new_entity_writes_no_event_but_columns_persist`).
    if entity_was_created_here && ctx.accepted_count == 0 {
        if let Some(eid) = entity_id.as_deref() {
            tx.execute("DELETE FROM curated_entities WHERE id = ?1", [eid])?;
            // Review finding: the gate ledger row written inside this
            // resolution (`gate_skipped` / `degraded` / `unlabeled_landing`)
            // must die with the entity — otherwise `entity_type_origin`
            // keeps a row for an entity that no longer exists and heal /
            // census count ghosts.
            tx.execute("DELETE FROM entity_type_origin WHERE entity_id = ?1", [eid])?;
            tx.execute(
                "UPDATE curated_proposals SET entity_id = NULL WHERE id = ?1",
                [proposal_id],
            )?;
        }
        entity_id = None;
    }

    let proposal_status = finalize_proposal_status(ctx.accepted_count, ctx.rejected_count);
    let source_label = trigger_source_label(&tx, proposal_id)?;
    if entity_id.is_some() {
        write_resolution_event(&tx, &ctx, proposal_status, &source_label)?;
    }

    finalize_proposal_status_guarded(
        &tx,
        &mut ctx,
        proposal_status,
        now_secs,
        reject_reason,
        proposal_id,
    )?;

    // Fail-closed audit (spec §7): the log row shares this transaction, so it
    // is impossible to observe a resolved proposal with no record of who
    // resolved it — and a failed audit insert aborts the resolution instead of
    // committing it unlogged.
    if let Some(audit) = options.audit.as_ref() {
        log_agent_access_checked(
            &tx,
            &audit.client,
            &audit.tool,
            Some(proposal_id),
            &audit.operation,
        )?;
    }

    tx.commit()?;

    Ok(CommitResult {
        committed: ctx.committed,
        conflicts: ctx.conflicts,
        dropped_edges: ctx.dropped_edges,
        proposal_status: proposal_status.to_string(),
        skipped_unanchored: ctx.skipped_unanchored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkStrategyTag};
    use crate::db::connection::open_in_memory;
    use crate::db::proposals::{
        insert_proposal, NewProposal, NewProposalItem, NewProposalSource, ProposalKind,
        ProposalSourceRole, StoredEvidenceChunk,
    };
    use crate::db::queries::{insert_chunk, upsert_document};

    /// Re-implementation of the engine's `normalizeSourceRef`
    /// (dist/index.js:4082) plus its five-predicate selector
    /// (dist/index.js:1454-1467). Used to prove the token is a fixed point.
    fn engine_would_rewrite(source_ref: &str) -> bool {
        let selected = source_ref.trim() != source_ref
            || source_ref.contains('/')
            || source_ref.contains('\\')
            || source_ref.contains('\0')
            || source_ref
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')));
        if !selected {
            return false;
        }
        let normalized: String = source_ref
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
            .collect::<String>()
            .trim()
            .chars()
            .take(255)
            .collect();
        normalized != source_ref
    }

    #[test]
    fn token_is_a_fixed_point_of_the_engine_normalizer() {
        for entry_id in ["fact_abc123", "fact_0000", "fact_zz~!@#"] {
            let token = librarian_source_ref_token(entry_id);
            assert!(
                token.starts_with("librarian-"),
                "token must carry the librarian- prefix: {token}"
            );
            assert_eq!(
                token.len(),
                "librarian-".len() + 32,
                "token must be 32 hex chars"
            );
            assert!(
                token[10..]
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "digest must be lowercase hex: {token}"
            );
            assert!(token.len() <= 255);
            assert!(
                !engine_would_rewrite(&token),
                "engine selector must not touch the token: {token}"
            );
        }
    }

    #[test]
    fn token_is_deterministic_and_per_entry_unique() {
        assert_eq!(
            librarian_source_ref_token("fact_a"),
            librarian_source_ref_token("fact_a")
        );
        assert_ne!(
            librarian_source_ref_token("fact_a"),
            librarian_source_ref_token("fact_b")
        );
    }

    #[test]
    fn evidence_roundtrips_and_deletes() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, access_count)
             VALUES ('fact_r','ent','t','b','[]','inferred','librarian_inferred','librarian-x',1,1,0)",
            [],
        )
        .unwrap();

        insert_librarian_evidence(
            &conn,
            "fact_r",
            "prop_r",
            r#"{"proposal_id":"prop_r","evidence":[]}"#,
            false,
            123,
        )
        .unwrap();

        assert_eq!(
            evidence_json_for_entry(&conn, "fact_r").unwrap().as_deref(),
            Some(r#"{"proposal_id":"prop_r","evidence":[]}"#)
        );
        assert_eq!(
            evidence_json_for_entry(&conn, "fact_missing").unwrap(),
            None
        );

        let removed = delete_librarian_evidence(&conn, &["fact_r".to_string()]).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(evidence_json_for_entry(&conn, "fact_r").unwrap(), None);
    }

    #[test]
    fn delete_librarian_evidence_chunks_past_the_variable_limit() {
        let conn = crate::db::connection::open_in_memory().unwrap();
        // Lower SQLite's variable limit so the test does not need 32k rows.
        // 999 is SQLITE_LIMIT_VARIABLE_NUMBER's historical default.
        unsafe {
            rusqlite::ffi::sqlite3_limit(
                conn.handle(),
                rusqlite::ffi::SQLITE_LIMIT_VARIABLE_NUMBER,
                50,
            );
        }

        let mut ids = Vec::new();
        for i in 0..200 {
            let id = format!("fact_limit_{i:04}");
            conn.execute(
                "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
                 VALUES (?1, 'ent_limit', 't', 'b', 1, 1)",
                [&id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at)
                 VALUES (?1, 'prop_limit', '{}', 1)",
                [&id],
            )
            .unwrap();
            ids.push(id);
        }

        let removed = delete_librarian_evidence(&conn, &ids).unwrap();
        assert_eq!(removed, 200, "every evidence row must be deleted");

        let left: i64 = conn
            .query_row("SELECT count(*) FROM librarian_evidence", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn evidence_has_live_chunk_detects_dangling_anchors() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES ('d.md','h','user_doc','indexed')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy,
                 entity_id, content_hash)
             VALUES (?1,'c',0,1,1,'prose','ent','hash_live')",
            [doc_id],
        )
        .unwrap();
        let chunk_id: i64 = conn.last_insert_rowid();

        let live = format!(
            r#"{{"evidence":[{{"chunk_id":{chunk_id},"content_hash":"hash_live"}}],"proposal_id":"p"}}"#
        );
        assert!(evidence_has_live_chunk(&conn, &live).unwrap());

        let dangling =
            r#"{"evidence":[{"chunk_id":999999,"content_hash":"hash_gone"}],"proposal_id":"p"}"#;
        assert!(!evidence_has_live_chunk(&conn, dangling).unwrap());

        // A hash present but unmatched means the content is gone: no fallback
        // to chunk_id, even when a chunk with that rowid exists (rowid reuse
        // must not read as live anchoring).
        let hash_miss_rowid_live = format!(
            r#"{{"evidence":[{{"chunk_id":{chunk_id},"content_hash":"hash_gone"}}],"proposal_id":"p"}}"#
        );
        assert!(!evidence_has_live_chunk(&conn, &hash_miss_rowid_live).unwrap());

        // Legacy evidence with no usable hash still falls back to chunk_id.
        let no_hash_rowid_live =
            format!(r#"{{"evidence":[{{"chunk_id":{chunk_id}}}],"proposal_id":"p"}}"#);
        assert!(evidence_has_live_chunk(&conn, &no_hash_rowid_live).unwrap());

        let empty = r#"{"evidence":[],"proposal_id":"p"}"#;
        assert!(!evidence_has_live_chunk(&conn, empty).unwrap());
    }

    pub(super) fn seed_document(conn: &Connection, path: &str) -> i64 {
        upsert_document(conn, path, "hash").unwrap()
    }

    pub(super) fn seed_chunk(conn: &Connection, doc_id: i64) -> i64 {
        let chunk = Chunk {
            text: "evidence".into(),
            start_line: 1,
            end_line: 2,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap()
    }

    fn seed_entity(conn: &Connection, id: &str, name: &str, summary: &str, updated_at: i64) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, 'concept', ?3, ?4, ?4)",
            params![id, name, summary, updated_at],
        )
        .unwrap();
    }

    /// Inserts a live `llm_wiki_entries` row directly. `deleted_at` is NULL.
    fn seed_fact_row(conn: &Connection, id: &str, entity_id: &str, body: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, NULL, NULL)",
            params![id, entity_id, body, body],
        )
        .unwrap();
    }

    fn seed_edge_row(conn: &Connection, id: &str, entity_id: &str, source: &str, target: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4, 'related_to', 1757000000000)",
            params![id, entity_id, source, target],
        )
        .unwrap();
    }

    /// A minimal `CommitContext` for exercising a single commit_* function
    /// directly, without going through a whole proposal.
    fn test_ctx(entity_id: &str) -> CommitContext {
        CommitContext {
            deposit_default_tier: crate::config::DEFAULT_DEPOSIT_TIER.to_string(),
            edge_endpoint_strict: std::collections::HashMap::new(),
            edge_endpoint_optout: std::collections::HashMap::new(),
            owner_edge_vocabulary: None,
            proposal_id: "prop-test".into(),
            proposal_created_at: 100,
            entity_id: entity_id.to_string(),
            entity_name: "Test Entity".into(),
            source_type: "librarian_inferred",
            now_secs: 200,
            now_ms: 200_000,
            committed: Vec::new(),
            conflicts: Vec::new(),
            dropped_edges: Vec::new(),
            accepted_count: 0,
            rejected_count: 0,
            facts_added: 0,
            facts_updated: 0,
            facts_archived: 0,
            tasks_added: 0,
            facts_duplicated: 0,
            skipped_unanchored: 0,
            entry_embeddings: std::collections::HashMap::new(),
            reviewed_by: None,
        }
    }

    fn test_item(id: &str, item_type: &str, target_id: Option<&str>) -> LoadedItem {
        LoadedItem {
            id: id.into(),
            item_type: item_type.into(),
            target_id: target_id.map(|s| s.to_string()),
            payload: serde_json::json!({}),
            evidence: Vec::new(),
            edited_payload: None,
        }
    }

    /// Build the `LoadedItem` a fact_add commit consumes. `evidence` carries the
    /// chunk anchors; an empty vec is the unanchored case.
    fn fact_add_item(evidence: Vec<StoredEvidenceChunk>) -> LoadedItem {
        fact_add_item_with_body("A fact worth storing.", evidence)
    }

    /// `fact_add_item` with a caller-chosen payload body — distinct bodies
    /// keep phase-1 dedupe out of the way in multi-commit tests.
    fn fact_add_item_with_body(body: &str, evidence: Vec<StoredEvidenceChunk>) -> LoadedItem {
        LoadedItem {
            id: "item_t".into(),
            item_type: "fact_add".into(),
            target_id: None,
            payload: serde_json::json!({
                "body": body,
                "tags": [],
                "confidence": "inferred"
            }),
            evidence,
            edited_payload: None,
        }
    }

    /// Test (c), spec §4 (write path): the fact_add INSERT stamps
    /// `embed_scheme = WRITE_SCHEME` whether or not the row carries a blob,
    /// and a precomputed vector passes parity only when it was embedded from
    /// the WRITE-scheme text — the raw-text vector is discarded (NULL → the
    /// sweep re-embeds). Each commit uses a DISTINCT payload body so the
    /// phase-1 dedupe never masks the scheme assertions.
    #[test]
    fn fact_add_stamps_embed_scheme_and_parity_uses_the_write_scheme_text() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-sc", "Test Entity", "summary", 100);
        let doc_id = seed_document(&conn, "notes.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        let content_hash: String = conn
            .query_row(
                "SELECT content_hash FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();

        let body = "A scheme-stamped fact.";
        let title = fact_title_from_body(body);
        let mut ctx = test_ctx("ent-sc");
        // A vector embedded from the RAW (pre-#265) text function: parity
        // under the WRITE scheme must reject it.
        ctx.entry_embeddings.insert(
            "item_t".to_string(),
            PrecomputedEmbedding {
                embed_text: format!("{title}\n\n{body}"),
                vector: vec![0.5_f32; 8],
            },
        );
        let item = fact_add_item_with_body(
            "A scheme-stamped fact.",
            vec![StoredEvidenceChunk {
                chunk_id: Some(chunk_id),
                content_hash,
                quote: "evidence".into(),
                start_line: Some(1),
                end_line: Some(2),
                source_kind: None,
            }],
        );
        let outcome = commit_fact_add(&conn, &mut ctx, &item, &item.payload).unwrap();
        assert!(matches!(outcome, FactAddOutcome::Applied));
        let entry_id = ctx.committed.last().unwrap().record_id.clone();

        // Raw-text vector discarded: NULL blob for the sweep to re-embed.
        let (blob, scheme): (Option<Vec<u8>>, String) = conn
            .query_row(
                "SELECT embedding_blob, embed_scheme FROM llm_wiki_entries WHERE id = ?1",
                [&entry_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            blob, None,
            "a raw-text vector must fail WRITE-scheme parity"
        );
        assert_eq!(scheme, crate::embed_scheme::WRITE_SCHEME);

        // Now a vector embedded from the WRITE-scheme text: parity accepts it.
        let mut ctx2 = test_ctx("ent-sc");
        let chunk2 = seed_chunk(&conn, doc_id);
        let hash2: String = conn
            .query_row(
                "SELECT content_hash FROM chunks WHERE id = ?1",
                [chunk2],
                |r| r.get(0),
            )
            .unwrap();
        // The WRITE-scheme text of the SECOND fact's own body/title.
        let body2 = "A second, distinct scheme-stamped fact.";
        let title2 = fact_title_from_body(body2);
        ctx2.entry_embeddings.insert(
            "item_t".to_string(),
            PrecomputedEmbedding {
                embed_text: crate::embed_sweep::embed_text_for_entry(&title2, body2),
                vector: vec![0.25_f32; 8],
            },
        );
        let item2 = fact_add_item_with_body(
            "A second, distinct scheme-stamped fact.",
            vec![StoredEvidenceChunk {
                chunk_id: Some(chunk2),
                content_hash: hash2,
                quote: "evidence".into(),
                start_line: Some(1),
                end_line: Some(2),
                source_kind: None,
            }],
        );
        let outcome2 = commit_fact_add(&conn, &mut ctx2, &item2, &item2.payload).unwrap();
        assert!(matches!(outcome2, FactAddOutcome::Applied));
        let entry_id2 = ctx2.committed.last().unwrap().record_id.clone();

        let (blob2, scheme2): (Option<Vec<u8>>, String) = conn
            .query_row(
                "SELECT embedding_blob, embed_scheme FROM llm_wiki_entries WHERE id = ?1",
                [&entry_id2],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            blob2,
            Some(crate::wiki_graph::f32_vec_to_blob(&[0.25_f32; 8])),
            "the WRITE-scheme vector must pass parity and land"
        );
        assert_eq!(scheme2, crate::embed_scheme::WRITE_SCHEME);
    }

    /// Test (c), spec §4: the fact_update paths re-stamp `embed_scheme` in the
    /// same statement that writes the blob — a fresh vector re-stamps to the
    /// WRITE scheme, and a row whose body changed lands consistent even when
    /// the blob goes NULL for the sweep.
    #[test]
    fn fact_update_re_stamps_embed_scheme_with_the_blob() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-su", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_su", "ent-su", "The original body.");
        // Stage a pre-#265 blob under the raw scheme.
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fact_su'",
            params![vec![1u8; 32]],
        )
        .unwrap();

        // Body changed, no fresh vector: blob NULLed AND scheme re-stamped —
        // never a stale raw blob/stamp pair stranded for the sweep to trip on.
        let mut ctx = test_ctx("ent-su");
        let item = test_item("item-1", "fact_update", Some("fact_su"));
        let payload = serde_json::json!({ "body": "A completely different body." });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();
        let (blob, scheme): (Option<Vec<u8>>, String) = conn
            .query_row(
                "SELECT embedding_blob, embed_scheme FROM llm_wiki_entries WHERE id = 'fact_su'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(blob, None);
        assert_eq!(scheme, crate::embed_scheme::WRITE_SCHEME);

        // Fresh WRITE-scheme vector on an unchanged body: the COALESCE branch
        // writes both blob and stamp.
        let body = "The original body.";
        let mut ctx2 = test_ctx("ent-su");
        ctx2.entry_embeddings.insert(
            "item-2".to_string(),
            PrecomputedEmbedding {
                embed_text: crate::embed_sweep::embed_text_for_entry(
                    &fact_title_from_body(body),
                    body,
                ),
                vector: vec![0.5_f32; 8],
            },
        );
        let item2 = test_item("item-2", "fact_update", Some("fact_su"));
        let payload2 = serde_json::json!({ "body": body });
        commit_fact_update(&conn, &mut ctx2, &item2, &payload2).unwrap();
        let (blob2, scheme2): (Option<Vec<u8>>, String) = conn
            .query_row(
                "SELECT embedding_blob, embed_scheme FROM llm_wiki_entries WHERE id = 'fact_su'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            blob2,
            Some(crate::wiki_graph::f32_vec_to_blob(&[0.5_f32; 8]))
        );
        assert_eq!(scheme2, crate::embed_scheme::WRITE_SCHEME);
    }

    #[test]
    fn commit_fact_add_writes_token_and_evidence_row() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-t", "Test Entity", "summary", 100);
        let doc_id = seed_document(&conn, "notes.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        let content_hash: String = conn
            .query_row(
                "SELECT content_hash FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();

        let mut ctx = test_ctx("ent-t");
        let item = fact_add_item(vec![StoredEvidenceChunk {
            chunk_id: Some(chunk_id),
            content_hash,
            quote: "evidence".into(),
            start_line: Some(1),
            end_line: Some(2),
            source_kind: None,
        }]);
        let outcome = commit_fact_add(&conn, &mut ctx, &item, &item.payload).unwrap();
        assert!(matches!(outcome, FactAddOutcome::Applied));

        let entry_id = ctx.committed.last().unwrap().record_id.clone();

        let source_ref: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = ?1",
                [&entry_id],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(source_ref, librarian_source_ref_token(&entry_id));
        assert!(!engine_would_rewrite(&source_ref));
        assert!(
            !source_ref.contains('{'),
            "no JSON may remain in source_ref: {source_ref}"
        );

        let stored = evidence_json_for_entry(&conn, &entry_id)
            .unwrap()
            .expect("evidence row must exist");
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert!(parsed.get("evidence").and_then(|v| v.as_array()).is_some());
        assert!(parsed.get("proposal_id").is_some());
    }

    /// Phase-2 strict gate (spec §2.4): a fact whose evidence anchors no
    /// surviving chunk is NOT written. Direct-`commit_fact_add` check that the
    /// skip is total: no entry, no evidence row, no outbox row.
    #[test]
    fn phase2_commit_fact_add_skips_unanchored() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-u", "Test Entity", "summary", 100);
        // No document, no chunk: the evidence references a chunk that is not there.
        let mut ctx = test_ctx("ent-u");
        let item = fact_add_item(vec![StoredEvidenceChunk {
            chunk_id: Some(999_999),
            content_hash: "nosuchhash".into(),
            quote: "dangling".into(),
            start_line: Some(1),
            end_line: Some(2),
            source_kind: None,
        }]);
        let outcome = commit_fact_add(&conn, &mut ctx, &item, &item.payload).unwrap();
        assert!(matches!(outcome, FactAddOutcome::SkippedUnanchored));
        assert_eq!(ctx.skipped_unanchored, 1);

        // Nothing written — no entry, no evidence row, no outbox row.
        for table in ["llm_wiki_entries", "librarian_evidence", "llm_wiki_outbox"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} must be untouched on phase2 skip");
        }
    }

    /// The anchored twin of `phase2_commit_fact_add_skips_unanchored`: a live
    /// chunk anchor still writes normally, flagged unanchored=0, and the skip
    /// counter stays at zero. (Covers the old
    /// `commit_fact_add_flags_unanchored_evidence` intent that anchored facts
    /// are unaffected by the gate.)
    #[test]
    fn phase2_anchored_facts_still_written_and_counter_zero() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-a", "Test Entity", "summary", 100);
        let doc_id = seed_document(&conn, "notes.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        let content_hash: String = conn
            .query_row(
                "SELECT content_hash FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();

        let mut ctx = test_ctx("ent-a");
        let item = fact_add_item(vec![StoredEvidenceChunk {
            chunk_id: Some(chunk_id),
            content_hash,
            quote: "evidence".into(),
            start_line: Some(1),
            end_line: Some(2),
            source_kind: None,
        }]);
        let outcome = commit_fact_add(&conn, &mut ctx, &item, &item.payload).unwrap();
        assert!(matches!(outcome, FactAddOutcome::Applied));
        assert_eq!(ctx.skipped_unanchored, 0);

        let entry_id = ctx.committed.last().unwrap().record_id.clone();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = ?1",
                [&entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "anchored fact must still be written");

        let unanchored: i64 = conn
            .query_row(
                "SELECT unanchored FROM librarian_evidence WHERE entry_id = ?1",
                [&entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unanchored, 0, "anchored fact must be flagged unanchored=0");
    }

    /// Resolve-level Phase-2 contract (spec §2.4): the skipped item flows
    /// through `ItemCommitOutcome::Rejected`, so it lands as status='rejected'
    /// on `curated_proposal_items`, increments `CommitResult.skipped_unanchored`,
    /// and the resolution-event summary carries the skipped clause (same shape
    /// as the duplicates clause).
    #[test]
    fn phase2_resolve_rejects_unanchored_item_and_counts_skip() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        // Evidence chunk_id dangles: no chunk row with that id exists, so
        // evidence_has_live_chunk is false through both the hash and id paths.
        insert_test_proposal(
            &conn,
            "prop-u1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "item-1".into(),
                item_type: "fact_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "body": "A fact worth storing.",
                    "tags": [],
                    "confidence": "inferred"
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(999_999),
                    content_hash: "nosuchhash".into(),
                    quote: "dangling".into(),
                    start_line: Some(1),
                    end_line: Some(2),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-u1",
            &[ItemDecision {
                item_id: "item-1".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.skipped_unanchored, 1);
        assert_eq!(result.committed.len(), 0);

        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposal_items WHERE id = 'item-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "rejected");

        // Resolution-event summary carries the skipped clause (duplicates shape).
        let summary: String = conn
            .query_row(
                "SELECT summary FROM llm_wiki_events ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            summary.contains("unanchored"),
            "resolution summary must surface the phase2 skip, got: {summary}"
        );
    }

    /// Empty-shell rollback (PR #201 review finding 2): `accepted_any` is
    /// DECISION-based, so an approved new_entity whose only fact_add is
    /// skipped by the Phase-2 gate has already minted the entity and stamped
    /// `proposals.entity_id` by the time the item commits. The rollback must
    /// undo both — the proposal resolves to 'rejected' with NO
    /// `curated_entities` row and NO resolution event, exactly like the
    /// all-reject path that never created an entity.
    #[test]
    fn phase2_unanchored_new_entity_rolls_back_the_minted_shell() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/shell.pdf");

        // Evidence chunk_id dangles: the Phase-2 gate skips the only item.
        insert_test_proposal(
            &conn,
            "prop-shell",
            ProposalKind::NewEntity,
            None,
            vec![NewProposalItem {
                id: "item-1".into(),
                item_type: "fact_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "body": "A fact worth storing.",
                    "tags": [],
                    "confidence": "inferred"
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(999_999),
                    content_hash: "nosuchhash".into(),
                    quote: "dangling".into(),
                    start_line: Some(1),
                    end_line: Some(2),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-shell",
            &[ItemDecision {
                item_id: "item-1".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.skipped_unanchored, 1);
        assert_eq!(result.proposal_status, "rejected");

        let (entities, stamped_id): (i64, Option<String>) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM curated_entities),
                        (SELECT entity_id FROM curated_proposals WHERE id = 'prop-shell')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(entities, 0, "the minted shell entity must be rolled back");
        assert_eq!(stamped_id, None, "proposals.entity_id must be unstamped");

        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            events, 0,
            "no resolution event for an entity whose only content was refused"
        );
    }

    #[test]
    fn archiving_a_fact_purges_its_edges() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "Body A");
        seed_fact_row(&conn, "fact_b", "ent-1", "Body B");
        seed_fact_row(&conn, "fact_c", "ent-1", "Body C");
        seed_edge_row(&conn, "edge_out", "ent-1", "fact_a", "fact_b");
        seed_edge_row(&conn, "edge_in", "ent-1", "fact_c", "fact_a");
        seed_edge_row(&conn, "edge_other", "ent-1", "fact_b", "fact_c");

        // R1 (remediation): the new heterogeneous contract only purges edges
        // whose partner is also dead in every endpoint table. Soft-delete
        // fact_b and fact_c so the cascade treats their edges as purgeable.
        // Without this, edge_out and edge_in would survive because fact_b /
        // fact_c remain alive in `llm_wiki_entries`.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id IN ('fact_b', 'fact_c')",
            [],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        let item = test_item("item-1", "fact_archive", Some("fact_a"));
        commit_fact_archive(&conn, &mut ctx, &item).unwrap();

        // The entry is soft-deleted...
        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(deleted_at, Some(200_000), "deleted_at is milliseconds");

        // ...and no edge references it any more.
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges
                  WHERE source_id = 'fact_a' OR target_id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0, "archived entry must leave no ghost edges");

        // edge_other (fact_b → fact_c) is not touched by the cascade from
        // fact_a because fact_a is on neither endpoint. Under the R1
        // contract it survives even though both its partners are soft-
        // deleted; the broader `purge_orphan_edges` would clean it up.
        let survivors: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(survivors, 1);
    }

    #[test]
    fn updating_a_body_replaces_the_stale_embedding() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "The original body.");
        // Give it a blob describing the ORIGINAL text.
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fact_a'",
            params![vec![1u8; 32]],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        // No pre-computed embedding for this item -> the stale blob must be
        // cleared, not left describing text the entry no longer has.
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        let payload = serde_json::json!({ "body": "A completely different body." });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob, None,
            "a changed body must never keep the vector of the old text"
        );
    }

    /// Review finding (r13-m3 transitive closure): the target fact may still
    /// be keyed to a redirected loser from before a merge. The update must
    /// match the CLUSTER and rekey the row to the survivor — a bare
    /// `entity_id = ?` bail rolls back the whole resolution and wedges the
    /// auto-approve retry loop on every later run.
    #[test]
    fn fact_update_targets_loser_keyed_row_and_rekeys_to_survivor() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Survivor", "Summary", 100);
        // fact_a is still keyed to the merged-away loser.
        seed_fact_row(&conn, "fact_a", "ent-loser", "The original body.");
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent-loser', 'ent-1', 1)",
            [],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        let payload = serde_json::json!({ "body": "The rekeyed body." });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let (entity_id, body): (String, String) = conn
            .query_row(
                "SELECT entity_id, body FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            entity_id, "ent-1",
            "a loser-keyed target is matched across the cluster and rekeyed"
        );
        assert_eq!(body, "The rekeyed body.");
    }

    /// Same rule for archive (review finding): a loser-keyed fact_archive
    /// target must archive instead of bailing the resolution, and rekey to
    /// the survivor so the row agrees with its Delete outbox payload (#132).
    #[test]
    fn fact_archive_targets_loser_keyed_row() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Survivor", "Summary", 100);
        seed_fact_row(&conn, "fact_b", "ent-loser", "The original body.");
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent-loser', 'ent-1', 1)",
            [],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        let item = test_item("item-2", "fact_archive", Some("fact_b"));
        commit_fact_archive(&conn, &mut ctx, &item).unwrap();

        let (entity_id, deleted_at): (String, Option<i64>) = conn
            .query_row(
                "SELECT entity_id, deleted_at FROM llm_wiki_entries WHERE id = 'fact_b'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(deleted_at.is_some(), "the loser-keyed row is archived");
        assert_eq!(
            entity_id, "ent-1",
            "the archived row is rekeyed to the survivor"
        );
        let outbox_payload: String = conn
            .query_row(
                "SELECT payload FROM llm_wiki_outbox WHERE record_id = 'fact_b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&outbox_payload).unwrap();
        assert_eq!(
            payload["entity_id"], "ent-1",
            "outbox owner matches the row"
        );
    }

    #[test]
    fn updating_a_body_stores_a_fresh_embedding_when_one_was_precomputed() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "The original body.");
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fact_a'",
            params![vec![1u8; 32]],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        let new_body = "A completely different body.";
        ctx.entry_embeddings.insert(
            "item-1".to_string(),
            PrecomputedEmbedding {
                embed_text: crate::embed_sweep::embed_text_for_entry(
                    &fact_title_from_body(new_body),
                    new_body,
                ),
                vector: vec![0.5_f32; 8],
            },
        );
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        let payload = serde_json::json!({ "body": new_body });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob,
            Some(crate::wiki_graph::f32_vec_to_blob(&[0.5_f32; 8])),
            "the fresh vector replaces the stale one"
        );
    }

    #[test]
    fn a_precomputed_vector_for_a_different_body_is_discarded() {
        // `resolve_proposal_cmd` embeds phase-1's payload, drops the DbState
        // mutex, then re-loads items in phase 3. If the payload changed in
        // between, the map still holds a vector for the OLD text keyed by the
        // same item id. Persisting it would make the entry match queries for
        // text it no longer contains — worse than NULL, which the sweep fixes.
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "The original body.");

        let mut ctx = test_ctx("ent-1");
        let stale_body = "The body that was embedded in phase one.";
        ctx.entry_embeddings.insert(
            "item-1".to_string(),
            PrecomputedEmbedding {
                embed_text: crate::embed_sweep::embed_text_for_entry(
                    &fact_title_from_body(stale_body),
                    stale_body,
                ),
                vector: vec![0.5_f32; 8],
            },
        );
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        // Phase 3 commits a DIFFERENT body than the one that was embedded.
        let payload = serde_json::json!({ "body": "The body phase three actually commits." });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob, None,
            "a vector computed from different text must never be persisted"
        );
    }

    #[test]
    fn an_unchanged_body_fills_a_null_blob_instead_of_discarding_the_vector() {
        // The row is NULL because an earlier write-time embed failed. A later
        // edit that does not change the body still carries a fresh, matching
        // vector — filling it here is free and keeps the entry searchable
        // without waiting for a sweep trigger this path never fires.
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        let body = "The body that does not change.";
        seed_fact_row(&conn, "fact_a", "ent-1", body);

        let mut ctx = test_ctx("ent-1");
        ctx.entry_embeddings.insert(
            "item-1".to_string(),
            PrecomputedEmbedding {
                embed_text: crate::embed_sweep::embed_text_for_entry(
                    &fact_title_from_body(body),
                    body,
                ),
                vector: vec![0.25_f32; 8],
            },
        );
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        let payload = serde_json::json!({ "body": body });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob,
            Some(crate::wiki_graph::f32_vec_to_blob(&[0.25_f32; 8])),
            "an unchanged body should take the fresh vector rather than stay NULL"
        );
    }

    #[test]
    fn an_unchanged_body_keeps_its_existing_blob_when_no_vector_was_precomputed() {
        // COALESCE must not clobber a good vector with NULL when the provider
        // was down and the precompute produced nothing.
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        let body = "The body that does not change.";
        seed_fact_row(&conn, "fact_a", "ent-1", body);
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fact_a'",
            params![vec![7u8; 32]],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        let payload = serde_json::json!({ "body": body });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob,
            Some(vec![7u8; 32]),
            "the existing vector still describes this body and must survive"
        );
    }

    #[test]
    fn archiving_pushes_no_edge_outbox_rows() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "Body A");
        seed_fact_row(&conn, "fact_b", "ent-1", "Body B");
        seed_edge_row(&conn, "edge_ab", "ent-1", "fact_a", "fact_b");

        let mut ctx = test_ctx("ent-1");
        let item = test_item("item-1", "fact_archive", Some("fact_a"));
        commit_fact_archive(&conn, &mut ctx, &item).unwrap();

        // Edges are not replicated (spec §2). Only the entries delete is in the outbox.
        let edge_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox WHERE table_name = 'edges'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(edge_rows, 0, "edge purges must not emit outbox rows");
    }

    fn insert_test_proposal(
        conn: &Connection,
        id: &str,
        kind: ProposalKind,
        entity_id: Option<&str>,
        items: Vec<NewProposalItem>,
        doc_id: i64,
    ) {
        insert_proposal(
            conn,
            &NewProposal {
                id: id.into(),
                kind,
                entity_id: entity_id.map(str::to_string),
                proposed_name: Some("Project X".into()),
                proposed_type: Some("project".into()),
                reasoning: Some("Because.".into()),
                model: "test".into(),
            },
            &items,
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();
    }

    fn fact_item(id: &str, chunk_id: i64, body: &str) -> NewProposalItem {
        NewProposalItem {
            id: id.into(),
            item_type: "fact_add".into(),
            target_id: None,
            payload: serde_json::json!({ "body": body, "tags": [], "confidence": "inferred" }),
            evidence: vec![StoredEvidenceChunk {
                chunk_id: Some(chunk_id),
                content_hash: String::new(),
                quote: "evidence".into(),
                start_line: Some(1),
                end_line: Some(2),
                source_kind: None,
            }],
        }
    }

    // ── HVG Task 2: reviewed_by stamp + in-tx pending guard ────────────────

    /// Seed a pending NewEntity proposal with one anchored fact_add item,
    /// reusing the suite's existing fixtures verbatim (same shape as the
    /// `:2904` status tests).
    fn seed_pending_proposal(conn: &Connection, id: &str) {
        let doc_id = seed_document(conn, "/vault/documents/hvg.pdf");
        let chunk_id = seed_chunk(conn, doc_id);
        insert_test_proposal(
            conn,
            id,
            ProposalKind::NewEntity,
            None,
            vec![fact_item(
                &format!("item-{id}"),
                chunk_id,
                "A verified fact.",
            )],
            doc_id,
        );
    }

    fn all_accept_decisions(conn: &Connection, proposal_id: &str) -> Vec<ItemDecision> {
        let item_ids: Vec<String> = {
            let items = load_items(conn, proposal_id).unwrap();
            items.into_iter().map(|item| item.id).collect()
        };
        item_ids
            .iter()
            .map(|item_id| ItemDecision {
                item_id: item_id.clone(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            })
            .collect()
    }

    /// Spec §2: the reviewer identity carried on ResolveOptions must be
    /// stamped into `curated_proposals.reviewed_by` on resolution, and the
    /// entries must land as `user_confirmed` (manual review path).
    #[test]
    fn resolve_proposal_stamps_reviewed_by_on_resolution() {
        let mut conn = open_in_memory().unwrap();
        seed_pending_proposal(&conn, "prop-hvg-1");

        let decisions = all_accept_decisions(&conn, "prop-hvg-1");
        let result = resolve_proposal(
            &mut conn,
            "prop-hvg-1",
            &decisions,
            None,
            ResolveOptions {
                auto_approve: false,
                reviewed_by: Some("tessera".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let (rb, st): (Option<String>, String) = conn
            .query_row(
                "SELECT reviewed_by, status FROM curated_proposals WHERE id = 'prop-hvg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rb.as_deref(), Some("tessera"));
        assert_eq!(st, "approved");
        assert_eq!(result.proposal_status, "approved");

        let ut: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'user_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ut >= 1, "manual resolution must stamp user_confirmed");
    }

    /// Characterization of the auto path: no reviewer on ResolveOptions →
    /// `reviewed_by` stays NULL.
    #[test]
    fn resolve_proposal_leaves_reviewed_by_null_when_absent() {
        let mut conn = open_in_memory().unwrap();
        seed_pending_proposal(&conn, "prop-hvg-2");

        let decisions = all_accept_decisions(&conn, "prop-hvg-2");
        resolve_proposal(
            &mut conn,
            "prop-hvg-2",
            &decisions,
            None,
            ResolveOptions {
                auto_approve: true,
                ..Default::default()
            },
        )
        .unwrap();

        let rb: Option<String> = conn
            .query_row(
                "SELECT reviewed_by FROM curated_proposals WHERE id = 'prop-hvg-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(rb.is_none(), "reviewed_by must stay NULL when absent");
    }

    /// The existing `resolve_proposal` pre-check (:2023) must reject a second
    /// sequential resolve of the same proposal.
    #[test]
    fn resolve_proposal_sequential_double_resolve_fails_at_precheck() {
        let mut conn = open_in_memory().unwrap();
        seed_pending_proposal(&conn, "prop-hvg-3");
        let decisions = all_accept_decisions(&conn, "prop-hvg-3");

        resolve_proposal(
            &mut conn,
            "prop-hvg-3",
            &decisions,
            None,
            ResolveOptions::default(),
        )
        .unwrap();

        let err = resolve_proposal(
            &mut conn,
            "prop-hvg-3",
            &decisions,
            None,
            ResolveOptions::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("pending"),
            "second resolve must fail at the pre-check with 'pending', got: {err}"
        );
    }

    /// Spec T-1 simulated concurrency: extract the guarded final UPDATE into
    /// `finalize_proposal_status_guarded` and drive it directly against a row
    /// already flipped to 'approved' by a second connection. The guarded
    /// UPDATE matches 0 rows → the caller bails "already resolved" and the
    /// transaction (event rows, edge writes) rolls back.
    #[test]
    fn resolve_proposal_concurrent_double_resolve_fails_at_guard() {
        let conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/hvg-c.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-hvg", "HVG Entity", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-hvg-4",
            ProposalKind::UpdateEntity,
            Some("ent-hvg"),
            vec![fact_item("item-hvg-4", chunk_id, "A contested fact.")],
            doc_id,
        );

        let tx = conn.unchecked_transaction().unwrap();
        // "conn B" wins the race: the proposal is no longer pending.
        tx.execute(
            "UPDATE curated_proposals SET status = 'approved', resolved_at = 1 WHERE id = 'prop-hvg-4'",
            [],
        )
        .unwrap();

        let _ = load_items(&conn, "prop-hvg-4").unwrap();
        let mut ctx = test_ctx("ent-hvg");
        ctx.proposal_id = "prop-hvg-4".into();

        let err =
            finalize_proposal_status_guarded(&tx, &mut ctx, "approved", 999, None, "prop-hvg-4")
                .unwrap_err();
        assert!(
            err.to_string().contains("already resolved"),
            "guard must bail with 'already resolved', got: {err}"
        );

        // No second resolution event may have been written by the loser.
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events WHERE entity_id = 'ent-hvg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            events, 0,
            "no resolution event may be written on guard bail"
        );

        // The loser's transaction must leave no edge/item residue behind.
        let edges: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE entity_id = 'ent-hvg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(edges, 0, "edge count must be unchanged on guard bail");

        let accepted_items: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM curated_proposal_items WHERE proposal_id = 'prop-hvg-4' AND status = 'accepted'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            accepted_items, 0,
            "no item may be marked accepted by the loser"
        );
    }

    #[test]
    fn resolve_new_entity_creates_entity_and_fact_with_outbox() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/notes.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        insert_test_proposal(
            &conn,
            "prop-1",
            ProposalKind::NewEntity,
            None,
            vec![fact_item("item-1", chunk_id, "A new fact.")],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-1",
            &[ItemDecision {
                item_id: "item-1".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.proposal_status, "approved");
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].table, "entries");

        let entity_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entity_count, 1);

        let fact_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'user_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fact_count, 1);

        let outbox_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox WHERE table_name = 'entries' AND operation = 'INSERT'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outbox_count, 1);

        // The proposal-insert outbox payload must carry the persisted
        // lifecycle_status (defaults to "stable" for newly committed facts).
        let payload_lifecycle: String = conn
            .query_row(
                "SELECT json_extract(payload, '$.lifecycle_status')
                 FROM llm_wiki_outbox
                 WHERE table_name = 'entries' AND operation = 'INSERT'
                 ORDER BY id ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(payload_lifecycle, "stable");

        let event_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(event_count, 1);
    }

    #[test]
    fn partial_approval_marks_proposal_partial() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-partial",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![
                fact_item("item-a", chunk_id, "Keep me."),
                fact_item("item-b", chunk_id, "Drop me."),
            ],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-partial",
            &[
                ItemDecision {
                    item_id: "item-a".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                },
                ItemDecision {
                    item_id: "item-b".into(),
                    decision: ItemDecisionKind::Reject,
                    edited_payload: None,
                },
            ],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.proposal_status, "partial");
        assert_eq!(result.committed.len(), 1);

        let accepted: String = conn
            .query_row(
                "SELECT status FROM curated_proposal_items WHERE id = 'item-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(accepted, "accepted");
    }

    /// Spec §3.2 writer contract: an entry whose evidence is deposit-origin is
    /// stamped with the configured deposit tier at write time. Before this,
    /// `wiki.deposit_default_tier` had no writer at all — every INSERT left
    /// `tier` NULL and the setting was inert until the offline backfill ran.
    #[test]
    fn deposit_origin_fact_add_is_stamped_with_the_configured_tier() {
        for configured in ["wisdom", "fact"] {
            let mut conn = open_in_memory().unwrap();
            // Absolute path — the shape the ingest walker actually writes.
            let doc_id = seed_document(
                &conn,
                "/Users/x/Vault/immutable-source-files/agents/deposit.md",
            );
            let chunk_id = seed_chunk(&conn, doc_id);
            seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
            insert_test_proposal(
                &conn,
                "prop-deposit",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![fact_item("item-a", chunk_id, "A deposited note.")],
                doc_id,
            );

            resolve_proposal(
                &mut conn,
                "prop-deposit",
                &[ItemDecision {
                    item_id: "item-a".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                }],
                None,
                ResolveOptions {
                    auto_approve: false,
                    embed_profile: None,
                    deposit_default_tier: Some(configured.to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            let tier: Option<String> = conn
                .query_row("SELECT tier FROM llm_wiki_entries LIMIT 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(
                tier.as_deref(),
                Some(configured),
                "a deposit-origin entry must carry the configured tier"
            );
        }
    }

    /// The other half of the contract: provenance that is not certainly a
    /// deposit stays NULL rather than being guessed at.
    #[test]
    fn non_deposit_fact_add_stays_unclassified() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/Users/x/Vault/immutable-source-files/spec.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-spec",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("item-a", chunk_id, "A spec fact.")],
            doc_id,
        );

        resolve_proposal(
            &mut conn,
            "prop-spec",
            &[ItemDecision {
                item_id: "item-a".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                deposit_default_tier: Some("fact".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tier, None, "non-deposit provenance must stay NULL");
    }

    /// A sibling directory sharing the deposit prefix must not be treated as a
    /// deposit — the separator is what makes the test a segment test.
    #[test]
    fn prefix_sibling_directory_is_not_deposit_origin() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(
            &conn,
            "/Users/x/Vault/immutable-source-files/agents-but-not-really/note.md",
        );
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-sib",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("item-a", chunk_id, "Not a deposit.")],
            doc_id,
        );

        resolve_proposal(
            &mut conn,
            "prop-sib",
            &[ItemDecision {
                item_id: "item-a".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                deposit_default_tier: Some("wisdom".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            tier, None,
            "a prefix sibling must not classify as a deposit"
        );
    }

    #[test]
    fn edited_payload_wins_over_stored_payload() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-edit",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("item-a", chunk_id, "Original body.")],
            doc_id,
        );

        resolve_proposal(
            &mut conn,
            "prop-edit",
            &[ItemDecision {
                item_id: "item-a".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: Some(serde_json::json!({
                    "body": "Edited body.",
                    "tags": ["edited"],
                    "confidence": "certain"
                })),
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        let body: String = conn
            .query_row("SELECT body FROM llm_wiki_entries LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(body, "Edited body.");
    }

    #[test]
    fn fact_update_succeeds_when_source_ref_is_null() {
        // Regression: imported facts can carry a NULL source_ref. The
        // row-mapping closure in commit_fact_update used to fail to
        // deserialize NULL into String before the update ran, so the
        // proposal resolution would error and the import + manual edit
        // couldn't be reconciled.
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        // Seed an entry whose source_ref is NULL (the path bundle_apply
        // can produce when a fact is imported without a `resource`).
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('fact-imported', 'ent-1', 'Original', 'Original body.',
                     '[]', 'inferred', 'librarian_inferred',
                     NULL, 100, 100)",
            [],
        )
        .unwrap();

        insert_test_proposal(
            &conn,
            "prop-null-ref",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "item-update-null-ref".into(),
                item_type: "fact_update".into(),
                target_id: Some("fact-imported".into()),
                payload: serde_json::json!({
                    "body": "Edited body.",
                    "tags": [],
                    "confidence": "inferred",
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-null-ref",
            &[ItemDecision {
                item_id: "item-update-null-ref".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .expect("fact_update must succeed when source_ref is NULL");

        assert_eq!(result.proposal_status, "approved");
        let body: String = conn
            .query_row(
                "SELECT body FROM llm_wiki_entries WHERE id = 'fact-imported'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(body, "Edited body.");
        // The outbox UPDATE payload should carry the coalesced empty string
        // (not error) so downstream consumers can decode the row.
        let payload_source_ref: String = conn
            .query_row(
                "SELECT json_extract(payload, '$.source_ref')
                 FROM llm_wiki_outbox
                 WHERE record_id = 'fact-imported' AND operation = 'UPDATE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(payload_source_ref, "");
    }

    #[test]
    fn summary_update_conflict_surfaces_for_manual_path() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let _chunk_id = seed_chunk(&conn, doc_id);

        conn.execute(
            "INSERT INTO curated_proposals (id, kind, entity_id, model, status, created_at)
             VALUES ('prop-conflict', 'update_entity', 'ent-1', 'test', 'pending', 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_items (id, proposal_id, item_type, payload, evidence)
             VALUES ('item-sum', 'prop-conflict', 'summary_update', '{\"summary\":\"New summary\"}', '[]')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role) VALUES ('prop-conflict', ?1, 'trigger')",
            [doc_id],
        )
        .unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Old summary", 200);

        let result = resolve_proposal(
            &mut conn,
            "prop-conflict",
            &[ItemDecision {
                item_id: "item-sum".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(result.conflicts.contains(&"item-sum".to_string()));
        assert_eq!(result.proposal_status, "rejected");

        let summary: String = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id = 'ent-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, "Old summary");
    }

    /// Install an ontology manifest for `entity_id`. `edge_types` are given as
    /// `(type, source_type, target_type)` triples — the shape core-llm-wiki
    /// persists.
    fn seed_manifest(
        conn: &Connection,
        entity_id: &str,
        mode: &str,
        node_types: &[&str],
        edge_types: &[(&str, &str, &str)],
    ) {
        let nodes: Vec<serde_json::Value> = node_types
            .iter()
            .map(|t| serde_json::json!({ "type": t, "description": "" }))
            .collect();
        let edges: Vec<serde_json::Value> = edge_types
            .iter()
            .map(|(t, s, d)| {
                serde_json::json!({
                    "type": t, "source_type": s, "target_type": d, "description": ""
                })
            })
            .collect();
        let manifest = serde_json::json!({ "node_types": nodes, "edge_types": edges }).to_string();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES (?1, ?2, ?3, 0)",
            params![entity_id, mode, manifest],
        )
        .unwrap();
    }

    /// Build a one-edge proposal between two existing facts.
    fn insert_edge_proposal(
        conn: &Connection,
        proposal_id: &str,
        entity_id: &str,
        edge_type: &str,
        target_fact_id: &str,
        doc_id: i64,
        chunk_id: i64,
    ) {
        insert_test_proposal(
            conn,
            proposal_id,
            ProposalKind::UpdateEntity,
            Some(entity_id),
            vec![NewProposalItem {
                id: "edge-1".into(),
                item_type: "edge_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "source": "self",
                    "target": { "existing_id": target_fact_id },
                    "edge_type": edge_type
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );
    }

    fn accept_edge(conn: &mut Connection, proposal_id: &str) -> CommitResult {
        resolve_proposal(
            conn,
            proposal_id,
            &[ItemDecision {
                item_id: "edge-1".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap()
    }

    /// Scaffolding for the §2.3 gate tests: an entity with two facts that an
    /// edge can legitimately connect.
    fn seed_linkable_entity(conn: &Connection) -> (i64, i64) {
        let doc_id = seed_document(conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(conn, doc_id);
        seed_entity(conn, "ent-1", "Existing", "Summary", 100);
        for (id, title) in [("fact-src", "Existing"), ("fact-dst", "Target Fact")] {
            conn.execute(
                "INSERT INTO llm_wiki_entries
                    (id, entity_id, title, body, tags, confidence, source_type,
                     created_at, updated_at)
                 VALUES (?1, 'ent-1', ?2, 'body', '[]', 'inferred',
                         'librarian_inferred', 100, 100)",
                params![id, title],
            )
            .unwrap();
        }
        (doc_id, chunk_id)
    }

    /// Spec §2.3: in strict mode a manifest-defined `edge_type` writes.
    #[test]
    fn strict_mode_admits_a_manifest_defined_edge_type() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn, "prop-ok", "ent-1", "supports", "fact-dst", doc_id, chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-ok");

        assert!(
            result.dropped_edges.is_empty(),
            "a declared edge type must not be dropped"
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE edge_type = 'supports'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    /// Spec §2.3: an off-manifest type is refused. It is dropped rather than
    /// raised — one hallucinated edge type must not discard a batch of good
    /// facts — and reported in `dropped_edges` so the drop is not silent.
    #[test]
    fn strict_mode_drops_an_edge_type_absent_from_the_manifest() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-bad",
            "ent-1",
            "invented_by_the_llm",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-bad");

        assert_eq!(result.dropped_edges, vec!["edge-1".to_string()]);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "an undeclared edge type must not be written");
    }

    /// Membership is case-insensitive, matching the engine's own
    /// `resolveEdgeDefinitions`. A guard stricter than the producer would
    /// reject types the librarian was told were legal.
    #[test]
    fn strict_mode_matches_edge_types_case_insensitively() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-case",
            "ent-1",
            "SUPPORTS",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-case");
        assert!(
            result.dropped_edges.is_empty(),
            "casing must not decide legality"
        );
    }

    /// Issue #189: the gate is case-insensitive by design, but the ROW must
    /// carry the manifest's canonical spelling. Before this, `dependson`
    /// passed the gate and was written verbatim, producing a case-variant
    /// duplicate under `UNIQUE(entity_id, source_id, target_id, edge_type)`.
    #[test]
    fn strict_mode_writes_the_manifest_canonical_spelling() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("dependsOn", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-case",
            "ent-1",
            "dependson",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-case");

        assert!(
            result.dropped_edges.is_empty(),
            "casing must not decide legality"
        );
        let stored: String = conn
            .query_row("SELECT edge_type FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            stored, "dependsOn",
            "the stored type must be the manifest's canonical spelling, not the candidate's"
        );
    }

    /// The exact-case candidate is already canonical and must pass through
    /// byte-identical — canonicalization must not normalize casing on its own
    /// authority, only to the manifest's declared spelling.
    #[test]
    fn strict_mode_leaves_an_already_canonical_type_unchanged() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("dependsOn", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-exact",
            "ent-1",
            "dependsOn",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        accept_edge(&mut conn, "prop-exact");

        let stored: String = conn
            .query_row("SELECT edge_type FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, "dependsOn");
    }

    /// An ungated brain has no vocabulary to canonicalize against, so the
    /// candidate is written exactly as proposed. Spec §2.2: inventing a
    /// casing rule for brains that opted out of the ontology would silently
    /// rewrite user data. This is a deliberate limit, not an oversight.
    #[test]
    fn ungated_mode_writes_the_candidate_spelling_verbatim() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        // Both lookups the resolver makes see a non-strict mode, so the gate
        // is genuinely off rather than accidentally empty.
        seed_manifest(&conn, "ent-1", "off", &[], &[]);
        seed_manifest(&conn, "tier_fact", "off", &[], &[]);
        insert_edge_proposal(
            &conn,
            "prop-free",
            "ent-1",
            "dependson",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        accept_edge(&mut conn, "prop-free");

        let stored: String = conn
            .query_row("SELECT edge_type FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, "dependson", "ungated writes are verbatim");
    }

    /// Issue #189: the Sep 6 librarian run wrote three `supersedes` edges
    /// between DISTINCT `ent_*` rows that share the name "Curated Thoughts"
    /// — dedupe artifacts with no semantic value. The writer must not encode
    /// entity duplication as graph edges; resolving the duplication is the
    /// entity-merge pass's job.
    #[test]
    fn same_name_curated_endpoints_are_dropped() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        // A second curated entity sharing `ent-1`'s name ("Existing").
        seed_entity(&conn, "ent-dupe", "Existing", "Duplicate summary", 100);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supersedes", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-dupe",
            "ent-1",
            "supersedes",
            "ent-dupe",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-dupe");

        assert_eq!(
            result.dropped_edges,
            vec!["edge-1".to_string()],
            "a same-name pair must be dropped AND reported, not silently skipped"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// A tombstoned same-name endpoint is dropped and reported, not written.
    ///
    /// Two independent gates now reach this verdict, and the assertions below
    /// cannot tell them apart: `resolve_edge_ref`'s endpoint-liveness check
    /// refuses the tombstoned endpoint first, and if it ever stopped doing so
    /// the #189 same-name guard would catch the pair — which is why
    /// `curated_entity_names` keeps reading tombstones. The behaviour under
    /// test is the outcome, and the outcome must hold either way.
    #[test]
    fn same_name_guard_sees_tombstoned_endpoints() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_entity(&conn, "ent-dupe", "Existing", "Duplicate summary", 100);
        conn.execute(
            "UPDATE curated_entities SET deleted_at = 1234 WHERE id = 'ent-dupe'",
            [],
        )
        .unwrap();
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supersedes", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-tomb",
            "ent-1",
            "supersedes",
            "ent-dupe",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-tomb");

        assert_eq!(
            result.dropped_edges,
            vec!["edge-1".to_string()],
            "a same-name pair must be dropped even when one half is tombstoned"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// A dead `existing_id` endpoint must never be written.
    ///
    /// `edge_purge` retains a **half-live** edge on purpose (module docs), so
    /// an edge minted against a dead endpoint whose partner is alive is never
    /// collected by any later cascade — it dangles for the life of the graph.
    /// The okf-backend-migration design states the contract directly: "an
    /// unresolved ref auto-rejects that item with a recorded reason — a
    /// dangling id is never written."
    ///
    /// The endpoint here is named differently from `ent-1` so the #189
    /// same-name guard cannot be what drops it: only the endpoint-liveness
    /// check can.
    #[test]
    fn edge_add_drops_tombstoned_curated_endpoint() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_entity(&conn, "ent-gone", "Retired Concept", "Summary", 100);
        conn.execute(
            "UPDATE curated_entities SET deleted_at = 1234 WHERE id = 'ent-gone'",
            [],
        )
        .unwrap();
        insert_edge_proposal(
            &conn,
            "prop-dead",
            "ent-1",
            "supersedes",
            "ent-gone",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-dead");

        assert_eq!(
            result.dropped_edges,
            vec!["edge-1".to_string()],
            "a tombstoned endpoint must be dropped AND reported"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// A soft-deleted `llm_wiki_entries` endpoint is dead for the same reason
    /// a tombstoned curated entity is — and this one has no `curated_entities`
    /// row at all, so the same-name guard provably cannot see it.
    #[test]
    fn edge_add_drops_soft_deleted_entry_endpoint() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1234 WHERE id = 'fact-dst'",
            [],
        )
        .unwrap();
        insert_edge_proposal(
            &conn,
            "prop-soft",
            "ent-1",
            "supersedes",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-soft");

        assert_eq!(result.dropped_edges, vec!["edge-1".to_string()]);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// An `existing_id` that names no row in any of the three endpoint tables
    /// is a hallucinated id. The `new_name` branch of `resolve_edge_ref`
    /// already refuses one; `existing_id` must too.
    #[test]
    fn edge_add_drops_unknown_existing_id_endpoint() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        insert_edge_proposal(
            &conn,
            "prop-ghost",
            "ent-1",
            "supersedes",
            "ent-nowhere",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-ghost");

        assert_eq!(result.dropped_edges, vec!["edge-1".to_string()]);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// `"self"` is not automatically live. A proposal raised while its entity
    /// was healthy, then resolved after the entity was soft-deleted, resolves
    /// `"self"` to a tombstone — and paired with the live `fact-dst` target
    /// that is a half-live edge `purge_dead_edges` would never collect.
    #[test]
    fn edge_add_drops_self_endpoint_of_soft_deleted_entity() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        insert_edge_proposal(
            &conn,
            "prop-dead-self",
            "ent-1",
            "supersedes",
            "fact-dst",
            doc_id,
            chunk_id,
        );
        // The entity dies between the proposal being raised and resolved. The
        // target stays live, so nothing else would refuse this edge.
        conn.execute(
            "UPDATE curated_entities SET deleted_at = 1234 WHERE id = 'ent-1'",
            [],
        )
        .unwrap();

        let result = accept_edge(&mut conn, "prop-dead-self");

        assert_eq!(
            result.dropped_edges,
            vec!["edge-1".to_string()],
            "a dead `self` endpoint must be dropped and reported"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "no half-live edge may be written");
    }

    /// The third endpoint home needs the tombstone case too, not just the
    /// live one. `edge_add_admits_live_task_endpoint` below keeps passing if
    /// the `llm_wiki_tasks` branch of `endpoint_is_live` is dropped entirely
    /// (a live task is still live via no branch at all only if the OR chain
    /// still names it) — but nothing would catch a refactor that narrows the
    /// chain to two tables and lets a *tombstoned* task mint the very edge
    /// #189/#191 set out to refuse.
    #[test]
    fn edge_add_drops_tombstoned_task_endpoint() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        conn.execute(
            "INSERT INTO llm_wiki_tasks (
                id, entity_id, description, status, priority,
                created_at, updated_at, resolved_at, deleted_at
             ) VALUES ('task-dead', 'ent-1', 'Ship it', 'pending', 0, 100, 100, NULL, 1234)",
            [],
        )
        .unwrap();
        insert_edge_proposal(
            &conn,
            "prop-task-dead",
            "ent-1",
            "blocks",
            "task-dead",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-task-dead");

        assert_eq!(result.dropped_edges, vec!["edge-1".to_string()]);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// The liveness check spans all three endpoint homes, so a live
    /// `llm_wiki_tasks` endpoint must still write. Guards the fix against
    /// over-restricting to `llm_wiki_entries` + `curated_entities`.
    #[test]
    fn edge_add_admits_live_task_endpoint() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        conn.execute(
            "INSERT INTO llm_wiki_tasks (
                id, entity_id, description, status, priority,
                created_at, updated_at, resolved_at, deleted_at
             ) VALUES ('task-live', 'ent-1', 'Ship it', 'pending', 0, 100, 100, NULL, NULL)",
            [],
        )
        .unwrap();
        insert_edge_proposal(
            &conn,
            "prop-task",
            "ent-1",
            "blocks",
            "task-live",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-task");

        assert!(
            result.dropped_edges.is_empty(),
            "a live task endpoint must not be dropped"
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE target_id = 'task-live'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    /// A true self-edge is the degenerate case of the same-name pair and is
    /// caught by the same comparison.
    #[test]
    fn self_referential_curated_edge_is_dropped() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supersedes", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-self",
            "ent-1",
            "supersedes",
            "ent-1",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-self");

        assert_eq!(result.dropped_edges, vec!["edge-1".to_string()]);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// Control: distinct curated entities with distinct names still write.
    #[test]
    fn distinct_name_curated_endpoints_still_write() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_entity(&conn, "ent-other", "Something Else", "Other summary", 100);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supersedes", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-ok",
            "ent-1",
            "supersedes",
            "ent-other",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-ok");

        assert!(result.dropped_edges.is_empty());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// The guard reads `curated_entities` only. An entry endpoint has no row
    /// there, so no comparison is made and the edge writes — the existing
    /// entity→fact edges every other test in this module relies on must not
    /// regress.
    #[test]
    fn entry_endpoints_are_not_name_guarded() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        // `fact-src` is an `llm_wiki_entries` row whose title is "Existing" —
        // the same name as curated entity `ent-1`, which is the edge source.
        insert_edge_proposal(
            &conn,
            "prop-entry",
            "ent-1",
            "supports",
            "fact-src",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-entry");

        assert!(
            result.dropped_edges.is_empty(),
            "a name collision across spaces must not trip the guard"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// Issue #191: `llm_wiki_edges.created_at` is milliseconds. The commit
    /// path wrote `now_secs`, so time-windowed tooling assuming ms silently
    /// missed every recent edge — the Sep 6 run's 11 edges read as "0 new".
    #[test]
    fn committed_edge_created_at_is_milliseconds() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn, "prop-ms", "ent-1", "supports", "fact-dst", doc_id, chunk_id,
        );

        accept_edge(&mut conn, "prop-ms");

        let created_at: i64 = conn
            .query_row("SELECT created_at FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert!(
            created_at >= crate::db::schema::SEC_VS_MS_THRESHOLD,
            "edge created_at must be epoch ms, got {created_at}"
        );
    }

    /// Non-strict modes admit any edge type: `emergent` grows its vocabulary
    /// from the corpus, and `off` has none to enforce. The fixture seeds
    /// both `ent-1` AND `tier_fact` with the same non-strict mode so the
    /// gate is actually exercised through every code path the resolver
    /// would reach — without a `tier_fact` row the gate passes only because
    /// the partition fallback is empty, not because the modes are honored.
    #[test]
    fn non_strict_modes_do_not_gate_edge_types() {
        for mode in ["emergent", "off"] {
            let mut conn = open_in_memory().unwrap();
            let (doc_id, chunk_id) = seed_linkable_entity(&conn);
            // Both lookups in `resolve_strict_edge_vocabulary` see the same
            // non-strict mode, so the test really pins mode behavior.
            seed_manifest(
                &conn,
                "ent-1",
                mode,
                &["thing"],
                &[("supports", "thing", "thing")],
            );
            seed_manifest(
                &conn,
                "tier_fact",
                mode,
                &["thing"],
                &[("supports", "thing", "thing")],
            );
            insert_edge_proposal(
                &conn,
                "prop-loose",
                "ent-1",
                "anything_goes",
                "fact-dst",
                doc_id,
                chunk_id,
            );

            let result = accept_edge(&mut conn, "prop-loose");
            assert!(
                result.dropped_edges.is_empty(),
                "mode {mode} must not gate edge types"
            );
        }
    }

    /// A brain with no manifest at all is ungated — the overwhelmingly common
    /// case, and the one PR #78's graceful-degradation contract covers.
    #[test]
    fn a_brain_with_no_manifest_is_not_gated() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        insert_edge_proposal(
            &conn,
            "prop-none",
            "ent-1",
            "whatever",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-none");
        assert!(result.dropped_edges.is_empty());
    }

    /// Strict mode with an empty edge vocabulary does not gate. A gate needs a
    /// vocabulary to be a gate, and this state is far more likely a
    /// half-finished seed than a deliberate "no edges permitted" policy —
    /// dropping every edge of every proposal would be severe and hard to
    /// diagnose. Deliberately the most permissive reading of an ambiguous
    /// state; pinned so flipping it is a conscious decision.
    #[test]
    fn strict_mode_with_no_declared_edge_types_does_not_gate() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(&conn, "ent-1", "strict", &["thing"], &[]);
        insert_edge_proposal(
            &conn,
            "prop-empty",
            "ent-1",
            "anything",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-empty");
        assert!(result.dropped_edges.is_empty());
    }

    /// Writes stay non-retroactive (the gate never rewrites history): a row
    /// written before the manifest existed stays in `llm_wiki_edges`. Reads,
    /// on the other hand, now intersect against the manifest vocabulary
    /// (Gap B fix, spec §1.1 / PR 1 of #158): traversal must not surface the
    /// off-manifest row as a first-class neighbour, even though the row
    /// itself is grandfathered at the storage layer.
    #[test]
    fn strict_mode_grandfathers_edges_written_before_the_manifest() {
        let conn = open_in_memory().unwrap();
        seed_linkable_entity(&conn);
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_old', 'ent-1', 'fact-src', 'fact-dst', 'legacy_type', 1757000000000)",
            [],
        )
        .unwrap();
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );

        // Storage is grandfathered: the row is still in the table.
        let stored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE id = 'edge_old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, 1,
            "a row written before the manifest stays in the table"
        );

        // Reads are filtered: the off-manifest edge is not surfaced as a
        // first-class neighbour.
        let walked = crate::wiki_graph::wiki_traverse_graph(
            &conn,
            Some("ent-1"),
            "fact-src",
            2,
            crate::wiki_graph::TraverseDirection::Both,
            &[],
        )
        .unwrap();
        assert!(
            walked.edges.is_empty(),
            "an off-manifest edge must not surface in traversal, got: {walked:?}"
        );
    }

    /// Regression: the curated-id lookup misses on production data because
    /// manifests are seeded against partitions (`tier_fact`, …), not curated
    /// entity ids. The resolver must therefore fall back to the partition and
    /// still apply the strict gate. With the bug present the first `Ok(_)`
    /// returned `None` and the gate never ran — every off-manifest edge on a
    /// real proposal slipped through.
    ///
    /// Two assertions, in two fresh connections: the gate must (a) drop an
    /// off-manifest edge type and (b) admit a manifest-defined one, both when
    /// the manifest is seeded at `tier_fact` (not at the curated id).
    #[test]
    fn strict_mode_falls_back_to_tier_fact_and_drops_off_manifest_edge() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        // Seed the strict manifest at the partition, NOT at the curated id.
        seed_manifest(
            &conn,
            "tier_fact",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        // entity_id is "ent-1" (curated, no manifest seeded here) — the
        // first lookup misses and exercises the partition fallback.
        insert_edge_proposal(
            &conn,
            "prop-curated-bad",
            "ent-1",
            "invented_by_the_llm",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-curated-bad");

        assert_eq!(
            result.dropped_edges,
            vec!["edge-1".to_string()],
            "a curated-id proposal whose partition manifest is strict must still drop \
             an off-manifest edge type; the partition fallback must fire when the \
             curated-id lookup misses"
        );
    }

    #[test]
    fn strict_mode_falls_back_to_tier_fact_and_admits_manifest_defined_edge() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "tier_fact",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        insert_edge_proposal(
            &conn,
            "prop-curated-ok",
            "ent-1",
            "supports",
            "fact-dst",
            doc_id,
            chunk_id,
        );

        let result = accept_edge(&mut conn, "prop-curated-ok");

        assert!(
            result.dropped_edges.is_empty(),
            "a declared edge type must not be dropped via the partition fallback"
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE edge_type = 'supports' \
                 AND entity_id = 'ent-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "a partition-gated edge must land in llm_wiki_edges"
        );
    }

    /// `curated_relationships` is the AST symbol-linker graph, written
    /// mechanically from code with structural rel_types. It is explicitly out
    /// of scope for the manifest gate (§2.3) — gating it would break code
    /// indexing on every strict brain.
    #[test]
    fn curated_relationships_writes_are_unaffected_by_strict_mode() {
        let conn = open_in_memory().unwrap();
        let (doc_id, _) = seed_linkable_entity(&conn);
        seed_manifest(
            &conn,
            "ent-1",
            "strict",
            &["thing"],
            &[("supports", "thing", "thing")],
        );
        let a = seed_chunk(&conn, doc_id);
        let b = seed_chunk(&conn, doc_id);

        conn.execute(
            "INSERT INTO curated_relationships (from_id, to_id, rel_type, symbol, entity_id)
             VALUES (?1, ?2, 'CALLS', 'my_fn', 'ent-1')",
            params![a, b],
        )
        .expect("the AST linker graph is not gated by the ontology manifest");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM curated_relationships WHERE rel_type = 'CALLS'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn edge_new_name_unresolved_is_dropped() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-edge",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "edge-1".into(),
                item_type: "edge_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "source": "self",
                    "target": { "new_name": "Missing Entity" },
                    "edge_type": "related_to"
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-edge",
            &[ItemDecision {
                item_id: "edge-1".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.dropped_edges, vec!["edge-1".to_string()]);
        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 0);
    }

    #[test]
    fn edge_insert_or_ignore_dedupes() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_entity(&conn, "ent-2", "Other", "Summary", 100);

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge-existing', 'ent-1', 'ent-1', 'ent-2', 'related_to', 1757000000000)",
            [],
        )
        .unwrap();

        insert_test_proposal(
            &conn,
            "prop-dedupe",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "edge-dup".into(),
                item_type: "edge_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "source": { "existing_id": "ent-1" },
                    "target": { "existing_id": "ent-2" },
                    "edge_type": "related_to"
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-dedupe",
            &[ItemDecision {
                item_id: "edge-dup".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(result.committed.is_empty());
        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 1);
    }

    #[test]
    fn failed_commit_leaves_no_outbox_rows() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-fail",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "bad-update".into(),
                item_type: "fact_update".into(),
                target_id: Some("missing-fact".into()),
                payload: serde_json::json!({ "body": "nope", "tags": [], "confidence": "inferred" }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let err = resolve_proposal(
            &mut conn,
            "prop-fail",
            &[ItemDecision {
                item_id: "bad-update".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found"));

        let outbox_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outbox_count, 0);

        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposals WHERE id = 'prop-fail'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn resolution_events_use_approved_rejected_types() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/notes.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);

        // Approve a proposal (NewEntity)
        insert_test_proposal(
            &conn,
            "prop-approve",
            ProposalKind::NewEntity,
            None,
            vec![fact_item("item-approve", chunk_id, "Approved fact.")],
            doc_id,
        );
        resolve_proposal(
            &mut conn,
            "prop-approve",
            &[ItemDecision {
                item_id: "item-approve".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        // Reject a proposal (UpdateEntity with existing entity so the resolve path reaches write_resolution_event)
        seed_entity(&conn, "ent-1", "Project X", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-reject",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("item-reject", chunk_id, "Rejected fact.")],
            doc_id,
        );
        resolve_proposal(
            &mut conn,
            "prop-reject",
            &[ItemDecision {
                item_id: "item-reject".into(),
                decision: ItemDecisionKind::Reject,
                edited_payload: None,
            }],
            Some("not relevant"),
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        let approved_type: String = conn
            .query_row(
                "SELECT event_type FROM llm_wiki_events WHERE summary LIKE 'Approved%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(approved_type, "approved");

        let rejected_type: String = conn
            .query_row(
                "SELECT event_type FROM llm_wiki_events WHERE event_type = 'rejected'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rejected_type, "rejected");
    }

    #[test]
    fn resolve_proposal_writes_content_hash_in_evidence_row() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/note.pdf");
        // Pre-seed a chunk with a real content_hash; the commit must
        // look this up and write it into the source_ref JSON.
        let chunk_id = seed_chunk(&conn, doc_id);
        let hash =
            crate::db::chunk_hash::compute_chunk_hash("quoted", "/vault/documents/note.pdf", 0);
        conn.execute(
            "UPDATE chunks SET content_hash = ?1 WHERE id = ?2",
            params![hash, chunk_id],
        )
        .unwrap();

        insert_test_proposal(
            &conn,
            "prop-hash",
            ProposalKind::NewEntity,
            None,
            vec![NewProposalItem {
                id: "item-h".into(),
                item_type: "fact_add".into(),
                target_id: None,
                payload: serde_json::json!({ "body": "Hashed fact.", "tags": [], "confidence": "inferred" }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(), // commit must look up the real hash
                    quote: "quoted".into(),
                    start_line: Some(1),
                    end_line: Some(2),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        resolve_proposal(
            &mut conn,
            "prop-hash",
            &[ItemDecision {
                item_id: "item-h".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        // Since #186, `source_ref` carries only the librarian token; the
        // evidence JSON (with the resolved content_hash) lives in
        // `librarian_evidence`, written in the same transaction.
        let entry_id: String = conn
            .query_row(
                "SELECT id FROM llm_wiki_entries ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let source_ref: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = ?1",
                [&entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source_ref, librarian_source_ref_token(&entry_id));

        let stored = evidence_json_for_entry(&conn, &entry_id)
            .unwrap()
            .expect("evidence row must exist");
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        let evidence = parsed.get("evidence").unwrap().as_array().unwrap();
        let entry = evidence[0].as_object().unwrap();
        assert_eq!(
            entry.get("content_hash").and_then(|v| v.as_str()).unwrap(),
            hash,
            "commit must populate content_hash from the chunk row"
        );
    }

    fn resolve_fact(conn: &mut Connection, prop_id: &str, item_id: &str) -> CommitResult {
        resolve_proposal(
            conn,
            prop_id,
            &[ItemDecision {
                item_id: item_id.into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn fact_add_identical_body_dedupes() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-1", chunk_id, "Rust is a systems language.")],
            doc_id,
        );

        // Separate trigger doc so the second proposal is not auto-superseded.
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-dup", chunk2, "Rust is a systems language.")],
            doc2,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-dup");

        assert!(result.committed.is_empty(), "duplicate must not commit");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "exactly one stored fact");
        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposal_items WHERE id = 'fact-dup'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "rejected", "duplicate item recorded as skipped");
    }

    #[test]
    fn fact_add_whitespace_varied_duplicate_dedupes() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-1",
                chunk_id,
                "Rust is  a  systems\tlanguage.",
            )],
            doc_id,
        );

        // Separate trigger doc so the second proposal is not auto-superseded.
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-ws",
                chunk2,
                "  Rust   is a systems\nlanguage.  ",
            )],
            doc2,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-ws");

        assert!(result.committed.is_empty());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// R2.7.5: a merge moves no facts, so a pre-merge fact stays keyed to
    /// the loser. Re-adding the same body on the SURVIVOR must dedupe
    /// against the whole redirect cluster, not mint a duplicate.
    #[test]
    fn fact_add_dedupes_against_merged_loser_facts() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-surv", "Rust", "Summary", 100);
        seed_entity(&conn, "ent-lose", "Rust", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-lose"),
            vec![fact_item("fact-1", chunk_id, "Rust is a systems language.")],
            doc_id,
        );
        resolve_fact(&mut conn, "prop-f1", "fact-1");
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent-lose', 'ent-surv', 1)",
            [],
        )
        .unwrap();

        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-surv"),
            vec![fact_item("fact-2", chunk2, "Rust is a  systems language.")],
            doc2,
        );
        let result = resolve_fact(&mut conn, "prop-f2", "fact-2");
        assert!(result.committed.is_empty(), "{:?}", result.committed);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn fact_add_different_body_still_commits() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-1", chunk_id, "Rust is a systems language.")],
            doc_id,
        );

        // Separate trigger doc so the second proposal is not auto-superseded.
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-new",
                chunk2,
                "Rust has no garbage collector.",
            )],
            doc2,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-new");

        assert_eq!(result.committed.len(), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn fact_add_dedupe_scoped_per_entity() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_entity(&conn, "ent-2", "Other", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-1", chunk_id, "Rust is a systems language.")],
            doc_id,
        );
        // Same normalized body, different entity — must NOT be treated as duplicate.
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-2"),
            vec![fact_item("fact-x", chunk_id, "Rust is a systems language.")],
            doc_id,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-x");

        assert_eq!(result.committed.len(), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn fact_add_all_duplicates_includes_count_in_rejected_event() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        // Seed an existing fact so the second proposal's items dedupe.
        insert_test_proposal(
            &conn,
            "prop-original",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-original",
                chunk_id,
                "Rust is a systems language.",
            )],
            doc_id,
        );
        resolve_fact(&mut conn, "prop-original", "fact-original");

        // New proposal with ONLY duplicate fact_add items (different trigger doc
        // so it isn't auto-superseded).
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-dup",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![
                fact_item("fact-d1", chunk2, "Rust is a systems language."),
                fact_item("fact-d2", chunk2, "Rust is a systems language."),
            ],
            doc2,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-dup",
            &[
                ItemDecision {
                    item_id: "fact-d1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                },
                ItemDecision {
                    item_id: "fact-d2".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                },
            ],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.proposal_status, "rejected");
        assert_eq!(result.committed.len(), 0);

        // The resolution event for prop-dup must surface the duplicate count
        // so reviewers can see *why* every item was rejected. Filter by
        // event_type rather than relying on `ORDER BY created_at DESC LIMIT 1`,
        // because back-to-back resolve_proposal calls can produce identical
        // millisecond timestamps and SQLite's order is then unspecified —
        // returning whichever row the storage layer chose first.
        let (event_type, event_summary): (String, String) = conn
            .query_row(
                "SELECT event_type, summary FROM llm_wiki_events
                 WHERE event_type = 'rejected'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_type, "rejected");
        assert!(
            event_summary.contains("2 duplicate fact(s) skipped"),
            "rejected event summary must include duplicate count, got: {event_summary}"
        );
    }

    #[test]
    fn fact_add_stores_an_embedding_when_a_profile_is_configured() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let mut conn = open_in_memory().unwrap();
            let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
            let chunk_id = seed_chunk(&conn, doc_id);
            seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

            insert_test_proposal(
                &conn,
                "prop-embed",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![NewProposalItem {
                    id: "fact-1".into(),
                    item_type: "fact_add".into(),
                    target_id: None,
                    payload: serde_json::json!({ "body": "A fact worth embedding." }),
                    evidence: vec![StoredEvidenceChunk {
                        chunk_id: Some(chunk_id),
                        content_hash: String::new(),
                        quote: "x".into(),
                        start_line: Some(1),
                        end_line: Some(1),
                        source_kind: None,
                    }],
                }],
                doc_id,
            );

            resolve_proposal(
                &mut conn,
                "prop-embed",
                &[ItemDecision {
                    item_id: "fact-1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                }],
                None,
                ResolveOptions {
                    auto_approve: false,
                    embed_profile: Some(EmbedProfile::default()),
                    ..Default::default()
                },
            )
            .unwrap();

            let blob_len: Option<i64> = conn
                .query_row(
                    "SELECT length(embedding_blob) FROM llm_wiki_entries WHERE entity_id = 'ent-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(blob_len, Some(32), "constant8 gives 8 dims -> 32 bytes");
        });
    }

    #[test]
    fn fact_add_commits_with_null_embedding_when_the_provider_fails() {
        temp_env::with_vars([("CURATED_EMBED_STUB", None::<&str>)], || {
            let mut conn = open_in_memory().unwrap();
            let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
            let chunk_id = seed_chunk(&conn, doc_id);
            seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

            insert_test_proposal(
                &conn,
                "prop-embed-fail",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![NewProposalItem {
                    id: "fact-1".into(),
                    item_type: "fact_add".into(),
                    target_id: None,
                    payload: serde_json::json!({ "body": "Curation that must survive." }),
                    evidence: vec![StoredEvidenceChunk {
                        chunk_id: Some(chunk_id),
                        content_hash: String::new(),
                        quote: "x".into(),
                        start_line: Some(1),
                        end_line: Some(1),
                        source_kind: None,
                    }],
                }],
                doc_id,
            );

            // Cloud profiles always Err -> the embed pre-pass fails.
            let result = resolve_proposal(
                &mut conn,
                "prop-embed-fail",
                &[ItemDecision {
                    item_id: "fact-1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                }],
                None,
                ResolveOptions {
                    auto_approve: false,
                    embed_profile: Some(EmbedProfile::Cloud {
                        provider: crate::embedder::CloudProvider::OpenAi,
                        model: "unreachable".into(),
                        api_key: String::new(),
                    }),
                    ..Default::default()
                },
            );

            assert!(
                result.is_ok(),
                "an embed failure must never destroy the librarian's curation"
            );
            let (count, blob): (i64, Option<Vec<u8>>) = conn
                .query_row(
                    "SELECT COUNT(*), MAX(embedding_blob) FROM llm_wiki_entries
                      WHERE entity_id = 'ent-1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(count, 1, "the fact is committed");
            assert_eq!(blob, None, "with a NULL blob for the sweep to fill later");
        });
    }

    #[test]
    fn fact_add_commits_with_null_embedding_when_provider_returns_wrong_size() {
        // R6 length-mismatch guard: `constant8_short` returns N-1 vectors for
        // an N-text batch. The pre-pass must skip that chunk instead of
        // zipping the surviving vector onto the first id — the fact still
        // commits, but with a NULL blob that the sweep will fill later.
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8_short"))], || {
            let mut conn = open_in_memory().unwrap();
            let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
            let chunk_id = seed_chunk(&conn, doc_id);
            seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

            insert_test_proposal(
                &conn,
                "prop-embed-short",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![NewProposalItem {
                    id: "fact-1".into(),
                    item_type: "fact_add".into(),
                    target_id: None,
                    payload: serde_json::json!({ "body": "Body that must commit cleanly." }),
                    evidence: vec![StoredEvidenceChunk {
                        chunk_id: Some(chunk_id),
                        content_hash: String::new(),
                        quote: "x".into(),
                        start_line: Some(1),
                        end_line: Some(1),
                        source_kind: None,
                    }],
                }],
                doc_id,
            );

            let result = resolve_proposal(
                &mut conn,
                "prop-embed-short",
                &[ItemDecision {
                    item_id: "fact-1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                }],
                None,
                ResolveOptions {
                    auto_approve: false,
                    embed_profile: Some(EmbedProfile::default()),
                    entry_embeddings: None,
                    ..Default::default()
                },
            );

            assert!(
                result.is_ok(),
                "a wrong-sized provider response must never destroy the librarian's curation"
            );
            let (count, blob): (i64, Option<Vec<u8>>) = conn
                .query_row(
                    "SELECT COUNT(*), MAX(embedding_blob) FROM llm_wiki_entries
                          WHERE entity_id = 'ent-1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(count, 1, "the fact is committed");
            assert_eq!(
                blob, None,
                "the wrong-sized batch must not pair a vector to this row"
            );
        });
    }

    // ── Issue #211: stranded proposals ─────────────────────────────────────

    /// Spec test 9.
    #[test]
    fn trigger_source_label_names_a_deleted_trigger() {
        let mut conn = open_in_memory().unwrap();
        seed_pending_proposal(&conn, "prop-label");
        crate::db::proposals::test_support::delete_path(&mut conn, "/vault/documents/hvg.pdf");

        assert_eq!(
            trigger_source_label(&conn, "prop-label").unwrap(),
            "hvg.pdf"
        );
    }

    /// Spec test 8: reject on a stranded proposal.
    #[test]
    fn stranded_proposal_reject_resolves_rejected() {
        let mut conn = open_in_memory().unwrap();
        seed_pending_proposal(&conn, "prop-strand-reject");
        crate::db::proposals::test_support::delete_path(&mut conn, "/vault/documents/hvg.pdf");

        let decisions: Vec<ItemDecision> = all_accept_decisions(&conn, "prop-strand-reject")
            .into_iter()
            .map(|d| ItemDecision {
                decision: ItemDecisionKind::Reject,
                ..d
            })
            .collect();
        let result = resolve_proposal(
            &mut conn,
            "prop-strand-reject",
            &decisions,
            Some("stale"),
            ResolveOptions::default(),
        )
        .unwrap();
        assert_eq!(result.proposal_status, "rejected");
    }

    /// Spec test 8: approve on a stranded proposal skips the unanchored fact
    /// and resolves `rejected` (Phase-2 gate + finalize_proposal_status).
    #[test]
    fn stranded_proposal_approve_skips_facts_and_resolves_rejected() {
        let mut conn = open_in_memory().unwrap();
        seed_pending_proposal(&conn, "prop-strand-approve");
        crate::db::proposals::test_support::delete_path(&mut conn, "/vault/documents/hvg.pdf");

        let decisions = all_accept_decisions(&conn, "prop-strand-approve");
        let result = resolve_proposal(
            &mut conn,
            "prop-strand-approve",
            &decisions,
            None,
            ResolveOptions::default(),
        )
        .unwrap();

        assert_eq!(result.skipped_unanchored, 1);
        assert_eq!(result.proposal_status, "rejected");
        let entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entries, 0, "nothing evidenced can be committed");
    }

    /// Seed a pending proposal whose single fact carries the real
    /// `content_hash` of a chunk at `path`, then delete `path`.
    fn seed_hashed_stranded_proposal(conn: &mut Connection, id: &str, path: &str, text: &str) {
        let doc_id = seed_document(conn, path);
        let hash = crate::db::chunk_hash::compute_chunk_hash(text, path, 0);
        let chunk = Chunk {
            text: text.into(),
            start_line: 1,
            end_line: 1,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        let chunk_id = insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", &hash).unwrap();
        let mut item = fact_item(&format!("item-{id}"), chunk_id, "A re-anchorable fact.");
        item.evidence[0].content_hash = hash;
        insert_test_proposal(conn, id, ProposalKind::NewEntity, None, vec![item], doc_id);
        crate::db::proposals::test_support::delete_path(conn, path);
    }

    /// Insert a chunk for `text` at `path` with its real content hash.
    fn ingest_text_at(conn: &Connection, path: &str, text: &str) {
        let doc_id = seed_document(conn, path);
        let chunk = Chunk {
            text: text.into(),
            start_line: 1,
            end_line: 1,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        let hash = crate::db::chunk_hash::compute_chunk_hash(text, path, 0);
        insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", &hash).unwrap();
    }

    /// Spec test 8: bytes restored at the ORIGINAL path re-anchor the evidence.
    #[test]
    fn stranded_evidence_reanchors_when_restored_at_the_original_path() {
        let mut conn = open_in_memory().unwrap();
        let path = "/vault/documents/anchor.md";
        seed_hashed_stranded_proposal(&mut conn, "prop-restore", path, "anchored text");

        ingest_text_at(&conn, path, "anchored text");

        let decisions = all_accept_decisions(&conn, "prop-restore");
        let result = resolve_proposal(
            &mut conn,
            "prop-restore",
            &decisions,
            None,
            ResolveOptions::default(),
        )
        .unwrap();
        assert_eq!(result.skipped_unanchored, 0);
        assert_eq!(result.proposal_status, "approved");
    }

    /// Spec test 8: the same text at a DIFFERENT path hashes differently
    /// (`content_hash` includes the doc path), so a move never re-anchors.
    #[test]
    fn stranded_evidence_does_not_reanchor_after_a_move() {
        let mut conn = open_in_memory().unwrap();
        seed_hashed_stranded_proposal(
            &mut conn,
            "prop-moved",
            "/vault/documents/anchor.md",
            "anchored text",
        );

        ingest_text_at(&conn, "/vault/documents/moved/anchor.md", "anchored text");

        let decisions = all_accept_decisions(&conn, "prop-moved");
        let result = resolve_proposal(
            &mut conn,
            "prop-moved",
            &decisions,
            None,
            ResolveOptions::default(),
        )
        .unwrap();
        assert_eq!(result.skipped_unanchored, 1);
        assert_eq!(result.proposal_status, "rejected");
    }

    /// A `GateResolutionContext` over a locally owned `IngestConfig` — the
    /// same injection shape the heal-ontology tests use, so the per-endpoint
    /// edge ladder can be exercised without touching the filesystem policy
    /// cache.
    fn edge_gate_ctx<'a>(
        ingest: &'a crate::config::IngestConfig,
        degraded: &'a crate::config::OntologyDegradedState,
    ) -> crate::db::entity_gate::GateResolutionContext<'a> {
        crate::db::entity_gate::GateResolutionContext {
            ingest,
            degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        }
    }

    fn edge_test_ctx(entity_id: &str) -> CommitContext {
        let mut ctx = test_ctx(entity_id);
        ctx.proposal_id = "prop_edge".into();
        ctx
    }

    /// Review finding (fact/task endpoints): edge endpoints are frequently
    /// `llm_wiki_entries` ids, not entity ids — the endpoint ladder must
    /// resolve them to their OWNING entity before rung 1b / rungs 2-3.
    /// Pre-fix, both rungs saw the raw fact id (no manifest row, no source
    /// paths), fell to rung 4, and an off-manifest edge was written
    /// verbatim even though the owning entity was strict.
    #[test]
    fn edge_fact_endpoints_inherit_owning_entitys_strict_gate() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_a", "A", "summary", 100);
        // The OWNER carries the strict manifest; `tier_fact` stays unmarked
        // so rung 4 alone would NOT gate — exactly the pre-fix hole.
        seed_manifest(
            &conn,
            "ent_a",
            "strict",
            &["thing"],
            &[("depends_on", "thing", "thing")],
        );
        seed_fact_row(&conn, "fact-src", "ent_a", "source fact");
        seed_fact_row(&conn, "fact-dst", "ent_a", "target fact");

        let ingest = crate::config::IngestConfig::default();
        let degraded = crate::config::OntologyDegradedState::default();
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_a");

        let outcome = resolve_edge_endpoint_vocabulary(
            &conn, "ent_a", "fact-src", "fact-dst", &mut ctx, &gate,
        )
        .unwrap();
        let vocab = outcome.expect("fact endpoints inherit the owner's strict ladder");
        assert!(
            vocab.canonicalize("depends_on").is_some(),
            "the gate runs under the owner entity's own manifest vocabulary"
        );
        assert!(
            vocab.canonicalize("invented_type").is_none(),
            "an off-manifest type must fail the owner's gate"
        );
    }

    /// Rung 1a for fact/task endpoints: a deliberate opt-out on the OWNING
    /// entity disarms the edge gate, exactly as it would for the entity id
    /// itself (the opt-out lives on the entity, never on a fact row).
    #[test]
    fn edge_fact_endpoints_honor_owning_entitys_optout() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_a", "A", "summary", 100);
        seed_manifest(
            &conn,
            "ent_a",
            "strict",
            &["thing"],
            &[("depends_on", "thing", "thing")],
        );
        seed_fact_row(&conn, "fact-src", "ent_a", "source fact");
        seed_fact_row(&conn, "fact-dst", "ent_a", "target fact");
        conn.execute(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at) VALUES ('ent_a', 'user', 1)",
            [],
        )
        .unwrap();

        let ingest = crate::config::IngestConfig::default();
        let degraded = crate::config::OntologyDegradedState::default();
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_a");
        let outcome = resolve_edge_endpoint_vocabulary(
            &conn, "ent_a", "fact-src", "fact-dst", &mut ctx, &gate,
        )
        .unwrap();
        assert!(
            outcome.is_none(),
            "the owner's opt-out must disarm the edge gate"
        );
    }

    /// The edge row is anchored to the PROPOSAL entity, and the read filter
    /// and the off-manifest purge judge it by that entity's strict
    /// vocabulary. A type admitted only by an ENDPOINT's vocabulary must be
    /// dropped at write time, or it is hidden on read and destroyed by the
    /// next sweep; a type both vocabularies declare is written.
    #[test]
    fn edge_admitted_by_endpoint_vocab_must_also_fit_the_anchor() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_a", "A", "summary", 100);
        seed_entity(&conn, "ent_b", "B", "summary", 100);
        seed_manifest(
            &conn,
            "ent_a",
            "strict",
            &["thing"],
            &[("related", "thing", "thing")],
        );
        seed_manifest(
            &conn,
            "ent_b",
            "strict",
            &["thing"],
            &[
                ("related", "thing", "thing"),
                ("mentions", "thing", "thing"),
            ],
        );
        seed_fact_row(&conn, "fact-b", "ent_b", "b fact");
        seed_fact_row(&conn, "fact-a", "ent_a", "a fact");

        let ingest = crate::config::IngestConfig::default();
        let degraded = crate::config::OntologyDegradedState::default();
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_a");
        let item = |id: &str| LoadedItem {
            id: id.into(),
            item_type: "edge_add".into(),
            target_id: None,
            payload: serde_json::Value::Null,
            evidence: Vec::new(),
            edited_payload: None,
        };
        let payload = |edge_type: &str| {
            serde_json::json!({
                "edge_type": edge_type,
                "source": {"existing_id": "fact-b"},
                "target": {"existing_id": "fact-a"},
            })
        };

        commit_edge_add(&conn, &gate, &mut ctx, &item("e1"), &payload("mentions")).unwrap();
        assert_eq!(ctx.dropped_edges, vec!["e1".to_string()]);

        commit_edge_add(&conn, &gate, &mut ctx, &item("e2"), &payload("related")).unwrap();
        let written: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE entity_id = 'ent_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(written, 1, "a type both vocabularies declare is written");
        assert_eq!(
            crate::db::edge_purge::purge_off_manifest_edges_all(&conn).unwrap(),
            0,
            "nothing the write gate admitted is purged"
        );
    }

    /// Review finding: a `new_name` endpoint resolves deterministically —
    /// exact case first, then case-insensitive, ties on the byte-wise lowest
    /// id (the survivor the merge pass would pick, R2.7.3).
    #[test]
    fn new_name_edge_ref_is_deterministic_and_case_insensitive() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_b", "Adrian", "s", 100);
        seed_entity(&conn, "ent_a", "Adrian", "s", 100);
        seed_entity(&conn, "ent_0", "ADRIAN", "s", 100);
        let by = |name: &str| {
            resolve_edge_ref(&conn, &serde_json::json!({ "new_name": name }), "ent_x").unwrap()
        };
        assert_eq!(
            by("Adrian").as_deref(),
            Some("ent_a"),
            "exact case wins, lowest id"
        );
        assert_eq!(by("ADRIAN").as_deref(), Some("ent_0"), "exact case wins");
        assert_eq!(
            by("adrian").as_deref(),
            Some("ent_0"),
            "no exact match → case-insensitive, lowest id"
        );
        assert_eq!(by("Nobody"), None);
    }

    /// §6 item 1b: R2.3.0 strict-wins — edge endpoint opt-out cascade
    /// short-circuits the edge gate. A `ct_entity_optouts` row on
    /// EITHER endpoint disarms the gate, so a strict manifest row on the
    /// OTHER endpoint (or the proposal entity) is bypassed and the edge is
    /// written verbatim.
    ///
    /// Matrix cases (per endpoint pair):
    ///   * no opt-out + strict endpoint manifest → vocabulary check fires
    ///   * opt-out on either endpoint → write verbatim (cascade
    ///     short-circuit per §2.1, r12-m1)
    #[test]
    fn edge_endpoint_opt_out_short_circuits_gate() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_a", "A", "summary", 100);
        seed_entity(&conn, "ent_b", "B", "summary", 100);
        // The TARGET endpoint carries its own strict manifest — under
        // R2.3.0 that alone gates the edge (source not strict).
        seed_manifest(
            &conn,
            "ent_b",
            "strict",
            &["thing"],
            &[("depends_on", "thing", "thing")],
        );

        let ingest = crate::config::IngestConfig::default();
        let degraded = crate::config::OntologyDegradedState::default();
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_a");

        // Case 1: no opt-out on either endpoint → the target's strict
        // manifest vocabulary fires.
        let outcome =
            resolve_edge_endpoint_vocabulary(&conn, "ent_a", "ent_a", "ent_b", &mut ctx, &gate)
                .unwrap();
        assert!(
            outcome.is_some(),
            "no opt-out + strict target endpoint → vocabulary fires"
        );

        // Case 2: opt-out on source endpoint → no vocabulary, write verbatim.
        // Each case is a NEW proposal (fresh context): the opt-out memo is
        // per-proposal, and opt-outs cannot change inside one proposal's
        // IMMEDIATE commit transaction.
        conn.execute(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at) VALUES ('ent_a', 'user', 1)",
            [],
        )
        .unwrap();
        let mut ctx = edge_test_ctx("ent_a");
        let outcome =
            resolve_edge_endpoint_vocabulary(&conn, "ent_a", "ent_a", "ent_b", &mut ctx, &gate)
                .unwrap();
        assert!(
            outcome.is_none(),
            "opt-out on source endpoint must disarm the edge gate"
        );

        // Case 3: opt-out on target endpoint → no vocabulary, write verbatim,
        // even though the target is the strict side.
        conn.execute(
            "DELETE FROM ct_entity_optouts WHERE entity_id = 'ent_a'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at) VALUES ('ent_b', 'user', 1)",
            [],
        )
        .unwrap();
        let mut ctx = edge_test_ctx("ent_a");
        let outcome =
            resolve_edge_endpoint_vocabulary(&conn, "ent_a", "ent_a", "ent_b", &mut ctx, &gate)
                .unwrap();
        assert!(
            outcome.is_none(),
            "opt-out on target endpoint must disarm the edge gate"
        )
    }

    /// §6 item 1b (R2.3.0, final wave): endpoints resolving to DIFFERENT
    /// modes gate PER DIRECTION — a strict endpoint gates the edge even
    /// when the PROPOSING entity is not strict, in either direction. The
    /// both-strict and neither-strict rows ride along.
    #[test]
    fn edge_endpoints_in_different_modes_gate_per_direction() {
        let ingest = crate::config::IngestConfig::default();
        let degraded = crate::config::OntologyDegradedState::default();
        let gate = edge_gate_ctx(&ingest, &degraded);

        // ── Row 1/2: different modes, both directions. `ent_strict`
        // carries its own strict manifest row (rung 1b); `ent_off` is
        // explicitly off; the PROPOSER `ent_plain` has no row at all.
        {
            let conn = open_in_memory().unwrap();
            seed_entity(&conn, "ent_plain", "Plain", "summary", 100);
            seed_entity(&conn, "ent_strict", "Strict", "summary", 100);
            seed_entity(&conn, "ent_off", "Off", "summary", 100);
            seed_manifest(
                &conn,
                "ent_strict",
                "strict",
                &["thing"],
                &[("depends_on", "thing", "thing")],
            );
            seed_manifest(&conn, "ent_off", "off", &[], &[]);
            let mut ctx = edge_test_ctx("ent_plain");

            // strict → off: the strict SOURCE endpoint gates.
            let outcome = resolve_edge_endpoint_vocabulary(
                &conn,
                "ent_plain",
                "ent_strict",
                "ent_off",
                &mut ctx,
                &gate,
            )
            .unwrap();
            assert!(
                outcome.is_some(),
                "strict source + off target must gate (strict-wins across endpoints)"
            );
            // off → strict: the strict TARGET endpoint still gates — an
            // off directory never downgrades an edge the strict side makes
            // checkable (R2.3.0, same asymmetry as §2.3.3).
            let outcome = resolve_edge_endpoint_vocabulary(
                &conn,
                "ent_plain",
                "ent_off",
                "ent_strict",
                &mut ctx,
                &gate,
            )
            .unwrap();
            assert!(
                outcome.is_some(),
                "off source + strict target must gate per direction (strict-wins)"
            );
            // The gated vocabulary is the STRICT side's own manifest.
            let vocab = outcome.expect("checked is_some above");
            assert!(
                vocab.canonicalize("depends_on").is_some(),
                "the gate runs under the strict endpoint's own manifest vocabulary"
            );
            assert!(
                vocab.canonicalize("not_in_manifest").is_none(),
                "an off-manifest type must fail the strict endpoint's gate"
            );
        }

        // ── Row 3: both endpoints strict → gated (source's vocabulary
        // fires first).
        {
            let conn = open_in_memory().unwrap();
            seed_entity(&conn, "ent_s1", "S1", "summary", 100);
            seed_entity(&conn, "ent_s2", "S2", "summary", 100);
            seed_manifest(
                &conn,
                "ent_s1",
                "strict",
                &["thing"],
                &[("from_source", "thing", "thing")],
            );
            seed_manifest(
                &conn,
                "ent_s2",
                "strict",
                &["thing"],
                &[("from_target", "thing", "thing")],
            );
            let mut ctx = edge_test_ctx("ent_s1");
            let outcome = resolve_edge_endpoint_vocabulary(
                &conn, "ent_s1", "ent_s1", "ent_s2", &mut ctx, &gate,
            )
            .unwrap();
            let vocab = outcome.expect("both-strict must gate");
            assert_eq!(
                vocab.canonicalize("from_source"),
                Some("from_source"),
                "both strict → the SOURCE side's vocabulary fires first"
            );
        }

        // ── Row 4: neither endpoint strict (both off rows, no tier_fact
        // row) → no gate, write verbatim.
        {
            let conn = open_in_memory().unwrap();
            seed_entity(&conn, "ent_o1", "O1", "summary", 100);
            seed_entity(&conn, "ent_o2", "O2", "summary", 100);
            seed_manifest(&conn, "ent_o1", "off", &[], &[]);
            seed_manifest(&conn, "ent_o2", "off", &[], &[]);
            let mut ctx = edge_test_ctx("ent_o1");
            let outcome = resolve_edge_endpoint_vocabulary(
                &conn, "ent_o1", "ent_o1", "ent_o2", &mut ctx, &gate,
            )
            .unwrap();
            assert!(
                outcome.is_none(),
                "neither endpoint strict → the edge gate must disarm"
            );
        }
    }

    /// R2.3.0 "both endpoints resolve off → SKIP" (§2.3 rung 2: off =
    /// SKIP, first hit decides): endpoints whose every source sits under an
    /// `off` folder disarm the edge gate even when tier_fact is strict —
    /// pre-fix, an off path "kept climbing" to the strict rung 4.
    #[test]
    fn all_off_source_endpoints_skip_despite_strict_tier_fact() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_a", "A", "summary", 100);
        seed_entity(&conn, "ent_b", "B", "summary", 100);
        let doc_id = seed_document(&conn, "ops/runbook.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        // `seed_chunk` writes an empty content_hash, which never resolves —
        // give the chunk a real one so rung 2 actually sees the path.
        conn.execute(
            "UPDATE chunks SET content_hash = 'h_off_runbook' WHERE id = ?1",
            [chunk_id],
        )
        .unwrap();
        let content_hash: String = conn
            .query_row(
                "SELECT content_hash FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();
        let source_ref = serde_json::json!({
            "evidence": [{ "content_hash": content_hash }]
        })
        .to_string();
        for (fact, ent) in [("fact_a", "ent_a"), ("fact_b", "ent_b")] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    source_ref, created_at, updated_at
                 ) VALUES (?1, ?2, 't', 'body', '[]',
                           'inferred', 'librarian_inferred', ?3, 100, 100)",
                params![fact, ent, source_ref],
            )
            .unwrap();
        }
        seed_manifest(
            &conn,
            "tier_fact",
            "strict",
            &["thing"],
            &[("depends_on", "thing", "thing")],
        );

        let mut ingest = crate::config::IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".to_string(), crate::config::OntologyMode::Off);
        let degraded = crate::config::OntologyDegradedState::default();
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_a");

        let outcome =
            resolve_edge_endpoint_vocabulary(&conn, "ent_a", "ent_a", "ent_b", &mut ctx, &gate)
                .unwrap();
        assert!(
            outcome.is_none(),
            "both endpoints off → the edge gate must SKIP, not climb to tier_fact"
        );
    }

    /// R2.3.4 (review finding): an endpoint with NO resolvable sources
    /// climbs from rung 3 — a host-wide `ontology_default: "off"` disarms
    /// the edge gate even under a strict tier_fact, matching the node
    /// gate's pathless arm. Pre-fix, empty paths jumped straight to rung 4.
    #[test]
    fn sourceless_endpoints_honor_host_default_off() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_a", "A", "summary", 100);
        seed_entity(&conn, "ent_b", "B", "summary", 100);
        seed_manifest(
            &conn,
            "tier_fact",
            "strict",
            &["thing"],
            &[("depends_on", "thing", "thing")],
        );
        let degraded = crate::config::OntologyDegradedState::default();

        let ingest = crate::config::IngestConfig {
            ontology_default: Some(crate::config::OntologyMode::Off),
            ..Default::default()
        };
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_a");
        let outcome =
            resolve_edge_endpoint_vocabulary(&conn, "ent_a", "ent_a", "ent_b", &mut ctx, &gate)
                .unwrap();
        assert!(
            outcome.is_none(),
            "host default off must shield fact-less endpoints, not fall to tier_fact"
        );

        // Control: no host default → rung 3 climbs → strict tier_fact gates.
        let ingest = crate::config::IngestConfig::default();
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_a");
        let outcome =
            resolve_edge_endpoint_vocabulary(&conn, "ent_a", "ent_a", "ent_b", &mut ctx, &gate)
                .unwrap();
        assert!(
            outcome.is_some(),
            "no host default → rung 4 strict tier_fact gates"
        );
    }

    /// §6 item 1b (R2.3.0 rungs 2–3): an endpoint whose SOURCE DIRECTORY
    /// resolves a strict `folder_ontology` prefix gates the edge under the
    /// `tier_fact` vocabulary (mode-vs-vocabulary rule) — even when the
    /// proposing entity and both endpoints carry no manifest row of their
    /// own. This is the scoped-strict-brain leak the final review's
    /// Critical 1 named: pre-fix, the gate read only the proposal entity's
    /// vocabulary and such edges were written ungated.
    #[test]
    fn strict_source_folder_gates_edge_via_tier_fact_vocabulary() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_plain", "Plain", "summary", 100);
        seed_entity(&conn, "ent_scoped", "Scoped", "summary", 100);

        // The scoped endpoint has ONE fact, grounded in a document under
        // the strict `ops/` folder (the rung-2 walk resolves its source).
        let doc_id = seed_document(&conn, "/vault/ops/runbook.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        // `seed_chunk` writes an empty content_hash, which never resolves —
        // give the chunk a real one so rung 2 actually sees the path.
        conn.execute(
            "UPDATE chunks SET content_hash = 'h_strict_runbook' WHERE id = ?1",
            [chunk_id],
        )
        .unwrap();
        let content_hash: String = conn
            .query_row(
                "SELECT content_hash FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_ref, created_at, updated_at
             ) VALUES ('fact_scoped', 'ent_scoped', 'Scoped', 'body', '[]',
                       'inferred', 'librarian_inferred', ?1, 100, 100)",
            [serde_json::json!({
                "evidence": [{ "content_hash": content_hash }]
            })
            .to_string()],
        )
        .unwrap();

        // tier_fact strict with a fallback supplies the rung-2/3 vocabulary.
        seed_manifest(
            &conn,
            "tier_fact",
            "strict",
            &["thing"],
            &[("depends_on", "thing", "thing")],
        );

        let mut ingest = crate::config::IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".to_string(), crate::config::OntologyMode::Strict);
        let degraded = crate::config::OntologyDegradedState::default();
        let gate = edge_gate_ctx(&ingest, &degraded);
        let mut ctx = edge_test_ctx("ent_plain");

        let outcome = resolve_edge_endpoint_vocabulary(
            &conn,
            "ent_plain",
            "ent_plain",
            "ent_scoped",
            &mut ctx,
            &gate,
        )
        .unwrap();
        let vocab = outcome.expect("a strict source folder on either endpoint must gate");
        assert_eq!(
            vocab.canonicalize("depends_on"),
            Some("depends_on"),
            "the gate runs under the tier_fact vocabulary (mode-vs-vocabulary rule)"
        );
        assert!(
            vocab.canonicalize("made_up_type").is_none(),
            "an off-manifest edge type must fail the rung-2 strict gate"
        );

        // Memoization pin (R2.3.0 "once per proposal"): the second lookup
        // hits the per-proposal memo and returns the same verdict. Exactly
        // ONE entry exists — the strict SOURCE resolved first and the
        // strict-wins early return never needed the target's ladder (with
        // `tier_fact` strict, the target would resolve strict too; the
        // source's vocabulary is the one that gates).
        let again = resolve_edge_endpoint_vocabulary(
            &conn,
            "ent_plain",
            "ent_plain",
            "ent_scoped",
            &mut ctx,
            &gate,
        )
        .unwrap();
        assert!(again.is_some(), "memoized resolution must stay strict");
        assert_eq!(
            ctx.edge_endpoint_strict.len(),
            1,
            "the source endpoint memoized exactly once; the strict-wins early \
             return skips the target resolution entirely"
        );
    }

    /// Review finding (rung-2 coverage): after a merge, facts can stay
    /// keyed to the LOSER id — the survivor's rung-2 source paths must
    /// still see them. `endpoint_fact_source_paths` expands the endpoint
    /// id to its redirect cluster, the same coverage `commit_fact_update`,
    /// `commit_fact_archive`, `get_entity` and `batched_cluster_counts`
    /// use. Pre-fix the query matched the survivor id only and a
    /// loser-keyed strict-folder fact was invisible to rungs 2–3.
    #[test]
    fn endpoint_fact_source_paths_cover_loser_keyed_facts() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-surv", "Survivor", "Summary", 100);
        seed_entity(&conn, "ent-loser", "Loser", "Summary", 100);
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent-loser', 'ent-surv', 1)",
            [],
        )
        .unwrap();

        // The loser's one fact, grounded in a resolvable document. The
        // shared resolver core matches evidence by `chunks.content_hash`,
        // so the chunk carries a real (non-empty) hash.
        let doc_id = seed_document(&conn, "/vault/ops/runbook.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        let content_hash = "a".repeat(64);
        conn.execute(
            "UPDATE chunks SET content_hash = ?1 WHERE id = ?2",
            params![content_hash, chunk_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_ref, created_at, updated_at
             ) VALUES ('fact-loser', 'ent-loser', 'Loser fact', 'body', '[]',
                       'inferred', 'librarian_inferred', ?1, 100, 100)",
            [serde_json::json!({
                "evidence": [{ "content_hash": content_hash }]
            })
            .to_string()],
        )
        .unwrap();

        let paths = endpoint_fact_source_paths(&conn, "ent-surv").unwrap();
        assert!(
            paths.iter().any(|p| p.contains("runbook")),
            "the loser-keyed fact's source path must surface in the \
             survivor's rung-2 inputs; got {paths:?}"
        );
    }

    /// §6 items 1/8 + Important-3 (final review): a corrupt `tier_fact`
    /// manifest (strict row, malformed `manifest_json`) + one LLM
    /// new-entity mint → the resolution FAILS with a LOUD §2.4.5
    /// diagnostic naming the ontology gate, the proposal STAYS pending,
    /// and its facts are kept (the items survive untouched — they re-enter
    /// when the manifest is repaired). Pre-fix, the Held arm returned
    /// `Ok((None, false))` and the only guard was the generic
    /// "proposal has no entity_id" bail — safety by accident.
    #[test]
    fn held_llm_mint_fails_loud_proposal_stays_pending_facts_kept() {
        let mut conn = open_in_memory().unwrap();
        seed_pending_proposal(&conn, "prop-held");
        // The corrupt config: tier_fact is strict but its manifest_json is
        // unparseable — rung 4 of the mint ladder reads it and Holds.
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', '{not json', 0)",
            [],
        )
        .unwrap();

        let decisions = all_accept_decisions(&conn, "prop-held");
        let err = resolve_proposal(
            &mut conn,
            "prop-held",
            &decisions,
            None,
            ResolveOptions {
                auto_approve: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("ontology gate held"),
            "the diagnostic must name the ontology gate cause, got: {message}"
        );
        assert!(
            message.contains("§2.4.5") || message.contains("2.4.5"),
            "the diagnostic must name the spec cause, got: {message}"
        );
        assert!(
            message.contains("prop-held"),
            "the diagnostic must name the held proposal, got: {message}"
        );
        // The diagnostic names the ACTUAL cause: a corrupt row, not a
        // missing fallback (the pre-`HoldReason` text blamed a missing
        // `fallback_node_type` for every hold, sending the operator to a
        // fix that changes nothing).
        assert!(
            message.contains("`tier_fact` manifest row could not be read"),
            "the diagnostic must name the unreadable row, got: {message}"
        );
        assert!(
            !message.contains("fallback"),
            "an unreadable row must not be diagnosed as a missing fallback, got: {message}"
        );

        // Facts kept + proposal held: still pending, item untouched, and
        // no entity was minted by the aborted resolution.
        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposals WHERE id = 'prop-held'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending", "a held proposal stays pending");
        let item_status: String = conn
            .query_row(
                "SELECT status FROM curated_proposal_items WHERE id = 'item-prop-held'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            item_status, "pending",
            "the held proposal's facts are kept, not dropped"
        );
        let entities: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entities, 0, "the held mint must not insert an entity");

        // Retry after the repair succeeds (§2.4.5: facts re-enter) — the
        // diagnostic is actionable, not a dead end.
        conn.execute(
            "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1
             WHERE entity_id = 'tier_fact'",
            [serde_json::json!({
                "node_types": [{"type": "project"}],
                "edge_types": [],
                "fallback_node_type": "project"
            })
            .to_string()],
        )
        .unwrap();
        let decisions = all_accept_decisions(&conn, "prop-held");
        let result = resolve_proposal(
            &mut conn,
            "prop-held",
            &decisions,
            None,
            ResolveOptions {
                auto_approve: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.proposal_status, "approved");
    }
    // ── Task 7 (spec R2.7.5 / r13-MAJOR-1): commit-path redirect write
    // resolution — a fact naming a merged-away loser lands on the SURVIVOR.

    /// r15-m4 required test: commit a fact naming a merged-away loser,
    /// assert it lands on the survivor (resolution happens once, where
    /// ctx.entity_id is fixed, so every item type in the commit inherits
    /// it).
    #[test]
    fn commit_fact_naming_loser_lands_on_survivor() {
        let mut conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent_surv", "Adrian", "same summary", 100);
        seed_entity(&conn, "ent_lose", "Adrian", "same summary", 100);
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent_lose','ent_surv',1)",
            [],
        )
        .unwrap();

        let doc_id = seed_document(&conn, "/vault/documents/notes.md");
        let chunk_id = seed_chunk(&conn, doc_id);
        insert_test_proposal(
            &conn,
            "prop-redir",
            ProposalKind::UpdateEntity,
            Some("ent_lose"),
            vec![fact_item(
                "item-redir",
                chunk_id,
                "Fact via a stale loser id.",
            )],
            doc_id,
        );

        let decisions = all_accept_decisions(&conn, "prop-redir");
        let result = resolve_proposal(
            &mut conn,
            "prop-redir",
            &decisions,
            None,
            ResolveOptions {
                auto_approve: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.proposal_status, "approved");

        let owner: String = conn
            .query_row(
                "SELECT entity_id FROM llm_wiki_entries
                 WHERE entity_id IN ('ent_surv','ent_lose') AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(owner, "ent_surv", "the fact must land on the survivor");
        let loser_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE entity_id = 'ent_lose'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(loser_rows, 0, "nothing attaches to the loser");
    }
}

/// D-tests for the `source_ref_is_still_grounded` consumer contract. Lives
/// in commit.rs so it shares `super::*` with the helper under test and the
/// in-memory connection. The five tests pin every branch of the helper:
/// legacy-path lookup, JSON parse, evidence-empty short-circuit, chunk-id
/// presence, and the parse-error "still-grounded" defensive policy that
/// Bug A's spec review called out.
#[cfg(test)]
mod source_ref_grounded_tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn insert_doc_indexed(conn: &Connection, path: &str) -> i64 {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES (?1, 'h', 'user_doc', 'indexed')",
            [path],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_chunk_for_doc(conn: &Connection, doc_id: i64, hash: &str) -> i64 {
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line,
                                 symbol_name, strategy, content_hash)
             VALUES (?1, 'ct', 0, 1, 3, NULL, 'prose', ?2)",
            rusqlite::params![doc_id, hash],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// D1 — Legacy path lookup: a `documents.path` match with
    /// `status='indexed'` is "still grounded". A non-indexed status or a
    /// missing path returns false.
    #[test]
    fn legacy_path_indexed_returns_true_missing_returns_false() {
        let conn = open_in_memory().unwrap();
        insert_doc_indexed(&conn, "documents/gone.md");
        // Flip the status to 'pending' so the legacy existence check fails.
        conn.execute(
            "UPDATE documents SET status = 'pending' WHERE path = ?1",
            ["documents/gone.md"],
        )
        .unwrap();
        assert!(
            !source_ref_is_still_grounded(&conn, "documents/gone.md"),
            "non-indexed status must report not-grounded"
        );

        // Set it back to indexed and re-check.
        conn.execute(
            "UPDATE documents SET status = 'indexed' WHERE path = ?1",
            ["documents/gone.md"],
        )
        .unwrap();
        assert!(
            source_ref_is_still_grounded(&conn, "documents/gone.md"),
            "indexed legacy path must report still-grounded"
        );

        // And a path that doesn't exist at all.
        assert!(
            !source_ref_is_still_grounded(&conn, "documents/missing.md"),
            "unknown legacy path must report not-grounded"
        );
    }

    /// D2 — JSON-shape happy path: at least one referenced chunk exists →
    /// still-grounded.
    #[test]
    fn json_with_live_chunk_returns_true() {
        let conn = open_in_memory().unwrap();
        let doc_id = insert_doc_indexed(&conn, "documents/notes.md");
        let chunk_id = insert_chunk_for_doc(&conn, doc_id, "h_alive");
        let src = format!(
            r#"{{"proposal_id":"p1","evidence":[{{"chunk_id":{chunk_id},"content_hash":"h_alive","quote":"q","start_line":1,"end_line":3}}]}}"#
        );
        assert!(source_ref_is_still_grounded(&conn, &src));
    }

    /// D3 — JSON-shape with all dead chunks → not-grounded. Pins the
    /// "partial evidence keeps the entry" rule's negative: every chunk_id
    /// gone means we soft-delete.
    #[test]
    fn json_with_all_dead_chunks_returns_false() {
        let conn = open_in_memory().unwrap();
        let doc_id = insert_doc_indexed(&conn, "documents/x.md");
        let chunk_id = insert_chunk_for_doc(&conn, doc_id, "h_dead");
        conn.execute("DELETE FROM chunks WHERE id = ?1", [chunk_id])
            .unwrap();
        let src = format!(
            r#"{{"proposal_id":"p1","evidence":[{{"chunk_id":{chunk_id},"content_hash":"h_dead","quote":"q","start_line":1,"end_line":3}}]}}"#
        );
        assert!(
            !source_ref_is_still_grounded(&conn, &src),
            "JSON with all-dead chunks must report not-grounded"
        );
    }

    /// D4 — Empty evidence array → still-grounded. The MANUAL_SOURCE_REF
    /// sentinel has `evidence:[]`; those rows are user_stated and the heal
    /// must never delete them based on the source_ref shape.
    #[test]
    fn json_with_empty_evidence_returns_true() {
        let conn = open_in_memory().unwrap();
        assert!(source_ref_is_still_grounded(
            &conn,
            r#"{"proposal_id":null,"evidence":[]}"#
        ));
    }

    /// D5 — Parse-error defensive branch. JSON-looking-but-malformed input
    /// returns `true` (no soft-delete) rather than false. This is the
    /// Bug A spec-review contract: a row that can't be parsed isn't
    /// *demonstrably* stale — it's a legacy path or a future producer we
    /// don't know about yet. Logging is the right response, not deletion.
    #[test]
    fn malformed_json_returns_true_defensive() {
        let conn = open_in_memory().unwrap();
        assert!(
            source_ref_is_still_grounded(&conn, "{not valid json"),
            "malformed JSON must defensively return true (no soft-delete)"
        );
        // A JSON-shaped value without an `evidence` key also returns true
        // (not a JSON parse failure, but no librarian evidence either).
        assert!(source_ref_is_still_grounded(
            &conn,
            r#"{"proposal_id":"p1"}"#
        ));
        // Empty string and whitespace-only strings also return true.
        assert!(source_ref_is_still_grounded(&conn, ""));
        assert!(source_ref_is_still_grounded(&conn, "   \t  "));
    }

    /// D6 — Defensive DB-error branch. A failing lookup (broken
    /// connection, corrupted schema, lock contention) must NOT be
    /// interpreted as "demonstrably stale" — same policy as the
    /// parse-error branch above. The previous `unwrap_or(None)`
    /// collapsed `Err(...)` into `None` and could over-delete on DB
    /// faults, which is the opposite of what the spec wants. We pin
    /// both call sites here: legacy-path lookup against `documents`
    /// and chunk lookup against `chunks`.
    ///
    /// We provoke the DB error by dropping the underlying table so
    /// the SELECT raises `SqliteFailure` (vs. `NoRows` — that's the
    /// distinction the new match arm is making).
    #[test]
    fn dropped_table_db_error_returns_true_defensive() {
        // Legacy-path branch: documents lookup against a vanished table.
        let conn = open_in_memory().unwrap();
        conn.execute("DROP TABLE documents", [])
            .expect("drop documents");
        assert!(
            source_ref_is_still_grounded(&conn, "documents/anything.md"),
            "legacy-path DB error must defensively return true (no soft-delete)"
        );

        // JSON branch: chunks lookup against a vanished table. Use a
        // shape with a real chunk_id so the code reaches the chunks
        // SELECT before failing on the missing table.
        let conn = open_in_memory().unwrap();
        conn.execute("DROP TABLE chunks", []).expect("drop chunks");
        let src = r#"{"proposal_id":"p1","evidence":[{"chunk_id":1,"content_hash":"h","quote":"q","start_line":1,"end_line":3}]}"#;
        assert!(
            source_ref_is_still_grounded(&conn, src),
            "JSON-path DB error must defensively return true (no soft-delete)"
        );
    }

    /// D7 — the seventh D-test. §2.1 deliberately does not rely on FK CASCADE,
    /// so a token row whose evidence row is missing is a *when*, not an *if*.
    /// The existing defensive posture (parse errors and DB errors are grounded)
    /// extends here: treat as grounded and warn loudly. Never auto-purge.
    #[test]
    fn d7_token_row_without_evidence_is_treated_as_grounded() {
        let conn = open_in_memory().unwrap();
        let token = librarian_source_ref_token("fact_orphan");
        assert!(
            source_ref_is_still_grounded(&conn, &token),
            "a token with no librarian_evidence row must not be soft-deleted"
        );
    }

    /// A token row whose evidence row exists and anchors no live chunk is
    /// demonstrably stale — the heal soft-deletes it.
    #[test]
    fn token_row_with_dangling_evidence_is_not_grounded() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, access_count)
             VALUES ('fact_d','ent','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
            [librarian_source_ref_token("fact_d")],
        )
        .unwrap();
        insert_librarian_evidence(
            &conn,
            "fact_d",
            "prop_d",
            r#"{"evidence":[{"chunk_id":999,"content_hash":"gone"}],"proposal_id":"prop_d"}"#,
            false,
            1,
        )
        .unwrap();
        assert!(!source_ref_is_still_grounded(
            &conn,
            &librarian_source_ref_token("fact_d")
        ));
    }

    /// Phase-2 revert (spec §2.3): grounding for token rows is strictly
    /// evidence-based. A flagged row whose evidence anchors no live chunk is
    /// NOT grounded — heal soft-deletes it, prune finishes it.
    #[test]
    fn phase2_unanchored_rows_with_no_live_chunk_are_not_grounded() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, access_count)
             VALUES ('fact_u','ent','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
            [librarian_source_ref_token("fact_u")],
        )
        .unwrap();
        insert_librarian_evidence(
            &conn,
            "fact_u",
            "prop_u",
            r#"{"evidence":[],"proposal_id":"prop_u"}"#,
            true,
            1,
        )
        .unwrap();
        assert!(!source_ref_is_still_grounded(
            &conn,
            &librarian_source_ref_token("fact_u")
        ));
    }

    /// Flagged-but-actually-anchored rows ARE grounded (the re-grade clears
    /// them, but heal must also be correct on the flagged value itself).
    #[test]
    fn phase2_flagged_but_anchored_rows_are_grounded() {
        let conn = open_in_memory().unwrap();
        // Seed a live chunk, an entry with a token ref, and a FLAGGED
        // (unanchored=1) evidence row whose JSON carries that live chunk's
        // content_hash — grounding must follow the evidence, not the flag.
        let doc_id = super::tests::seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = super::tests::seed_chunk(&conn, doc_id);
        let hash: String = conn
            .query_row(
                "SELECT content_hash FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, access_count)
             VALUES ('fact_a','ent','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
            [librarian_source_ref_token("fact_a")],
        )
        .unwrap();
        insert_librarian_evidence(
            &conn,
            "fact_a",
            "prop_a",
            &format!(r#"{{"evidence":[{{"chunk_id":{chunk_id},"content_hash":"{hash}"}}],"proposal_id":"prop_a"}}"#),
            true,
            1,
        )
        .unwrap();
        assert!(source_ref_is_still_grounded(
            &conn,
            &librarian_source_ref_token("fact_a")
        ));
    }
}
