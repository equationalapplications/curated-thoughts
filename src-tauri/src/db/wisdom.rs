//! Manual wisdom CRUD from Brain mode entity pages — mirrors commit.rs write conventions
//! (ms timestamps on llm_wiki_entries, outbox rows, curated_entities touch).

use crate::db::commit::{
    fact_title_from_body, now_timestamps, push_entries_outbox, wiki_fact_outbox_payload,
};
use crate::db::outbox_format::OutboxOperation;
use crate::embedder::{embed_batch, EmbedProfile};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::io::Write;

// Task 9 (user-confirmation-queue): the app's DIRECT `user_stated` write is
// RETIRED (spec §3.7.6, §2 rejected alternative "keeping the direct app write
// alongside Path B"). Statements typed in the app are captured as `stated/`
// files and ingested through the Librarian (`capture_statement`, below) so
// they gain evidence and become deliverable mid-task. Update/archive of
// existing facts keep their direct surfaces — only the INSERT path was the
// two-semantics hole.

fn assert_entity_active(conn: &Connection, entity_id: &str) -> Result<String> {
    // Task 7 (r13-MAJOR-1): a fact mutator keyed by a merged-away loser id
    // resolves to the survivor BEFORE acting — the fact lands on the entity
    // recall shows, never on a row no reader surfaces. Returns the resolved
    // survivor id so callers key every write to it.
    let resolved = crate::db::entities::resolve_entity_id(conn, entity_id)?;
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM live_entities WHERE id = ?1 AND deleted_at IS NULL",
            [&resolved],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_none() {
        bail!("entity not found or archived: {entity_id}");
    }
    Ok(resolved)
}

fn touch_entity(conn: &Connection, entity_id: &str, now_secs: i64) -> Result<()> {
    conn.execute(
        "UPDATE curated_entities SET updated_at = ?1 WHERE id = ?2",
        params![now_secs, entity_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Path B capture (spec §3.7): statements typed in the app become stated/ files,
// are attested by the app, and are ingested through the Librarian kick.
// ---------------------------------------------------------------------------

/// Author of app-captured statements. Single-user, local-first: identity is a
/// string stamped on paths and frontmatter (spec §3.10 "Who verifies").
pub const STATED_AUTHOR: &str = "kurt";

/// Attestation identity for app-written statements and queue confirmations.
pub const HUMAN_ATTESTER: &str = "human:kurt";

/// Frontmatter capture-method marker for statements typed into the app.
pub const CAPTURE_METHOD: &str = "app-statement";

/// Everything the capture flow needs: where the brain db and the vault live,
/// and the embed profile the kick's ingest leg should run under.
pub struct CaptureContext {
    pub db_path: std::path::PathBuf,
    pub vault_root: std::path::PathBuf,
    pub profile: crate::embedder::EmbedProfile,
}

/// What the queue-level confirm-or-capture entry did.
pub enum QueueStatementOutcome {
    /// The fact existed and was confirmed (`queue::confirm_fact_at`).
    Confirmed,
    /// No backing fact (or a fresh statement): Path B captured it; the string
    /// is the vault-relative `stated/` document path.
    Captured(String),
}

/// Capture a statement typed into the app (Path B entry point, spec §3.7).
/// Renders `stated/kurt/<year>/<slug>.md`, writes the attestation of record
/// and its `.attestations.jsonl` mirror, registers the kick, and runs the
/// kick synchronously — the caller IS the host (Tauri command / test).
pub fn capture_statement(ctx: &CaptureContext, text: &str) -> Result<String> {
    capture_statement_with(ctx, text, None)
}

/// [`capture_statement`] with an injected librarian completer — Task 9's
/// deterministic test seam (see `wisdom_deposit::run_kick_with`). Production
/// callers use [`capture_statement`], which uses the configured provider.
pub(crate) fn capture_statement_with(
    ctx: &CaptureContext,
    text: &str,
    kick: Option<&dyn crate::librarian::LlmCompleter>,
) -> Result<String> {
    let mut conn = open_capture_conn(&ctx.db_path)?;
    capture_statement_on(&mut conn, ctx, text, kick)
}

/// Rule 15 / spec §9: the queue-level confirm. `Some(fact_id)` with a live
/// fact confirms it; a confirm-with-no-fact, or a fresh statement typed into
/// the queue view, routes through `capture_statement` — the app CREATES the
/// fact the only pipeline-consistent way (stated/ file, attest, kick), never
/// a direct insert.
pub fn confirm_or_capture(
    ctx: &CaptureContext,
    fact_id: Option<&str>,
    fresh_text: Option<&str>,
) -> Result<QueueStatementOutcome> {
    confirm_or_capture_with(ctx, fact_id, fresh_text, None)
}

/// [`confirm_or_capture`] with the test completer seam.
pub(crate) fn confirm_or_capture_with(
    ctx: &CaptureContext,
    fact_id: Option<&str>,
    fresh_text: Option<&str>,
    kick: Option<&dyn crate::librarian::LlmCompleter>,
) -> Result<QueueStatementOutcome> {
    let mut conn = open_capture_conn(&ctx.db_path)?;
    if let Some(fact_id) = fact_id {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM llm_wiki_entries WHERE id = ?1 AND deleted_at IS NULL",
                [fact_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_some() {
            crate::db::queue::confirm_fact_at(&conn, &ctx.vault_root, fact_id)?;
            return Ok(QueueStatementOutcome::Confirmed);
        }
        // Fall through: the fact is gone (superseded/archived/never was) —
        // spec §9 sends the statement down Path B instead of failing.
    }
    let text = fresh_text
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "fact not found or already archived and no statement text given: {fact_id:?}"
            )
        })?;
    let rel = capture_statement_on(&mut conn, ctx, text, kick)?;
    Ok(QueueStatementOutcome::Captured(rel))
}

/// Open the brain db READ-WRITE, NO-CREATE (the with_rw contract): a vanished
/// or misconfigured brain must not be resurrected as an empty database.
fn open_capture_conn(db_path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .with_context(|| format!("open brain {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// The whole capture: render + write the file, write the attestation row and
/// its ledger mirror, register the kick, run the kick. Order matters: the
/// FILE is written first (the pipeline reads bytes from the vault), the
/// attestation row + ledger line second (so the Path B provenance arm finds
/// a row whose hash matches the ingested document), the kick registration
/// third, and the kick last.
///
/// NOT atomic (verdict-corrected 2026-10-22): the steps are separate
/// autocommits, so a failure between them can leave the file and its
/// attestation row behind without a ledger line. That is TOLERATED BY
/// DESIGN — the ledger line is the durable carrier, and the rebuild
/// re-derives the table from it, dropping a row whose line is missing
/// (spec §8; Task-5 E2 ruling). Do not paper over this with a transaction:
/// hiding the partial state would hide the inconsistency the rebuild
/// resolves. A kick failure is reported via the kick rows, not the return
/// (`run_kick_with`).
fn capture_statement_on(
    conn: &mut Connection,
    ctx: &CaptureContext,
    text: &str,
    kick: Option<&dyn crate::librarian::LlmCompleter>,
) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        bail!("statement text must not be empty");
    }
    let at_iso = chrono::Utc::now().to_rfc3339();
    // Never overwrite an existing statement file: create with
    // `create_new(true)` — atomic at the FS level, so two concurrent captures
    // of the same text cannot both win the slug (an exists()-then-write
    // pre-check races: the loser truncates the winner's file and its
    // INSERT OR REPLACE steals the attestation row). An AlreadyExists loser
    // retries with the next `-N` suffix, exactly as the deposit path does.
    let year = chrono::Utc::now().format("%Y");
    let base = slugify(text);
    let dir = ctx.vault_root.join(format!("stated/{STATED_AUTHOR}/{year}"));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut n = 1u32;
    let (rel, full) = loop {
        let suffix = if n == 1 { String::new() } else { format!("-{n}") };
        let rel = format!("stated/{STATED_AUTHOR}/{year}/{base}{suffix}.md");
        let full = dir.join(format!("{base}{suffix}.md"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
        {
            Ok(mut file) => {
                file.write_all(render_statement(text, &at_iso).as_bytes())
                    .with_context(|| format!("writing {}", full.display()))?;
                break (rel, full);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("writing {}", full.display())));
            }
        }
    };

    // Attestation of record (spec §3.7.2): the brain-side row is the index
    // the Path B provenance arm joins on; the .attestations.jsonl line is
    // the durable carrier the rebuild re-derives from (spec §3.8). Both are
    // keyed to the SAME hash routine the documents table uses, so the
    // attested hash matches the ingested document exactly. The TABLE KEY is
    // the watcher's virtual-absolute `documents.path` form (`virtual_path_key`,
    // defect B1): in the running app the watcher — not this synchronous kick —
    // ingests the capture, and the Path B join (`a.document_path = d.path`)
    // must match its staged row. The ledger carrier below stays
    // vault-relative (`rel`, spec §3.8).
    let bytes = std::fs::read(&full)?;
    let content_hash = crate::hasher::hash_bytes(&bytes);
    let attestation_key = super::queue::virtual_path_key(&ctx.vault_root.join(&rel))?;
    conn.execute(
        "INSERT OR REPLACE INTO stated_attestations
             (document_path, content_hash, attested_by, attested_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![attestation_key, content_hash, HUMAN_ATTESTER, at_iso],
    )?;
    crate::db::stated_ledger::append_attestation(
        &ctx.vault_root,
        &crate::db::stated_ledger::AttestationLine {
            document_path: rel.clone(),
            content_hash,
            by: HUMAN_ATTESTER.to_string(),
            at: at_iso,
        },
    )?;

    // App-written kick (spec §3.7.3): the APP registers the ledger row.
    crate::wisdom_deposit::register_app_kick(conn, &rel)?;
    crate::wisdom_deposit::run_kick_with(
        ctx.db_path.clone(),
        ctx.vault_root.clone(),
        rel.clone(),
        ctx.profile.clone(),
        kick,
    );
    Ok(rel)
}

/// Filesystem-safe slug: lowercase ascii alphanumerics and single dashes,
/// capped at 48 chars, never empty (`statement` fallback).
fn slugify(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.len() >= 48 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "statement".to_string()
    } else {
        trimmed
    }
}

/// The stated/ note body: YAML frontmatter recording WHO said it, HOW it was
/// captured, and the human-readable attestation note (spec §3.7.1). Free text
/// stays in the BODY — frontmatter values are app-controlled constants and a
/// quoted ISO timestamp only, so no user text can break the YAML.
fn render_statement(text: &str, at_iso: &str) -> String {
    format!(
        "---\nattributed_speaker: {STATED_AUTHOR}\ncapture_method: {CAPTURE_METHOD}\n\
         attestation: \"{HUMAN_ATTESTER} attested this statement in the Curated Thoughts desktop app at {at_iso}\"\n\
         ---\n\n{text}\n"
    )
}

/// Compute the embedding blob for a single user-authored wisdom entry, OUTSIDE any
/// DB or app-level mutex. `embed_batch` is a blocking network call and must
/// never run while a write lock is held.
///
/// Returns `None` when no profile is configured, the body is empty after
/// trim, or the provider fails — the caller commits the wisdom anyway and
/// leaves the blob NULL for the null-embedding sweep to retry.
pub fn precompute_entry_embedding(profile: Option<&EmbedProfile>, body: &str) -> Option<Vec<u8>> {
    let profile = profile?;
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let title = fact_title_from_body(body);
    let text = crate::embed_sweep::embed_text_for_entry(&title, body);
    match embed_batch(profile, vec![text]) {
        // Defensive cardinality check, mirroring the `vectors.len() != id_chunk.len()`
        // guard in `db/commit.rs`: one text in, exactly one vector out. Both
        // shipping providers bail on a mismatch, but `pop()` alone would silently
        // persist the *last* vector of an over-long response, and the sweep's
        // `embedding_blob IS NULL` predicate could never correct that mis-association.
        Ok(vectors) if vectors.len() == 1 => vectors
            .into_iter()
            .next()
            .map(|v| crate::wiki_graph::f32_vec_to_blob(&v)),
        Ok(vectors) => {
            // False positive: the eprintln! below only interpolates the usize count
            // (vectors.len) — the API key resolved inside embed_batch never reaches
            // this format string.
            // codeql[rust/cleartext-logging]
            eprintln!(
                "precompute_entry_embedding: expected 1 vector, got {}; leaving NULL for the sweep",
                vectors.len()
            );
            None
        }
        Err(e) => {
            // `e` is an anyhow chain from embed_batch; the resolved API key is
            // used only inside the Bearer header and never reaches this format
            // string (the chain carries API-key env-var *names* only).
            // codeql[rust/cleartext-logging]
            eprintln!(
                "precompute_entry_embedding: provider failed, leaving NULL for the sweep: {e}"
            );
            None
        }
    }
}

/// Rewrite a wisdom entry's body (title re-derived); pushes full-payload outbox UPDATE.
///
/// Equivalent to `update_wisdom_with_blob(conn, entity_id, wisdom_id, body, None)`
/// — the entry lands with a NULL embedding for the sweep to fill. Kept for
/// tests and callers with no embed profile to hand.
pub fn update_wisdom(
    conn: &mut Connection,
    entity_id: &str,
    wisdom_id: &str,
    body: &str,
) -> Result<()> {
    update_wisdom_with_blob(conn, entity_id, wisdom_id, body, None)
}

/// Rewrite a wisdom entry's body, storing a caller-computed embedding blob.
///
/// The blob must be computed OUTSIDE the caller's DB lock via
/// [`precompute_entry_embedding`]: `embed_batch` is a blocking network round-trip.
///
/// `None` writes NULL, which is what a provider failure collapses to — the
/// null-embedding sweep re-derives it later. Passing the fresh blob is what
/// keeps an edited wisdom entry searchable immediately instead of falling out of
/// semantic retrieval until the next sweep trigger (which this path does not
/// itself fire).
pub fn update_wisdom_with_blob(
    conn: &mut Connection,
    entity_id: &str,
    wisdom_id: &str,
    body: &str,
    embedding_blob: Option<Vec<u8>>,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    update_wisdom_in_tx(&tx, entity_id, wisdom_id, body, embedding_blob)?;
    tx.commit()?;
    Ok(())
}

/// Transaction-scoped core of [`update_wisdom_with_blob`] (no BEGIN/COMMIT;
/// same fail-closed audit discipline as the other transaction-scoped writers).
pub fn update_wisdom_in_tx(
    tx: &Transaction<'_>,
    entity_id: &str,
    wisdom_id: &str,
    body: &str,
    embedding_blob: Option<Vec<u8>>,
) -> Result<()> {
    let body = body.trim();
    if body.is_empty() {
        bail!("wisdom body must not be empty");
    }
    let (now_secs, now_ms) = now_timestamps();
    let title = fact_title_from_body(body);

    let entity_id = assert_entity_active(tx, entity_id)?;
    // Transitive fact closure (r13-m3): the row being updated may still be
    // keyed to a redirected loser from before the merge.
    let cluster = crate::db::entities::cluster_ids(tx, &entity_id)?;
    let placeholders = crate::db::entities::in_placeholders(&cluster);
    let existing = tx
        .query_row(
            &format!(
                "SELECT tags, confidence, source_type, COALESCE(source_ref, ''), created_at,
                        source_hash, okf_type, okf_sources, okf_verified, okf_usage_window,
                        lifecycle_status, stale_after, generated_by,
                        last_verified_at, last_verified_by
                 FROM llm_wiki_entries
                 WHERE id = ? AND entity_id IN ({placeholders}) AND deleted_at IS NULL"
            ),
            rusqlite::params_from_iter(
                std::iter::once(wisdom_id)
                    .map(String::from)
                    .chain(cluster.iter().cloned()),
            ),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                    r.get::<_, Option<i64>>(13)?,
                    r.get::<_, Option<String>>(14)?,
                ))
            },
        )
        .optional()?;
    let Some((
        tags_raw,
        confidence,
        source_type,
        source_ref,
        created_at,
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
        bail!("wisdom not found or archived: {wisdom_id}");
    };

    // Write the caller's freshly computed vector, or NULL when there is none
    // so the sweep re-derives it — never leave a vector describing text the
    // entry no longer contains. Mirrors `commit_fact_update`. The scheme
    // stamp rides in the same statement: a fresh vector re-stamps, and a
    // NULL blob re-stamps too — the old blob is discarded with the body it
    // described, so the row lands consistent for the sweep (which embeds
    // under the WRITE scheme).
    //
    // The lookup above matched anywhere in the survivor's redirect cluster,
    // so a pre-merge row may still be keyed to a loser — rekey it to the
    // survivor here so the persisted row and the outbox payload agree (the
    // #132 prisma-outbox divergence class `create_task` guards against).
    tx.execute(
        "UPDATE llm_wiki_entries
            SET title = ?1, body = ?2, updated_at = ?3, embedding_blob = ?4,
                embed_scheme = ?5, entity_id = ?7
          WHERE id = ?6",
        params![
            title,
            body,
            now_ms,
            embedding_blob,
            crate::embed_scheme::WRITE_SCHEME,
            wisdom_id,
            entity_id
        ],
    )?;
    let tags: Vec<String> = serde_json::from_str(&tags_raw).unwrap_or_default();
    push_entries_outbox(
        tx,
        &entity_id,
        wisdom_id,
        OutboxOperation::Update,
        wiki_fact_outbox_payload(
            wisdom_id,
            &entity_id,
            &title,
            body,
            &tags,
            &confidence,
            &source_type,
            existing_source_hash.as_deref(),
            &source_ref,
            existing_okf_type.as_deref(),
            existing_okf_sources.as_deref(),
            existing_okf_verified.as_deref(),
            existing_okf_usage_window.as_deref(),
            created_at,
            now_ms,
            None,
            Some(existing_lifecycle_status.as_str()),
            existing_stale_after,
            existing_generated_by.as_deref(),
            existing_last_verified_at,
            existing_last_verified_by.as_deref(),
        ),
        now_ms,
    )?;
    touch_entity(tx, &entity_id, now_secs)?;
    Ok(())
}

/// Soft-delete a wisdom entry; pushes minimal outbox DELETE (same shape as commit_fact_archive).
pub fn archive_wisdom(conn: &mut Connection, entity_id: &str, wisdom_id: &str) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    archive_wisdom_in_tx(&tx, entity_id, wisdom_id)?;
    tx.commit()?;
    Ok(())
}

/// Transaction-scoped core of [`archive_wisdom`] (no BEGIN/COMMIT; same
/// fail-closed audit discipline as the other transaction-scoped writers).
pub fn archive_wisdom_in_tx(tx: &Transaction<'_>, entity_id: &str, wisdom_id: &str) -> Result<()> {
    let (now_secs, now_ms) = now_timestamps();

    let entity_id = assert_entity_active(tx, entity_id)?;
    // Transitive fact closure (r13-m3), same as `update_wisdom_in_tx`: the
    // row may still be keyed to a redirected loser from before the merge.
    // Widen the match to the survivor's cluster and rekey the row to the
    // survivor in the same UPDATE, so the archived row and the outbox
    // payload agree (#132 prisma-outbox divergence class) — without this,
    // archiving a loser-keyed row bails "not found" even though reads
    // surface it through the redirect.
    let cluster = crate::db::entities::cluster_ids(tx, &entity_id)?;
    let placeholders = (4..4 + cluster.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut bind: Vec<&dyn rusqlite::ToSql> = vec![&now_ms, &entity_id, &wisdom_id];
    for id in &cluster {
        bind.push(id);
    }
    let changes = tx.execute(
        &format!(
            "UPDATE llm_wiki_entries
             SET deleted_at = ?1, updated_at = ?1, entity_id = ?2
             WHERE id = ?3 AND entity_id IN ({placeholders}) AND deleted_at IS NULL"
        ),
        bind.as_slice(),
    )?;
    if changes == 0 {
        bail!("wisdom not found or already archived: {wisdom_id}");
    }

    // Edges die with their endpoints, inside this same transaction (spec §2).
    crate::db::edge_purge::purge_edges_for_entry(tx, wisdom_id)?;

    push_entries_outbox(
        tx,
        &entity_id,
        wisdom_id,
        OutboxOperation::Delete,
        serde_json::json!({
            "id": wisdom_id,
            "entity_id": entity_id,
            "deleted_at": now_ms,
        }),
        now_ms,
    )?;
    touch_entity(tx, &entity_id, now_secs)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::{open_in_memory, AppDb};
    use crate::db::entities::{create_entity, get_entity, CreateEntityInput};
    // -------------------------------------------------------------------------
    // pre-existing tests
    // -------------------------------------------------------------------------

    fn make_entity(conn: &mut Connection) -> String {
        create_entity(
            conn,
            &CreateEntityInput {
                name: "Subject".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap()
        .id
    }

    /// Task 9 fixture: insert one live fact row DIRECTLY. The retired
    /// `add_wisdom` fixture is gone; update/archive tests only need a row to
    /// act on. Same column shape the retired INSERT used, so existing
    /// assertions hold.
    struct SeededFact {
        id: String,
    }

    fn seed_fact(conn: &Connection, entity_id: &str, body: &str) -> SeededFact {
        let (now_secs, now_ms) = now_timestamps();
        let id = crate::db::commit::generate_llm_id("fact_");
        let title = fact_title_from_body(body);
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES (?1, ?2, ?3, ?4, '[]', 'confirmed', 'user_stated', NULL, NULL, ?5, ?5, NULL, 0, NULL, NULL, ?6, NULL)",
            params![
                id,
                entity_id,
                title,
                body,
                now_ms,
                crate::embed_scheme::WRITE_SCHEME
            ],
        )
        .unwrap();
        let _ = now_secs;
        SeededFact { id: id.clone() }
    }

    /// [`seed_fact`] with a non-NULL embedding blob (for the sweep tests).
    fn seed_fact_with_blob(conn: &Connection, entity_id: &str, body: &str) -> SeededFact {
        let fact = seed_fact(conn, entity_id, body);
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = ?2",
            params![vec![7u8; 32], fact.id],
        )
        .unwrap();
        fact
    }

    fn outbox_count(conn: &Connection, record_id: &str, operation: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM llm_wiki_outbox
             WHERE record_id = ?1 AND table_name = 'entries' AND operation = ?2",
            params![record_id, operation],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn update_wisdom_rewrites_body_and_pushes_outbox_update() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        let fact = seed_fact(&conn, &entity_id, "Old body.");

        update_wisdom(
            &mut conn,
            &entity_id,
            &fact.id,
            "New body with more detail.",
        )
        .unwrap();

        let loaded = get_entity(&conn, &entity_id).unwrap().unwrap();
        assert_eq!(loaded.facts[0].body, "New body with more detail.");
        assert_eq!(loaded.facts[0].title, "New body with more detail.");
        assert_eq!(outbox_count(&conn, &fact.id, "UPDATE"), 1);
    }

    #[test]
    fn update_wisdom_clears_embedding_blob_so_sweep_rederives_it() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let mut conn = open_in_memory().unwrap();
            let entity_id = make_entity(&mut conn);

            // Seed a fact with a real (non-NULL) embedding blob.
            let fact = seed_fact_with_blob(&conn, &entity_id, "Original body.");
            let blob_before: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT embedding_blob FROM llm_wiki_entries WHERE id = ?1",
                    [&fact.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                blob_before.is_some(),
                "precondition: seeded row must have a non-NULL blob",
            );

            // Edit the fact — body changes, blob must be wiped.
            update_wisdom(&mut conn, &entity_id, &fact.id, "Edited body.").unwrap();

            let blob_after: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT embedding_blob FROM llm_wiki_entries WHERE id = ?1",
                    [&fact.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                blob_after, None,
                "update_wisdom must NULL embedding_blob so the sweep re-derives it",
            );
        });
    }

    #[test]
    fn update_wisdom_rejects_unknown_or_archived_fact() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        assert!(update_wisdom(&mut conn, &entity_id, "fact_missing", "x").is_err());
        let fact = seed_fact(&conn, &entity_id, "Body.");
        archive_wisdom(&mut conn, &entity_id, &fact.id).unwrap();
        assert!(update_wisdom(&mut conn, &entity_id, &fact.id, "x").is_err());
    }

    #[test]
    fn archive_wisdom_soft_deletes_and_pushes_outbox_delete() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        let fact = seed_fact(&conn, &entity_id, "Ephemeral.");

        archive_wisdom(&mut conn, &entity_id, &fact.id).unwrap();

        let loaded = get_entity(&conn, &entity_id).unwrap().unwrap();
        assert!(loaded.facts.is_empty(), "archived fact must not be listed");
        assert_eq!(outbox_count(&conn, &fact.id, "DELETE"), 1);
        assert!(
            archive_wisdom(&mut conn, &entity_id, &fact.id).is_err(),
            "double archive errors"
        );
    }

    /// Issue #265 Test (c) (write path): the body UPDATE stamps
    /// `embed_scheme = WRITE_SCHEME` in the same statement that clears the
    /// blob. (The INSERT half of this test retired WITH the direct app write
    /// — Task 9: user statements enter through the librarian pipeline, whose
    /// scheme stamping is commit.rs's own pinned coverage.)
    #[test]
    fn wisdom_writers_stamp_embed_scheme() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);

        let fact = seed_fact(&conn, &entity_id, "A stamped user fact.");

        // UPDATE path (update_wisdom_with_blob): a pre-#265 raw blob+stamp is
        // replaced — blob NULLed (sweep re-embeds) AND scheme re-stamped.
        conn.execute(
            "UPDATE llm_wiki_entries
                SET embedding_blob = ?1, embed_scheme = 'raw'
              WHERE id = ?2",
            params![vec![7u8; 32], fact.id],
        )
        .unwrap();
        update_wisdom(&mut conn, &entity_id, &fact.id, "A rewritten body.").unwrap();
        let (blob2, scheme2): (Option<Vec<u8>>, String) = conn
            .query_row(
                "SELECT embedding_blob, embed_scheme FROM llm_wiki_entries WHERE id = ?1",
                [&fact.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(blob2, None);
        assert_eq!(scheme2, crate::embed_scheme::WRITE_SCHEME);
    }

    #[test]
    fn archive_wisdom_purges_edges_touching_the_fact() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        let fact = seed_fact(&conn, &entity_id, "The archived fact body.");
        let other = seed_fact(&conn, &entity_id, "The surviving fact body.");

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_out', ?1, ?2, ?3, 'related_to', 1757000000000)",
            params![entity_id, fact.id, other.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_in', ?1, ?2, ?3, 'related_to', 1757000000000)",
            params![entity_id, other.id, fact.id],
        )
        .unwrap();

        // R1 (remediation): the new heterogeneous contract only purges edges
        // whose partner is also dead in every endpoint table. Soft-delete
        // `other` so both seeded edges have dead partners and are
        // purgeable. Without this, both edges would survive because `other`
        // remains alive in `llm_wiki_entries`.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id = ?1",
            params![other.id],
        )
        .unwrap();

        archive_wisdom(&mut conn, &entity_id, &fact.id).unwrap();

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "both edges touching the archived fact must go"
        );
    }

    #[test]
    fn archive_wisdom_leaves_unrelated_edges_alone() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        let fact = seed_fact(&conn, &entity_id, "The archived fact body.");
        let b = seed_fact(&conn, &entity_id, "Fact B body.");
        let c = seed_fact(&conn, &entity_id, "Fact C body.");

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_bc', ?1, ?2, ?3, 'related_to', 1757000000000)",
            params![entity_id, b.id, c.id],
        )
        .unwrap();

        archive_wisdom(&mut conn, &entity_id, &fact.id).unwrap();

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "an edge between two live facts must survive");
    }

    /// Review fix (final review): a loser-keyed row updated through the
    /// survivor's redirect cluster must be REKEYED to the survivor — the row
    /// and the outbox payload have to agree (#132 prisma-outbox divergence
    /// class, same guard as `create_task_via_loser_keys_row_outbox_and_result_to_survivor`).
    #[test]
    fn update_wisdom_via_loser_rekeys_row_and_outbox_to_survivor() {
        let mut conn = open_in_memory().unwrap();
        let surv = make_entity(&mut conn);
        let loser = make_entity(&mut conn);
        let fact = seed_fact(&conn, &surv, "Pre-merge body.");
        // Simulate a pre-merge row: keyed to the entity that later lost the merge.
        conn.execute(
            "UPDATE llm_wiki_entries SET entity_id = ?1 WHERE id = ?2",
            params![loser, fact.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES (?1, ?2, 1)",
            params![loser, surv],
        )
        .unwrap();

        update_wisdom(&mut conn, &surv, &fact.id, "Post-merge body.").unwrap();

        let (stored_entity, payload): (String, String) = conn
            .query_row(
                "SELECT e.entity_id, o.payload FROM llm_wiki_entries e
                 JOIN llm_wiki_outbox o ON o.record_id = e.id
                 WHERE e.id = ?1 AND o.table_name = 'entries' AND o.operation = 'UPDATE'
                 ORDER BY o.id DESC LIMIT 1",
                [&fact.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_entity, surv, "the row is rekeyed to the survivor");
        let payload_json: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            payload_json["entity_id"].as_str().unwrap(),
            surv,
            "the replica payload is keyed to the survivor, never the loser"
        );
        assert_ne!(payload_json["entity_id"].as_str().unwrap(), loser);
    }

    /// Review fix (final review): archiving a loser-keyed row through the
    /// survivor must succeed (not bail "not found"), soft-delete it, and
    /// rekey it so the archived row and the DELETE payload agree.
    #[test]
    fn archive_wisdom_via_loser_soft_deletes_and_rekeys_to_survivor() {
        let mut conn = open_in_memory().unwrap();
        let surv = make_entity(&mut conn);
        let loser = make_entity(&mut conn);
        let fact = seed_fact(&conn, &surv, "Pre-merge body.");
        conn.execute(
            "UPDATE llm_wiki_entries SET entity_id = ?1 WHERE id = ?2",
            params![loser, fact.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES (?1, ?2, 1)",
            params![loser, surv],
        )
        .unwrap();

        archive_wisdom(&mut conn, &surv, &fact.id).unwrap();

        let (deleted_at, stored_entity, payload): (Option<i64>, String, String) = conn
            .query_row(
                "SELECT e.deleted_at, e.entity_id, o.payload FROM llm_wiki_entries e
                 JOIN llm_wiki_outbox o ON o.record_id = e.id
                 WHERE e.id = ?1 AND o.table_name = 'entries' AND o.operation = 'DELETE'
                 ORDER BY o.id DESC LIMIT 1",
                [&fact.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(deleted_at.is_some(), "the loser-keyed row is soft-deleted");
        assert_eq!(
            stored_entity, surv,
            "the archived row is rekeyed to the survivor"
        );
        let payload_json: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            payload_json["entity_id"].as_str().unwrap(),
            surv,
            "the DELETE payload is keyed to the survivor, never the loser"
        );
        assert_ne!(payload_json["entity_id"].as_str().unwrap(), loser);
    }

    // =========================================================================
    // Task 9 (user-confirmation-queue plan): the stated/ capture flow.
    // Spec §3.7 (Path B), §3.8 (ledger mirror), §9 (confirm with no backing
    // fact). The three pinned plan tests are here; supporting helpers keep to
    // this module.
    // =========================================================================

    /// Serializes every Task 9 test that pins CURATED_* process env.
    static TASK9_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Deterministic librarian stand-in: returns one canned synthesis
    /// response, so the kick's extraction leg runs the REAL pipeline
    /// (chunk → propose → auto-approve → resolve) without an LLM host.
    struct FixedCompleter {
        response: String,
    }

    impl crate::librarian::LlmCompleter for FixedCompleter {
        fn complete(&self, _system: &str, _user: &str) -> anyhow::Result<String> {
            Ok(self.response.clone())
        }
    }

    /// Temp vault + temp brain with a REAL on-disk app db (the kick opens its
    /// own connection by path). Returns the TempDir guard so the tree lives
    /// until the caller's frame ends.
    fn task9_dirs() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault = tmp.path().join("vault");
        let brain = tmp.path().join("brain");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&brain).unwrap();
        (tmp, vault, brain)
    }


    /// Open the temp brain the way the APP does: config.json beside the db
    /// carries the vault root, so the V22/V25 migration ladder actually
    /// stamps (a rootless open defers V22 and V25's deposit_kick_state +
    /// folder rules never land). Returns the raw Connection.
    fn task9_open_brain(brain: &std::path::Path, vault: &std::path::Path) -> Connection {
        std::fs::write(
            brain.join("config.json"),
            serde_json::json!({ "vault_path": vault.to_string_lossy() }).to_string(),
        )
        .unwrap();
        AppDb::open_with_config(&brain.join("brain.db"), brain.join("config.json"))
            .expect("open migrated temp brain db")
            .0
    }

    fn task9_with_env<T>(vault: &std::path::Path, brain: &std::path::Path, f: impl FnOnce() -> T) -> T {
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_DIR", Some(brain.to_str().unwrap())),
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
                ("CURATED_VAULT_ROOT", Some(vault.to_str().unwrap())),
                ("CURATED_EMBED_STUB", Some("constant8")),
            ],
            f,
        )
    }

    /// Seed an existing entity + one live fact so the canned proposal can
    /// target `existing_id` (no new-entity ontology minting in this path).
    fn task9_seed_entity_with_fact(conn: &Connection, entity_id: &str, fact_id: &str) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, 'Alpha', 'concept', 'Alpha summary', 100, 100)",
            [entity_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                source_type, created_at, updated_at)
             VALUES (?1, ?2, 'Fact', 'Existing fact body', '[]', 'inferred',
                'librarian_inferred', 100, 100)",
            params![fact_id, entity_id],
        )
        .unwrap();
    }

    fn task9_proposal_json(body: &str) -> String {
        serde_json::json!({
            "proposals": [{
                "target": { "existing_id": "ent-alpha" },
                "reasoning": "Stated capture.",
                "summary_update": null,
                "facts": [{
                    "op": "add",
                    "body": body,
                    "tags": [],
                    "confidence": "inferred",
                    "evidence": ["C1"]
                }],
                "edges": [],
                "tasks": []
            }]
        })
        .to_string()
    }

    fn task9_fact_count(conn: &Connection, source_type: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM llm_wiki_entries
              WHERE source_type = ?1 AND deleted_at IS NULL",
            [source_type],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Drive the WATCHER-side ingest over a `stated/` file — the app regime:
    /// the watcher holds the vault lock for its lifetime, so the capture's
    /// synchronous kick cannot ingest (queued_watcher fallback) and the
    /// watcher stages + ingests the file, keying `documents.path` in the
    /// virtual-absolute form (issue #204, defect B1). Shared by the B1
    /// regression test and the task9 plan tests. Returns the document key.
    fn task9_watch_ingest(
        vault: &std::path::Path,
        conn: &mut Connection,
        profile: &EmbedProfile,
        rel: &str,
    ) -> String {
        let abs = vault.join(rel);
        crate::db::queue::enqueue_vault_event(
            conn,
            notify::EventKind::Create(notify::event::CreateKind::Any),
            &abs,
            Some(vault),
        )
        .unwrap();
        let doc_path = crate::db::queue::virtual_path_key(&abs).unwrap();
        // Folder rules match by literal ancestor prefix, so the watcher-keyed
        // lane carries its own absolute rule — the same synthesize +
        // auto-approve intent V25 seeds for the kick's rel-keyed lane
        // ('stated').
        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve)
             VALUES (?1, 'synthesize', 1)",
            [vault.join("stated").to_string_lossy().as_ref()],
        )
        .unwrap();
        crate::pipeline::ingest_document_virtual(
            conn,
            profile,
            &doc_path,
            &abs.to_string_lossy(),
            false,
            Some(vault.to_string_lossy().as_ref()),
        )
        .unwrap();
        doc_path
    }

    /// The librarian + commit leg over the watcher-keyed document: the
    /// `synthesize` folder rule auto-approves, and the auto-approve resolve
    /// runs the Path B predicate (all_sources_attested).
    fn task9_librarian_leg(conn: &mut Connection, doc_path: &str, completer: &FixedCompleter) {
        let model = crate::librarian::active_generation_model(crate::setup::recommended_model());
        crate::librarian::generate_summary_with(conn, doc_path, &model, false, completer).unwrap();
    }

    /// Pinned plan test 1: capture renders `stated/kurt/<year>/<slug>.md`,
    /// writes the attestation row + `.attestations.jsonl` line (content-hash
    /// keyed), registers the kick ledger row, and the fact the librarian
    /// commits is `user_stated` with evidence back to the stated file (spec
    /// §3.7.4-5: instant confirm, no queue step).
    ///
    /// B1 adaptation (driver only — every §3.7 assertion kept): the test now
    /// runs under the APP's lock regime. The watcher holds the vault lock for
    /// its lifetime, so the synchronous kick cannot ingest (the ledger
    /// honestly records `queued_watcher`) and the watcher-side pipeline does
    /// the ingest, keying `documents.path` virtual-absolute. Pre-B1 this test
    /// ran the kick with NO lock holder — a scenario that never occurs in the
    /// running app — and the rel-keyed ingest masked the key-form defect.
    #[test]
    fn capture_statement_writes_attested_file_and_kicks() {
        let _env = TASK9_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (_tmp, vault, brain) = task9_dirs();
        task9_with_env(&vault, &brain, || {
            // The brain db opens UNDER the env: V22 needs the vault root
            // resolvable at open or the migration defers and V25's
            // deposit_kick_state never lands.
            let db_path = brain.join("brain.db");
            let mut conn = task9_open_brain(&brain, &vault);
            task9_seed_entity_with_fact(&conn, "ent-alpha", "fact-alpha");
            let completer = FixedCompleter {
                response: task9_proposal_json("Alpha renews the Baxter lease on March 1."),
            };
            let ctx = super::CaptureContext {
                db_path: db_path.clone(),
                vault_root: vault.clone(),
                profile: EmbedProfile::default(),
            };

            // Names the seeded entity: the librarian's candidate matching is
            // name-based (name_match_entities), and the mock proposal targets
            // that entity.
            let text = "Alpha's Baxter warehouse lease renews on March 1.";
            // App regime: the watcher owns the vault lock while the capture
            // runs — the synchronous kick cannot ingest.
            let lock = crate::watcher::VaultLock::acquire(&brain).unwrap();
            let rel = super::capture_statement_with(&ctx, text, Some(&completer)).unwrap();
            drop(lock);

            // 1. The file exists under stated/kurt/<year>/.
            let year = chrono::Utc::now().format("%Y");
            assert!(
                rel.starts_with(&format!("stated/kurt/{year}/")) && rel.ends_with(".md"),
                "rel path must be stated/kurt/<year>/<slug>.md, got: {rel}"
            );
            let full = vault.join(&rel);
            assert!(full.is_file(), "stated file must exist at {full:?}");
            let rendered = std::fs::read_to_string(&full).unwrap();
            assert!(
                rendered.contains(text),
                "the statement text is the file body"
            );

            // 2. The attestation of record: brain row content-hash keyed to the
            //    SAME hash routine the documents table uses (spec §3.7.2).
            //    The TABLE KEY is the watcher's virtual-absolute documents.path
            //    form (defect B1); the readback below mirrors the exact helper.
            let attestation_key = crate::db::queue::virtual_path_key(&vault.join(&rel)).unwrap();
            let row_hash: String = conn
                .query_row(
                    "SELECT content_hash FROM stated_attestations WHERE document_path = ?1",
                    [&attestation_key],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                row_hash,
                crate::hasher::hash_bytes(&std::fs::read(&full).unwrap()),
                "attestation hash must match the file bytes exactly"
            );
            let (attested_by, attested_at): (String, String) = conn
                .query_row(
                    "SELECT attested_by, attested_at FROM stated_attestations
                      WHERE document_path = ?1",
                    [&attestation_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(attested_by, "human:kurt");
            assert!(!attested_at.is_empty());

            // 3. The vault ledger mirror: exactly one line for this file.
            let ledger = std::fs::read_to_string(vault.join("stated/.attestations.jsonl")).unwrap();
            let lines: Vec<&str> = ledger.lines().filter(|l| !l.trim().is_empty()).collect();
            assert_eq!(lines.len(), 1, "one attestation line, not {lines:?}");
            let line: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
            assert_eq!(line["document_path"].as_str().unwrap(), rel);
            assert_eq!(line["content_hash"].as_str().unwrap(), row_hash);

            // 4. The app registered the kick (spec §3.7.3: a NON-DEPOSIT kick
            //    path — the kick ledger row is written by the APP). Under the
            //    app's lock regime the synchronous kick cannot ingest, so the
            //    ledger honestly records the handoff to the watcher.
            let state: String = conn
                .query_row(
                    "SELECT state FROM deposit_kick_state WHERE path = ?1",
                    [&rel],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                state, "queued_watcher",
                "the kick must record the watcher handoff"
            );

            // Watcher-side ingest + librarian commit over the watcher-keyed
            // document (defect B1's key-form contract).
            let doc_path = task9_watch_ingest(&vault, &mut conn, &ctx.profile, &rel);
            task9_librarian_leg(&mut conn, &doc_path, &completer);

            // 5. Post-kick: the committed fact is user_stated WITH evidence
            //    pointing back at the stated file.
            assert_eq!(
                task9_fact_count(&conn, "user_stated"),
                1,
                "the librarian's fact lands human-tier (instant confirm)"
            );
            let (fact_id, body): (String, String) = conn
                .query_row(
                    "SELECT id, body FROM llm_wiki_entries
                      WHERE source_type = 'user_stated' AND deleted_at IS NULL",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(body, "Alpha renews the Baxter lease on March 1.");
            let evidence_rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM librarian_evidence le
                      JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
                      JOIN documents d ON d.id = ps.doc_id
                     WHERE le.entry_id = ?1 AND d.path = ?2",
                    params![fact_id, doc_path],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                evidence_rows > 0,
                "the fact's evidence must trace to the stated/ source document"
            );
        });
    }

    /// Pinned plan test 2 (Rule 15 / spec §9): a fresh statement typed into
    /// the queue view — confirm-with-no-backing-fact — routes through
    /// capture_statement, so the fact is created the only pipeline-consistent
    /// way (stated/ file + attest + kick), never a direct insert.
    #[test]
    fn fresh_statement_via_queue_creates_backing_fact() {
        let _env = TASK9_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (_tmp, vault, brain) = task9_dirs();
        task9_with_env(&vault, &brain, || {
            // The brain db opens UNDER the env: V22 needs the vault root
            // resolvable at open or the migration defers and V25's
            // deposit_kick_state never lands.
            let db_path = brain.join("brain.db");
            let mut conn = task9_open_brain(&brain, &vault);
            task9_seed_entity_with_fact(&conn, "ent-alpha", "fact-alpha");
            let completer = FixedCompleter {
                response: task9_proposal_json("Alpha ships the release on Friday."),
            };
            let ctx = super::CaptureContext {
                db_path: db_path.clone(),
                vault_root: vault.clone(),
                profile: EmbedProfile::default(),
            };

            // No fact id: the queue's confirm-with-no-fact path. App lock
            // regime: the watcher owns the vault lock while the capture runs
            // (B1 adaptation — the pre-B1 driver ran the kick with no lock
            // holder, masking the key-form defect).
            let lock = crate::watcher::VaultLock::acquire(&brain).unwrap();
            let super::QueueStatementOutcome::Captured(rel) =
                super::confirm_or_capture_with(
                    &ctx,
                    None,
                    Some("Alpha ships the release on Friday."),
                    Some(&completer),
                )
                .unwrap()
            else {
                panic!("no backing fact ⇒ capture, not confirm");
            };
            drop(lock);
            assert!(
                vault.join(&rel).is_file(),
                "a stated/ file backs the fresh statement: {rel}"
            );

            // Watcher-side ingest + librarian commit over the watcher-keyed
            // document (defect B1's key-form contract).
            let doc_path = task9_watch_ingest(&vault, &mut conn, &ctx.profile, &rel);
            task9_librarian_leg(&mut conn, &doc_path, &completer);

            assert_eq!(
                task9_fact_count(&conn, "user_stated"),
                1,
                "the fresh statement's backing fact exists after the kick"
            );
            assert_eq!(
                task9_fact_count(&conn, "librarian_inferred"),
                1,
                "the pre-seeded entity fact is untouched"
            );
        });
    }

    /// RED (B1 regression): the APP's capture regime. The watcher holds the
    /// vault lock for its lifetime, so the capture's synchronous kick cannot
    /// ingest — it falls back to `queued_watcher` and the WATCHER stages and
    /// ingests the stated file, keying `documents.path` in the VIRTUAL
    /// ABSOLUTE form (issue #204 contract). The attestation row must carry
    /// the SAME key form, or the Path B provenance arm (`all_sources_attested`
    /// joining `a.document_path = d.path`) never matches and the §3.7
    /// "instant confirm" contract lands `librarian_inferred`.
    #[test]
    fn capture_through_watcher_ingest_stamps_user_stated() {
        let _env = TASK9_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (_tmp, vault, brain) = task9_dirs();
        task9_with_env(&vault, &brain, || {
            let db_path = brain.join("brain.db");
            let mut conn = task9_open_brain(&brain, &vault);
            task9_seed_entity_with_fact(&conn, "ent-alpha", "fact-alpha");
            let completer = FixedCompleter {
                response: task9_proposal_json("Alpha renews the Baxter lease on March 1."),
            };
            let ctx = super::CaptureContext {
                db_path: db_path.clone(),
                vault_root: vault.clone(),
                profile: EmbedProfile::default(),
            };
            let text = "Alpha's Baxter warehouse lease renews on March 1.";

            // App regime: hold the vault lock so the synchronous kick cannot
            // ingest (queued_watcher fallback — wisdom_deposit.rs:484-491).
            let lock = crate::watcher::VaultLock::acquire(&brain).unwrap();
            let rel = super::capture_statement_with(&ctx, text, Some(&completer)).unwrap();
            drop(lock);

            // Watcher leg, driven exactly the way the `enqueue_vault_event`
            // tests do: stage the file event, then ingest the staged row. The
            // watcher keys `documents.path` by `std::path::absolute` of the
            // file (queue.rs:93, issue #204) — NOT the vault-relative rel.
            let abs = vault.join(&rel);
            let abs_key = std::path::absolute(&abs)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            crate::db::queue::enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::Any),
                &abs,
                Some(&vault),
            )
            .unwrap();
            let doc_path: String = conn
                .query_row(
                    "SELECT path FROM documents WHERE path = ?1",
                    [&abs_key],
                    |r| r.get(0),
                )
                .expect("the watcher staged the stated file under its absolute key form");
            crate::pipeline::ingest_document_virtual(
                &conn,
                &ctx.profile,
                &doc_path,
                &abs.to_string_lossy(),
                false,
                Some(vault.to_string_lossy().as_ref()),
            )
            .unwrap();

            // Librarian + commit over the WATCHER-keyed document. Folder rules
            // match by literal ancestor prefix, so the watcher-keyed lane is
            // configured with its own absolute rule — the same synthesize +
            // auto-approve intent V25 seeds for the kick's rel-keyed lane
            // ('stated'). The auto-approve resolve runs the Path B predicate.
            conn.execute(
                "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve)
                 VALUES (?1, 'synthesize', 1)",
                [vault.join("stated").to_string_lossy().as_ref()],
            )
            .unwrap();
            let model =
                crate::librarian::active_generation_model(crate::setup::recommended_model());
            crate::librarian::generate_summary_with(
                &mut conn, &doc_path, &model, false, &completer,
            )
            .unwrap();

            // The B1 contract: the attestation row and the documents row must
            // share the SAME key form so the Path B arm joins.
            let att_path: String = conn
                .query_row("SELECT document_path FROM stated_attestations", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(
                att_path, doc_path,
                "RED (B1): the attestation is keyed vault-relative while the watcher keyed \
                 documents.path absolute — the Path B join can never match"
            );
            let source_type: String = conn
                .query_row(
                    "SELECT source_type FROM llm_wiki_entries
                      WHERE body = ?1 AND deleted_at IS NULL",
                    ["Alpha renews the Baxter lease on March 1."],
                    |r| r.get(0),
                )
                .expect("the stated fact committed");
            assert_eq!(
                source_type, "user_stated",
                "RED (B1): the join miss lands the capture librarian_inferred, breaking \
                 §3.7 instant confirm"
            );
        });
    }

    /// Pinned plan test 3: the app's direct `add_wisdom_in_tx` write is
    /// RETIRED (spec §3.7.6 / §2 rejected alternative). Compile-level proof:
    /// the function no longer exists (this file would not compile if it did);
    /// the grep guard pins non-reintroduction on the whole retired surface.
    #[test]
    fn direct_write_is_gone() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for rel_file in ["src/db/wisdom.rs", "src/entities_api.rs", "src/db/mod.rs"] {
            let text = std::fs::read_to_string(manifest.join(rel_file)).unwrap();
            // Needle built by concatenation so the guard does not trip over
            // its own source text. The "fn " anchor pins the DEFINITION (and
            // call sites), never this test's documentation of the retirement.
            let needle = format!("fn add_{}", "wisdom_in_tx");
            assert!(
                !text.contains(&needle),
                "{rel_file} still defines or references the retired direct write"
            );
        }
    }

    // ---- Task 9 supporting tests (capture-path behavior beyond the pins) --

    /// Empty/whitespace statements are rejected BEFORE anything is written —
    /// no file, no attestation row, no ledger line, no kick row.
    #[test]
    fn capture_statement_rejects_empty_text() {
        let _env = TASK9_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (_tmp, vault, brain) = task9_dirs();
        task9_with_env(&vault, &brain, || {
            // The brain db opens UNDER the env: V22 needs the vault root
            // resolvable at open or the migration defers and V25's
            // deposit_kick_state never lands.
            let db_path = brain.join("brain.db");
            let conn = task9_open_brain(&brain, &vault);
            let ctx = super::CaptureContext {
                db_path: db_path.clone(),
                vault_root: vault.clone(),
                profile: EmbedProfile::default(),
            };
            assert!(super::capture_statement_with(&ctx, "   ", None).is_err());
            assert!(
                !vault.join("stated").exists(),
                "no stated tree is created for an empty statement"
            );
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM stated_attestations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(rows, 0, "no attestation row for an empty statement");
        });
    }

    /// Two statements with the same opening words never collide: the second
    /// capture gets a `-2` suffix — a statement file is never overwritten.
    #[test]
    fn capture_statement_dedupes_duplicate_slugs() {
        let _env = TASK9_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (_tmp, vault, brain) = task9_dirs();
        task9_with_env(&vault, &brain, || {
            // The brain db opens UNDER the env: V22 needs the vault root
            // resolvable at open or the migration defers and V25's
            // deposit_kick_state never lands.
            let db_path = brain.join("brain.db");
            let conn = task9_open_brain(&brain, &vault);
            let ctx = super::CaptureContext {
                db_path: db_path.clone(),
                vault_root: vault.clone(),
                profile: EmbedProfile::default(),
            };
            let text = "The warehouse lease renews on March 1, 2027 per the landlord notice!";
            let rel1 = super::capture_statement_with(&ctx, text, None).unwrap();
            let rel2 = super::capture_statement_with(&ctx, text, None).unwrap();
            assert_ne!(rel1, rel2, "same slug twice must dedupe, not overwrite");
            assert!(rel2.trim_end_matches(".md").ends_with("-2"));
            assert!(vault.join(&rel1).is_file());
            assert!(vault.join(&rel2).is_file());
            // No generation host in this env: the kicks recorded `chunked`
            // honestly instead of pretending the librarian ran.
            let state: String = conn
                .query_row(
                    "SELECT state FROM deposit_kick_state WHERE path = ?1",
                    [&rel1],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(state, "chunked");
        });
    }

    /// The stated file's frontmatter records WHO said it, HOW it was
    /// captured, and the human-readable attestation note (spec §3.7.1);
    /// the statement text itself stays in the BODY.
    #[test]
    fn capture_statement_renders_speaker_capture_attestation_frontmatter() {
        let _env = TASK9_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (_tmp, vault, brain) = task9_dirs();
        let db_path = brain.join("brain.db");
        let _conn = task9_open_brain(&brain, &vault);
        task9_with_env(&vault, &brain, || {
            let ctx = super::CaptureContext {
                db_path: db_path.clone(),
                vault_root: vault.clone(),
                profile: EmbedProfile::default(),
            };
            let text = "Alpha is the preferred vendor for print work.";
            let rel = super::capture_statement_with(&ctx, text, None).unwrap();
            let rendered = std::fs::read_to_string(vault.join(&rel)).unwrap();
            assert!(rendered.starts_with("---\n"), "frontmatter opens the file");
            assert!(rendered.contains("attributed_speaker: kurt"));
            assert!(rendered.contains("capture_method: app-statement"));
            assert!(
                rendered.contains("attestation: \"human:kurt attested this statement"),
                "a human-readable attestation note is in the frontmatter"
            );
            assert!(
                rendered.trim_end().ends_with(text),
                "the statement text is the body, untouched"
            );
        });
    }
}
