//! Vault event queue: stage file events into the documents table.
//!
//! Spec: docs/superpowers/specs/2026-08-25-ct-headless-cli-phase2-watch.md §6
//!
//! 4-stage path hardening:
//!   1. std::path::absolute (resolve symlinks + .. components)
//!   2. std::fs::canonicalize (resolve filesystem-level symlinks; fallback to absolute)
//!   3. Vault-root guard (explicit `vault_root` argument, falling back to
//!      `CURATED_VAULT_ROOT`; reject paths outside it)
//!      3b. Extension gate (`should_ingest_extension`, issue #203); Deletes bypass it
//!   4. sha256 the bytes; upsert documents row with status='pending'
//!
//! For Delete events: skip step 4 (file is gone); delete the documents row via
//! `queries::delete_document`, which first records pending-proposal provenance.
//! chunks cascade-delete via FK ON DELETE CASCADE.
//!
//! Path stored in `documents.path` is the VIRTUAL form (`abs`, pre-
//! canonicalize) — issue #204 unifies the column on the virtual form so the
//! watcher's row matches the walker's (reconcile.rs:74-77 documents the
//! column as virtual). The canonical form is still computed for the
//! containment check (symlink targets can canonicalize outside the vault)
//! and for `std::fs::read` (the bytes live at the canonical location).
//! Pre-#204, the watcher stored canonical, which diverged from the walker
//! under approved `documents/` trusted links and created phantom rows.

use std::path::{Path, PathBuf};

use anyhow::Result;
use notify::EventKind;
use rusqlite::{params, Connection, OptionalExtension};

/// Stage a vault file event into the documents table.
///
/// `conn` is a mutable SQLite connection (caller manages lifecycle — ephemeral
/// per call from the desktop watcher callback to avoid holding DbState mutex
/// during the sha256 hashing; long-lived from `ct watch`). WAL mode allows
/// concurrent writer + readers; see src-tauri/src/db/connection.rs:130.
pub fn enqueue_vault_event(
    conn: &mut Connection,
    event_kind: notify::EventKind,
    raw_path: &Path,
    vault_root: Option<&Path>,
) -> Result<()> {
    // Spec D3 — resolution order: explicit argument -> CURATED_VAULT_ROOT
    // -> None. The desktop app never sets the env var, which is precisely
    // the configuration where the immortal-pending bug was reported, so a
    // gate that relied on the env var alone would not fix it.
    let configured_root: Option<PathBuf> = match vault_root {
        Some(p) => Some(p.to_path_buf()),
        None => std::env::var_os("CURATED_VAULT_ROOT").map(PathBuf::from),
    };

    // Canonicalize the vault root the same way as the event path so that
    // symlinked / non-canonical vault roots (e.g. macOS /var → /private/var)
    // can't bypass the containment check. Without this, an event for
    // `/var/vault/note.md` would fail `starts_with("/var/vault")` against a
    // canonicalized `/private/var/vault/note.md` and be silently dropped.
    let containment_root: Option<PathBuf> = configured_root
        .as_ref()
        .map(|p| std::path::absolute(p).map(|abs| std::fs::canonicalize(&abs).unwrap_or(abs)))
        .transpose()?;

    let abs = std::path::absolute(raw_path)?;
    let canonical = std::fs::canonicalize(&abs).unwrap_or(abs.clone());

    if let Some(vr) = &containment_root {
        // Trusted-link carve-out: a symlink in the vault can resolve (via
        // canonicalize) to a path outside the canonical vault root, so the
        // canonical-only containment check would drop the event before the
        // gate ever runs. Accepting the VIRTUAL path when it lies under the
        // as-configured root lets trusted-link files stage. Truly external
        // paths still fail both checks.
        let abs_in_vault = configured_root
            .as_deref()
            .is_some_and(|vroot| abs.starts_with(vroot));
        if !canonical.starts_with(vr) && !abs_in_vault {
            eprintln!(
                "[watch] skipping out-of-vault path: {}",
                canonical.display()
            );
            return Ok(());
        }
    }

    // Issue #204: store the VIRTUAL path (`abs`), matching what the walker
    // writes (tools/src/cmds.rs:217) and what reconcile.rs:74-77 documents
    // as the column's contract. Pre-fix, this wrote `canonical`, which
    // diverged from the walker under approved `documents/` trusted links
    // (e.g. a Modify under `documents/specs` → `<external>/target/` staged
    // a row keyed by the external canonical path and every reconcile pass
    // then saw that row as vanished). `canonical` is still computed above
    // for the containment check (above) and for `std::fs::read` below.
    let path_str = abs.to_string_lossy().into_owned();

    // Deletes must run BEFORE the exclusion check: a `documents` row staged
    // by pre-filter code (or another tool) must still be deletable when the
    // scratch file's Remove event arrives. Exclusion only gates STAGING new
    // rows, not healing old ones.
    if matches!(event_kind, EventKind::Remove(_)) {
        // Checked transaction: `conn` is `&mut` (issue #211 spec D5).
        let tx = conn.transaction()?;
        crate::db::queries::delete_document(&tx, &path_str)?;
        tx.commit()?;
        return Ok(());
    }

    // Issue #203: gate STAGING by extension, mirroring the walker
    // (`collect_files`) and the pipeline's own early-return. A row for a
    // non-ingestable file could never leave `pending`, so the supervisor
    // sweep would re-enqueue it forever. Remove events above deliberately
    // bypass this gate: a row staged before the gate existed must still be
    // deletable when its file goes away. Read from `abs` (virtual path),
    // the same path the walker and pipeline take the extension from.
    let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !crate::chunker::should_ingest_extension(ext) {
        return Ok(());
    }

    // The walker has always filtered these; the watcher never did. An editor
    // scratch file that exists for milliseconds would get a `documents` row
    // that outlives it, and the supervisor sweep then re-enqueues that row on
    // every pass, forever (spec §3).
    if crate::walk_vault::is_excluded_file(&canonical) {
        return Ok(());
    }

    // Excluded working directories (`.brain`, `node_modules`, `target`, …).
    //
    // Matched on `abs` — the VIRTUAL, pre-canonicalize path — never on
    // `canonical` (spec D2). Canonicalizing rewrites the path in both
    // wrong directions: it STRIPS `.brain` when the directory is symlinked
    // out (the bug would recur for that subtree) and INJECTS `.brain` for
    // an approved trusted link whose target lives under one (events for a
    // directory the walker happily ingests would be rejected).
    //
    // Relativized first (spec D1): EXCLUDED_DIRS names occur in ordinary
    // ancestors, so an absolute match would kill live-sync for any vault
    // under e.g. /var/build/wiki. Unrelativizable paths fail open (D2b).
    match configured_root.as_deref() {
        Some(root) => {
            if crate::walk_vault::abs_path_is_excluded_in_vault(&abs, root) {
                return Ok(());
            }
        }
        None => warn_no_vault_root_once(),
    }

    // Add / Modify: hash, upsert.
    //
    // A file that vanished between the event firing and this read is not an
    // error -- it is a delete that arrived out of order. Propagating an error
    // here left the staged row in place with nothing behind it. Every other
    // IO error still propagates (spec §3).
    let bytes = match std::fs::read(&canonical) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let tx = conn.transaction()?;
            crate::db::queries::delete_document(&tx, &path_str)?;
            tx.commit()?;
            return Ok(());
        }
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("read {}", canonical.display())));
        }
    };
    let hash = sha256_hex(&bytes);

    conn.execute(
        "INSERT INTO documents (path, hash, tier, status) \
         VALUES (?1, ?2, 'user_doc', 'pending') \
         ON CONFLICT(path) DO UPDATE SET \
            hash = excluded.hash, \
            status = 'pending' \
         WHERE documents.hash != excluded.hash \
            OR documents.status IN ('pending','error','orphaned')",
        rusqlite::params![&path_str, &hash],
    )?;
    Ok(())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// One-time notice that the exclusion gate is inert because no vault root
/// could be resolved (spec D3). Skipping matches today's behavior for the
/// containment check on the same path; logging once makes the condition
/// diagnosable rather than invisible, without spamming a per-event log.
fn warn_no_vault_root_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        eprintln!(
            "[watch] no vault root (argument or CURATED_VAULT_ROOT); \
             excluded-directory gate is inactive for this process"
        );
    });
}

// ---------------------------------------------------------------------------
// Confirmation queue (user-confirmation-queue plan Task 4; spec rev 4.1 §3.2)
//
// The queue is a QUERY, not a table. Membership: (a) live — deleted_at IS NULL
// AND lifecycle_status='stable' AND not superseded/expired per the EXISTING
// temporal filters (same probe + fragment semantics as
// wisdom_match::entry_eligibility_where); (b) provenance NOT human-tier —
// `source_type IS NULL OR source_type NOT IN ('user_stated','user_confirmed')`
// (NULL/out-of-vocab counts as unverified; delivery, by contrast, admits only
// exact vocabulary values); (c) not deny-suppressed by a queue_denials row
// keyed (document path, chunk hash, fact-text hash) — sibling-safe, because a
// sibling extracted from the same chunk has a DIFFERENT fact-text hash.
// ---------------------------------------------------------------------------

/// One row of the human confirmation queue (`curated_queue_list`, read-only —
/// NO mutate tool: Task 5 owns confirm/deny and exposes them frontend-only).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueueRow {
    pub fact_id: String,
    pub title: String,
    pub body: String,
    /// `provenance_for(source_type)` — verbatim vocabulary value, `None`
    /// outside the vocab (wisdom_match.rs).
    pub provenance: Option<String>,
    pub entity_id: String,
    /// First stored evidence quote, via the `deposit_status_row` join chain
    /// (librarian_evidence → curated_proposal_sources → documents).
    pub evidence_quote: Option<String>,
    pub source_document: Option<String>,
    /// Deposit attribution speaker (Task 1's `deposit_attribution` table),
    /// joined on the source document path. Display only.
    pub attributed_speaker: Option<String>,
    /// `approval_instruction_source` was DROPPED (spec rev 4.1 §6 — no
    /// carrier exists); the field stays in the row shape for the frontend
    /// contract and is always `None` until a carrier lands.
    pub instruction_source: Option<String>,
    /// Whole days since `created_at` (aging indicator; floor).
    pub age_days: i64,
}

/// Normalized fact-text hash — the third `queue_denials` key component. The
/// chunk hash is stable across re-extraction, so the fact-text hash pins the
/// SPECIFIC fact: a verbatim re-extraction stays suppressed, while a sibling
/// extracted from the same chunk (different text) keeps its vouch opportunity
/// (spec rev 4.1 §3.3, rev-4 M3). Normalization: whitespace-collapsed +
/// lowercased title, one newline, same-normalized body, SHA-256 hex.
///
/// Task 5's `deny_fact` MUST write denials with THIS function so the queue
/// anti-match and the ledger agree.
pub fn fact_text_hash(title: &str, body: &str) -> String {
    fn normalize(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
    }
    let mut bytes = normalize(title).into_bytes();
    bytes.push(b'\n');
    bytes.extend_from_slice(normalize(body).as_bytes());
    sha256_hex(&bytes)
}

/// The confirmation queue: live, non-human-tier, non-denied facts, oldest
/// first (`ORDER BY age DESC`). Infallible by contract (the plan pins
/// `-> Vec<QueueRow>`): a query failure degrades to an empty queue with a
/// logged error — a broken queue read must never take down a caller.
pub fn queue_rows(conn: &Connection) -> Vec<QueueRow> {
    match queue_rows_inner(conn) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[queue] confirmation queue query failed: {e:#}");
            Vec::new()
        }
    }
}

fn queue_rows_inner(conn: &Connection) -> anyhow::Result<Vec<QueueRow>> {
    // Supersede/expiry: reuse the EXISTING temporal filters. `temporal_columns`
    // is wisdom_match's ONE column probe; the two fragments below are the exact
    // eligibility predicate `entry_eligibility_where` applies (future-stamped
    // supersessions stay eligible). The embedding/scheme legs of that fragment
    // are DELIVERY gates, not membership gates — the queue must surface a fact
    // whose embedding failed, so they are deliberately not carried over (spec
    // §3.2: membership is live + non-human-tier + not denied, nothing else).
    let t = crate::wisdom_match::temporal_columns(conn, None)?;
    let now = crate::wisdom_deposit::now_ms();
    let mut where_sql = String::from(
        "e.deleted_at IS NULL \
         AND e.lifecycle_status = 'stable' \
         AND (e.source_type IS NULL OR e.source_type NOT IN ('user_stated','user_confirmed'))",
    );
    if t.has_superseded_by {
        where_sql.push_str(" AND (e.superseded_by IS NULL OR e.superseded_at > :now)");
    }
    if t.has_valid_to {
        where_sql.push_str(" AND (e.valid_to IS NULL OR e.valid_to > :now)");
    }

    // Evidence quote via the `deposit_status_row` join chain (the trace entry →
    // librarian_evidence → proposal → curated_proposal_sources → documents
    // path), attribution joined on the source document path.
    let sql = format!(
        "SELECT e.id, e.title, e.body, e.source_type, e.entity_id, e.created_at, \
                le.evidence_json, d.path, da.speaker \
           FROM llm_wiki_entries e \
           LEFT JOIN librarian_evidence le ON le.entry_id = e.id \
           LEFT JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id \
           LEFT JOIN documents d ON d.id = ps.doc_id \
           LEFT JOIN deposit_attribution da ON da.document_path = d.path \
          WHERE {where_sql} \
          ORDER BY e.created_at ASC"
    );

    // Denial anti-match: the plan's "LEFT JOIN anti-match" against
    // queue_denials. The key's third component is a SHA-256 computed in Rust
    // (SQLite ships no hash function), so the triple match cannot execute
    // inside the statement; the (document_path, chunk_hash, fact_text_hash)
    // triple is enforced per row below — same semantics, sibling-safe.
    let mut denied: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT document_path, chunk_hash, fact_text_hash FROM queue_denials",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (p, c, f) = row?;
            denied.insert((p, c, f));
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::named_params! {":now": now}, |r| {
        let fact_id: String = r.get(0)?;
        let title: String = r.get(1)?;
        let body: String = r.get(2)?;
        let source_type: Option<String> = r.get(3)?;
        let entity_id: String = r.get(4)?;
        let created_at: i64 = r.get(5)?;
        let evidence_json: Option<String> = r.get(6)?;
        let doc_path: Option<String> = r.get(7)?;
        let speaker: Option<String> = r.get(8)?;

        // evidence_json shape is pinned by `evidence_json_with_hashes`
        // (commit.rs): {"proposal_id", "evidence":[{chunk_id, content_hash,
        // quote, ...}]}. First entry wins; a malformed blob degrades to None.
        let (quote, chunk_hash) = evidence_json
            .as_deref()
            .and_then(|ej| serde_json::from_str::<serde_json::Value>(ej).ok())
            .map(|v| {
                (
                    v["evidence"][0]["quote"].as_str().map(str::to_string),
                    v["evidence"][0]["content_hash"].as_str().map(str::to_string),
                )
            })
            .unwrap_or((None, None));

        Ok((
            fact_id,
            title,
            body,
            source_type,
            entity_id,
            created_at,
            quote,
            doc_path,
            chunk_hash,
            speaker,
        ))
    })?;

    let mut out: Vec<QueueRow> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        let (
            fact_id,
            title,
            body,
            source_type,
            entity_id,
            created_at,
            quote,
            doc_path,
            chunk_hash,
            speaker,
        ) = row?;
        // Join fan-out guard: a fact with several evidence rows / source docs
        // must appear once (first row wins — the quote is illustrative, not
        // exhaustive).
        if !seen.insert(fact_id.clone()) {
            continue;
        }
        // Sibling-safe denial check: only an exact triple suppresses.
        if let (Some(doc), Some(chunk)) = (&doc_path, &chunk_hash) {
            let key = (doc.clone(), chunk.clone(), fact_text_hash(&title, &body));
            if denied.contains(&key) {
                continue;
            }
        }
        out.push(QueueRow {
            age_days: (now - created_at).div_euclid(86_400_000),
            provenance: crate::wisdom_match::provenance_for(source_type.as_deref()),
            fact_id,
            title,
            body,
            entity_id,
            evidence_quote: quote,
            source_document: doc_path,
            attributed_speaker: speaker,
            instruction_source: None,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Confirm / deny / undeny actions (user-confirmation-queue plan Task 5; spec
// rev 4.1 §3.3 + §8). FRONTEND-ONLY exposure: the Tauri command layer (Task 9)
// is the only caller. NO MCP tool, NO CLI wrapper, NO dispatch registration —
// D2 closes the trust hole by leaving every non-app surface at agent tier.
// ---------------------------------------------------------------------------

/// The actor every app confirm/deny stamps (spec §3 pins `human:kurt`).
pub const HUMAN_CONFIRMER: &str = "human:kurt";

/// The denial key: (source document path, evidence chunk hash, normalized
/// fact-text hash). The `queue_denials` primary key and the
/// `stated/.denials.jsonl` match key — the chunk hash is stable across
/// re-extraction while the fact-text hash pins the SPECIFIC fact, so denying
/// one fact never hides a sibling from the same chunk (rev-4 M3, pinned by
/// `deny_suppresses_and_undeny_restores`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenialKey {
    pub document_path: String,
    pub chunk_hash: String,
    pub fact_text_hash: String,
}

/// Confirm a queue fact (spec §3.3). One UPDATE in the `drafts.rs:62-74`
/// json_insert shape — MINUS the `lifecycle_status` line, which confirm must
/// not touch — plus an outbox Update row (Rule 5 sync), an event, and the
/// `stated/.confirmations.jsonl` append via Task 2's `record_confirmation`
/// primitive (REUSED, not forked: it owns the ledger append and its
/// hash-keyed `source_type` upsert; `confidence`, the `okf_verified` entry,
/// the `last_verified_by/at` mirror and the outbox/event legs are not covered
/// by that primitive and live in the stamp below — flagged in the Task 5
/// report rather than forked silently). Idempotent on re-confirm: no
/// duplicate event, no duplicate ledger line (spec §4 double-confirm).
pub fn confirm_fact(conn: &Connection, fact_id: &str) -> Result<()> {
    let vault_root = crate::db::stated_ledger::resolve_vault_root()?;
    confirm_fact_at(conn, &vault_root, fact_id)
}

/// Explicit-root form of [`confirm_fact`] (tests and callers that already
/// hold the vault root).
pub fn confirm_fact_at(conn: &Connection, vault_root: &Path, fact_id: &str) -> Result<()> {
    let (entity_id, title, body, source_type, confidence): (String, String, String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT entity_id, title, body, source_type, confidence
               FROM llm_wiki_entries WHERE id = ?1 AND deleted_at IS NULL",
            params![fact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("fact not found or already archived: {fact_id}"))?;

    // Idempotent re-confirm (spec §4): an already fully-stamped fact is a
    // no-op BEFORE the ledger append, so neither the event nor the jsonl line
    // can duplicate.
    if source_type.as_deref() == Some("user_confirmed")
        && confidence.as_deref() == Some("confirmed")
    {
        return Ok(());
    }

    let now_ms = crate::wisdom_deposit::now_ms();
    let at_iso = confirm_iso_utc(now_ms)?;

    // 1. Ledger first (spec §8 durable carrier): Task 2's primitive, verbatim
    //    — append the content-hash-keyed line AND its own `source_type`
    //    upsert. If the stamp below fails after this, the rebuild still
    //    re-derives the confirmation from the ledger.
    let content_hash =
        crate::db::stated_ledger::confirmation_content_hash(&title, &body);
    crate::db::stated_ledger::record_confirmation_at(
        conn,
        vault_root,
        fact_id,
        &content_hash,
        HUMAN_CONFIRMER,
        &at_iso,
    )?;

    // 2. The full stamp, one transaction: UPDATE + outbox Update + event.
    //    `updated_at` untouched (drafts.rs discipline), `lifecycle_status`
    //    untouched (already `stable`; spec §3.3).
    let tx = conn.unchecked_transaction()?;
    let changed = tx.execute(
        "UPDATE llm_wiki_entries
            SET source_type = 'user_confirmed',
                confidence = 'confirmed',
                okf_verified = json_insert(
                    COALESCE(NULLIF(okf_verified, ''), '[]'),
                    '$[#]',
                    json_object('by', ?3, 'at', ?4)
                ),
                last_verified_by = ?3,
                last_verified_at = ?5
          WHERE id = ?1 AND entity_id = ?2
            AND NOT (source_type = 'user_confirmed' AND confidence = 'confirmed')",
        params![fact_id, entity_id, HUMAN_CONFIRMER, at_iso, now_ms],
    )?;
    if changed == 0 {
        // Event-once guard (spec §4: the same rows_affected pattern proposal
        // resolution uses) — a concurrent confirm converged elsewhere.
        return Ok(());
    }
    crate::db::commit::push_entries_outbox(
        &tx,
        &entity_id,
        fact_id,
        crate::db::outbox_format::OutboxOperation::Update,
        crate::db::drafts::fact_row_outbox_payload(&tx, fact_id, &entity_id)?,
        now_ms,
    )?;
    tx.execute(
        "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
         VALUES (?1, ?2, 'fact_confirmed', ?3, ?4, ?5)",
        params![
            crate::db::commit::generate_llm_id("evt_"),
            entity_id,
            "Confirmed by Kurt in the desktop app — human tier (spec §3.3)",
            fact_id,
            now_ms
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// Deny a queue fact (spec §3.3: deny = archive + suppress). Archive reuses
/// the existing `archive_wisdom_in_tx` primitive (soft-delete + edge purge +
/// outbox Delete); the suppression row keys the (document path, chunk hash,
/// fact-text hash) triple from `fact_text_hash` (Task 4, sibling-safe), and
/// the durable denial rides `stated/.denials.jsonl` through Task 2's append
/// primitive. Idempotent: an already-denied key is a no-op.
pub fn deny_fact(conn: &Connection, fact_id: &str) -> Result<()> {
    let vault_root = crate::db::stated_ledger::resolve_vault_root()?;
    deny_fact_at(conn, &vault_root, fact_id)
}

/// The `deny_fact_at` source row: the fact plus the first evidence row's
/// document path / evidence JSON (factored out for clippy::type_complexity).
struct DenySourceRow {
    entity_id: String,
    title: String,
    body: String,
    document_path: Option<String>,
    evidence_json: Option<String>,
}

/// Explicit-root form of [`deny_fact`].
pub fn deny_fact_at(conn: &Connection, vault_root: &Path, fact_id: &str) -> Result<()> {
    // Row + denial key via the same join chain the queue reads (first
    // evidence row wins — the same fan-out rule queue_rows applies). A fact
    // with no evidence source document/chunk cannot form the key (spec §3.3)
    // and is REFUSED rather than suppressed keyless.
    let row: Option<DenySourceRow> = conn
        .query_row(
            "SELECT e.entity_id, e.title, e.body, d.path, le.evidence_json
               FROM llm_wiki_entries e
               LEFT JOIN librarian_evidence le ON le.entry_id = e.id
               LEFT JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
               LEFT JOIN documents d ON d.id = ps.doc_id
              WHERE e.id = ?1 AND e.deleted_at IS NULL",
            params![fact_id],
            |r| {
                Ok(DenySourceRow {
                    entity_id: r.get(0)?,
                    title: r.get(1)?,
                    body: r.get(2)?,
                    document_path: r.get(3)?,
                    evidence_json: r.get(4)?,
                })
            },
        )
        .optional()?;
    let Some(DenySourceRow {
        entity_id,
        title,
        body,
        document_path: doc_path,
        evidence_json,
    }) = row
    else {
        anyhow::bail!("fact not found or already archived: {fact_id}");
    };
    // Evidence JSON shape pinned by commit.rs: {"proposal_id",
    // "evidence":[{chunk_id, content_hash, quote, ...}]} — first entry wins.
    let chunk_hash = evidence_json
        .as_deref()
        .and_then(|ej| serde_json::from_str::<serde_json::Value>(ej).ok())
        .and_then(|v| {
            v["evidence"][0]["content_hash"]
                .as_str()
                .map(str::to_string)
        });
    let (Some(document_path), Some(chunk_hash)) = (doc_path, chunk_hash) else {
        anyhow::bail!(
            "cannot deny {fact_id}: no evidence source document/chunk to key \
             the denial (spec §3.3)"
        );
    };
    let fact_hash = fact_text_hash(&title, &body);

    let already: i64 = conn.query_row(
        "SELECT COUNT(*) FROM queue_denials
          WHERE document_path = ?1 AND chunk_hash = ?2 AND fact_text_hash = ?3",
        params![document_path, chunk_hash, fact_hash],
        |r| r.get(0),
    )?;
    if already > 0 {
        // Idempotent re-deny: no duplicate ledger line.
        return Ok(());
    }

    let now_ms = crate::wisdom_deposit::now_ms();
    let at_iso = confirm_iso_utc(now_ms)?;

    // 1. Ledger first (durable suppression — the rebuild repopulates from it,
    //    so a failure in the brain write below still converges).
    crate::db::stated_ledger::append_denial(
        vault_root,
        &crate::db::stated_ledger::DenialLine {
            document_path: document_path.clone(),
            chunk_hash: chunk_hash.clone(),
            fact_text_hash: fact_hash.clone(),
            denied_by: HUMAN_CONFIRMER.to_string(),
            denied_at: at_iso.clone(),
        },
    )?;

    // 2. Archive (reused primitive: cluster-rekeyed soft-delete + edge purge
    //    + outbox Delete) + the suppression row, one transaction.
    let tx = conn.unchecked_transaction()?;
    crate::db::wisdom::archive_wisdom_in_tx(&tx, &entity_id, fact_id)?;
    tx.execute(
        "INSERT OR REPLACE INTO queue_denials
            (document_path, chunk_hash, fact_text_hash, denied_by, denied_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![document_path, chunk_hash, fact_hash, HUMAN_CONFIRMER, at_iso],
    )?;
    tx.commit()?;
    Ok(())
}

/// Un-deny: drop the suppression everywhere and un-archive the denied fact
/// (spec §3.3 "un-denying un-archives the fact").
pub fn undeny_fact(conn: &Connection, key: &DenialKey) -> Result<()> {
    let vault_root = crate::db::stated_ledger::resolve_vault_root()?;
    undeny_fact_at(conn, &vault_root, key)
}

/// Explicit-root form of [`undeny_fact`].
pub fn undeny_fact_at(conn: &Connection, vault_root: &Path, key: &DenialKey) -> Result<()> {
    let now_ms = crate::wisdom_deposit::now_ms();
    // 1. Durable side first: drop the ledger line. `DenialLine` has no
    //    tombstone shape and the rebuild repopulates verbatim, so un-denying
    //    rewrites the ledger without the line (Task-2 `remove_denial`). A
    //    missing line is fine — the key may already be gone; the remaining
    //    drops are then no-ops.
    crate::db::stated_ledger::remove_denial(
        vault_root,
        &key.document_path,
        &key.chunk_hash,
        &key.fact_text_hash,
    )?;

    // 2. Brain side, one transaction: drop the suppression row, then
    //    un-archive every archived fact whose normalized text matches the
    //    key's fact-text hash, pushing an outbox Update per restored row so
    //    replicas resurrect it (same payload builder as promote_draft).
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM queue_denials
          WHERE document_path = ?1 AND chunk_hash = ?2 AND fact_text_hash = ?3",
        params![key.document_path, key.chunk_hash, key.fact_text_hash],
    )?;
    let archived: Vec<(String, String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, entity_id, title, body
               FROM llm_wiki_entries WHERE deleted_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, entity_id, title, body) in archived {
        if fact_text_hash(&title, &body) != key.fact_text_hash {
            continue;
        }
        tx.execute(
            "UPDATE llm_wiki_entries SET deleted_at = NULL WHERE id = ?1",
            params![id],
        )?;
        crate::db::commit::push_entries_outbox(
            &tx,
            &entity_id,
            &id,
            crate::db::outbox_format::OutboxOperation::Update,
            crate::db::drafts::fact_row_outbox_payload(&tx, &id, &entity_id)?,
            now_ms,
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// ISO-8601 UTC millis, `Z` suffix — the same stamp format `promote_draft`
/// writes (drafts.rs:56-58).
fn confirm_iso_utc(now_ms: i64) -> Result<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or_else(|| anyhow::anyhow!("timestamp out of range: {now_ms}"))
}


#[cfg(test)]
mod tests {
    use super::enqueue_vault_event;
    use crate::db::connection::open_in_memory;
    use crate::db::queries::upsert_document;
    use rusqlite::Connection;
    use std::path::PathBuf;

    // ---- enqueue_vault_event fixtures ----------------------------------

    /// Serializes every test that touches `CURATED_VAULT_ROOT`.
    /// `temp_env::with_var` mutates the process-global environment; without
    /// this lock a test that pins the var can race a concurrent test that
    /// needs it unset — the intermittent class the per-test comments
    /// describe but never fixed.
    static VAULT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ---- enqueue_vault_event fixtures ----------------------------------

    /// Open a fresh, fully migrated in-memory brain db. Deletes now record
    /// pending-proposal provenance (issue #211), so a hand-rolled subset of
    /// `documents` is no longer enough, and a partial copy of the curated
    /// DDL would drift from the real one.
    fn open_seeded_conn() -> Connection {
        open_in_memory().expect("open migrated in-memory brain db")
    }

    /// Create a temp vault dir and pre-populate it with a file so notify
    /// events have a real path to canonicalize.
    fn setup_vault_with_file(body: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("note.md");
        std::fs::write(&path, body).unwrap();
        (tmp, path)
    }

    // ---- enqueue_vault_event tests -------------------------------------

    #[test]
    fn enqueue_add_creates_pending_row() {
        let (vault, file_path) = setup_vault_with_file(b"hello world");
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::Any),
                &file_path,
                None,
            )
            .unwrap();
        });

        let mut stmt = conn
            .prepare("SELECT status, hash, tier FROM documents WHERE path = ?1")
            .unwrap();
        // Issue #204: the watcher stores the VIRTUAL path (`abs`), matching
        // what the walker writes. Pre-fix this looked up by the CANONICAL
        // path and silently missed the row.
        let abs = std::path::absolute(&file_path).unwrap();
        let mut rows = stmt
            .query(rusqlite::params![abs.to_string_lossy()])
            .unwrap();
        let row = rows.next().unwrap().unwrap();
        let status: String = row.get(0).unwrap();
        let hash: String = row.get(1).unwrap();
        let tier: String = row.get(2).unwrap();
        assert_eq!(status, "pending");
        assert_eq!(hash, super::sha256_hex(b"hello world"));
        assert_eq!(tier, "user_doc");
        let _ = vault.path();
    }

    /// Issue #204: the watcher's row for a Modify event under a trusted
    /// link in `documents/` MUST be keyed by the VIRTUAL path (the path
    /// notify saw), not the canonical path the symlink resolves to. Pre-fix
    /// the watcher wrote the external canonical path; every subsequent
    /// `reconcile_vault` pass then saw the row as vanished and deleted it
    /// (or, when the walker had already written a virtual-path row, the
    /// two paths never collapsed via the UNIQUE(path) constraint and the
    /// watcher created a divergent phantom row alongside the walker's
    /// authoritative one).
    #[cfg(unix)]
    #[test]
    fn enqueue_modify_under_trusted_link_stages_virtual_path_not_canonical() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join("documents")).unwrap();
        let target = tmp.path().join("ext");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("x.md"), b"content").unwrap();
        // Trusted link: <vault>/documents/specs → <target>
        std::os::unix::fs::symlink(&target, root.join("documents").join("specs")).unwrap();

        let mut conn = open_seeded_conn();
        // raw_path is what notify delivers — the VIRTUAL form notify saw,
        // which is also the path the walker would store under.
        let virtual_path = root.join("documents").join("specs").join("x.md");
        enqueue_vault_event(&mut conn, modify(), &virtual_path, Some(&root)).unwrap();

        let staged = staged_paths(&conn);
        assert_eq!(staged.len(), 1, "exactly one row must be staged");
        // The stored path MUST be the virtual form, not the canonical
        // (external) target. Pre-fix this asserted canonical and broke
        // reconcile for any vault with an approved `documents/` trusted
        // link.
        assert_eq!(
            staged[0],
            virtual_path.to_string_lossy(),
            "watcher must stage the virtual path, not the canonical symlink \
             target — pre-fix divergence from the walker (issue #204)"
        );
        // The canonical form (external target) must NOT appear in the
        // documents table — that was the phantom row D2a called out.
        let canonical_external = std::fs::canonicalize(target.join("x.md")).unwrap();
        assert_ne!(
            staged[0],
            canonical_external.to_string_lossy(),
            "the external canonical path must not be the staged form"
        );
    }

    #[test]
    fn enqueue_modify_indexed_with_diff_hash_flips_to_pending() {
        let (vault, file_path) = setup_vault_with_file(b"v1");
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        // Pre-seed: indexed doc with stale hash + synth watermark.
        // Issue #204: pre-seed under the VIRTUAL path (abs), matching what
        // the watcher stores post-fix. The bytes-on-disk write still uses
        // the canonical path below — write goes to the filesystem, the row
        // stores the path, and the two are independent.
        let abs = std::path::absolute(&file_path).unwrap();
        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let path_str = abs.to_string_lossy().into_owned();
        upsert_document(&conn, &path_str, "stale-hash").unwrap();
        conn.execute(
            "UPDATE documents SET status = 'indexed', synth_hash = 'stale-hash', synth_model = 'm' WHERE path = ?1",
            rusqlite::params![&path_str],
        )
        .unwrap();

        // Now mutate the file on disk; Modify event arrives with the new hash.
        std::fs::write(&canonical, b"v2").unwrap();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Modify(notify::event::ModifyKind::Any),
                &file_path,
                None,
            )
            .unwrap();
        });

        let mut stmt = conn
            .prepare("SELECT status, hash FROM documents WHERE path = ?1")
            .unwrap();
        let mut rows = stmt.query(rusqlite::params![&path_str]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let status: String = row.get(0).unwrap();
        let hash: String = row.get(1).unwrap();
        assert_eq!(status, "pending");
        assert_eq!(hash, super::sha256_hex(b"v2"));
        let _ = vault.path();
    }

    #[test]
    fn enqueue_modify_indexed_with_same_hash_is_noop() {
        let (vault, file_path) = setup_vault_with_file(b"stable");
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        let abs = std::path::absolute(&file_path).unwrap();
        let path_str = abs.to_string_lossy().into_owned();
        let stable_hash = super::sha256_hex(b"stable");
        upsert_document(&conn, &path_str, &stable_hash).unwrap();
        conn.execute(
            "UPDATE documents SET status = 'indexed', synth_hash = ?2, synth_model = 'm' WHERE path = ?1",
            rusqlite::params![&path_str, &stable_hash],
        )
        .unwrap();

        // No filesystem mutation; Modify event fires but bytes are unchanged.
        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Modify(notify::event::ModifyKind::Any),
                &file_path,
                None,
            )
            .unwrap();
        });

        let mut stmt = conn
            .prepare("SELECT status FROM documents WHERE path = ?1")
            .unwrap();
        let mut rows = stmt.query(rusqlite::params![&path_str]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let status: String = row.get(0).unwrap();
        assert_eq!(
            status, "indexed",
            "Modify with unchanged bytes must not flip status back to pending"
        );
        let _ = vault.path();
    }

    #[test]
    fn enqueue_delete_removes_row() {
        let (vault, file_path) = setup_vault_with_file(b"to-be-deleted");
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        let abs = std::path::absolute(&file_path).unwrap();
        let path_str = abs.to_string_lossy().into_owned();
        upsert_document(&conn, &path_str, "h").unwrap();
        let count_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_before, 1);

        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Remove(notify::event::RemoveKind::Any),
                &file_path,
                None,
            )
            .unwrap();
        });

        let count_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_after, 0, "Remove event must delete the documents row");
        let _ = vault.path();
    }

    #[test]
    fn enqueue_delete_missing_row_is_noop() {
        let (vault, file_path) = setup_vault_with_file(b"never-indexed");
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        let abs = std::path::absolute(&file_path).unwrap();
        let _path_str = abs.to_string_lossy().into_owned();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 0);

        // Remove event for a path we never ingested must not error.
        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Remove(notify::event::RemoveKind::Any),
                &file_path,
                None,
            )
            .unwrap();
        });

        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0);
        let _ = vault.path();
    }

    #[test]
    fn enqueue_out_of_vault_path_is_rejected() {
        // Use temp_env to set CURATED_VAULT_ROOT to a directory that does
        // NOT contain the file we touch — the event must be rejected.
        let inner_vault = tempfile::TempDir::new().unwrap();
        let outer_vault = tempfile::TempDir::new().unwrap();
        let outer_file = outer_vault.path().join("escapee.md");
        std::fs::write(&outer_file, b"outside").unwrap();
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(inner_vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::Any),
                &outer_file,
                None,
            )
            .unwrap();
        });

        let abs = std::path::absolute(&outer_file).unwrap();
        let path_str = abs.to_string_lossy().into_owned();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "out-of-vault event must not insert a row");
    }

    /// Positive counterpart to `enqueue_out_of_vault_path_is_rejected`: a
    /// path inside the vault must be ingested when `CURATED_VAULT_ROOT` is
    /// set. Without this, a regression that simply skips every event would
    /// pass the negative test alone.
    #[test]
    fn enqueue_in_vault_path_is_accepted() {
        let vault = tempfile::TempDir::new().unwrap();
        let note = vault.path().join("note.md");
        std::fs::write(&note, b"inside").unwrap();
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::Any),
                &note,
                None,
            )
            .unwrap();
        });

        let abs = std::path::absolute(&note).unwrap();
        let path_str = abs.to_string_lossy().into_owned();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "in-vault event must insert a row");
        let _ = vault.path();
    }

    #[test]
    fn excluded_temp_paths_are_not_staged() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut conn = open_seeded_conn();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(dir.path()), || {
            for name in ["note.md~", "note.md.tmp", ".#note.md", "4913"] {
                let p = dir.path().join(name);
                std::fs::write(&p, b"scratch").unwrap();
                enqueue_vault_event(
                    &mut conn,
                    notify::EventKind::Create(notify::event::CreateKind::File),
                    &p,
                    None,
                )
                .unwrap();
            }
        });

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "editor temp files must never be staged");
        let _ = dir.path();
    }

    #[test]
    fn remove_event_deletes_even_excluded_named_rows() {
        // A row staged for a temp-named file by pre-filter code (or another
        // tool) must still be healable: when the scratch file's Remove event
        // arrives, the exclusion check must not swallow the delete. Deleting
        // is always safe regardless of naming; exclusion only gates staging.
        let dir = tempfile::TempDir::new().unwrap();
        // Seed with the CANONICAL path: enqueue_vault_event canonicalizes the
        // event path (macOS /var -> /private/var), and the seeded row must use
        // the same spelling for the DELETE to match.
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let mut conn = open_seeded_conn();
        let p = base.join("note.md~");
        std::fs::write(&p, b"scratch").unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h', 'user_doc', 'pending')",
            rusqlite::params![p.to_string_lossy().as_ref()],
        )
        .unwrap();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(dir.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Remove(notify::event::RemoveKind::File),
                &p,
                None,
            )
            .expect("Remove for an excluded-named path must still delete");
        });

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "legacy ghost rows must be deletable via Remove");
        let _ = dir.path();
    }

    #[test]
    fn ordinary_markdown_is_still_staged() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut conn = open_seeded_conn();
        let p = dir.path().join("real-note.md");
        std::fs::write(&p, b"# real").unwrap();
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(dir.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::File),
                &p,
                None,
            )
            .unwrap();
        });

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "the filter must not swallow real content");
        let _ = dir.path();
    }

    #[test]
    fn vanished_file_is_treated_as_a_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        // TempDir paths come back through /var, a symlink to /private/var.
        // Canonicalize the root so the containment check matches the
        // canonical path of a LIVE file; for the vanished file canonicalize
        // falls back to the non-canonical absolute path, and a /var root
        // would then reject it as out-of-vault before the read is attempted.
        let vault_root = dir.path().canonicalize().unwrap();
        let mut conn = open_seeded_conn();
        // Build the path under the canonicalized root: once the file is gone
        // canonicalize() falls back to the raw absolute path, and that must
        // still pass the containment check for the read (and thus the
        // NotFound branch) to be reached at all.
        let p = vault_root.join("racy.md");
        let _env = VAULT_ENV_LOCK.lock().unwrap();

        // Stage it while it exists.
        std::fs::write(&p, b"# here").unwrap();
        temp_env::with_var("CURATED_VAULT_ROOT", Some(&vault_root), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::File),
                &p,
                None,
            )
            .unwrap();
        });
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        // Now it disappears, and a Modify event arrives for the dead path.
        std::fs::remove_file(&p).unwrap();
        temp_env::with_var("CURATED_VAULT_ROOT", Some(&vault_root), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Modify(notify::event::ModifyKind::Data(
                    notify::event::DataChange::Content,
                )),
                &p,
                None,
            )
            .expect("a vanished file is a delete, not an error");
        });

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "the row must not outlive the file");
        let _ = dir.path();
    }

    // ---- .brain exclusion gate (spec D1-D4) ----------------------------

    /// Write `body` at `root/rel`, creating parents. Returns the abs path.
    fn write_at(root: &std::path::Path, rel: &str, body: &[u8]) -> PathBuf {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    fn staged_paths(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT path FROM documents ORDER BY path")
            .unwrap();
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    fn modify() -> notify::EventKind {
        notify::EventKind::Modify(notify::event::ModifyKind::Any)
    }

    #[test]
    fn gate_rejects_brain_paths_and_stages_lookalikes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();

        let excluded = vec![
            write_at(&root, ".brain/errors.md", b"x"),
            write_at(&root, "nested/.brain/x.md", b"x"),
        ];
        let controls = [
            write_at(&root, "notes.md", b"a"),
            write_at(&root, "brain/x.md", b"b"),
            write_at(&root, "my.brain.notes/x.md", b"c"),
            write_at(&root, ".brainish/x.md", b"d"),
        ];

        let mut conn = open_seeded_conn();
        for p in excluded.iter().chain(controls.iter()) {
            enqueue_vault_event(&mut conn, modify(), p, Some(&root)).unwrap();
        }

        let staged = staged_paths(&conn);
        assert_eq!(
            staged.len(),
            controls.len(),
            "expected only control paths staged, got {staged:?}"
        );
        for p in &excluded {
            let name = p.to_string_lossy();
            assert!(
                !staged.iter().any(|s| s.contains(".brain/")),
                "{name} was staged: {staged:?}"
            );
        }
        assert!(staged.iter().any(|s| s.ends_with("brain/x.md")));
        assert!(staged.iter().any(|s| s.ends_with("my.brain.notes/x.md")));
        assert!(staged.iter().any(|s| s.ends_with(".brainish/x.md")));
    }

    /// Spec D1 / test "excluded-name vault root": a vault under an
    /// EXCLUDED_DIRS-named ancestor must still live-sync. Without
    /// relativization this test fails, which is the point.
    #[test]
    fn gate_does_not_reject_vault_under_excluded_ancestor() {
        for ancestor in ["build", "target"] {
            let tmp = tempfile::TempDir::new().unwrap();
            let root = tmp.path().join(ancestor).join("wiki");
            std::fs::create_dir_all(&root).unwrap();
            let note = write_at(&root, "notes.md", b"a");

            let mut conn = open_seeded_conn();
            enqueue_vault_event(&mut conn, modify(), &note, Some(&root)).unwrap();

            assert_eq!(
                staged_paths(&conn).len(),
                1,
                "vault under /{ancestor}/ lost live-sync"
            );
        }
    }

    /// Spec D2: canonicalize STRIPS `.brain` when the dir is symlinked out.
    /// Matching the canonical path would let the bug recur for that subtree.
    #[cfg(unix)]
    #[test]
    fn gate_rejects_symlinked_out_brain_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let real = tmp.path().join("elsewhere-no-brain-in-name");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("x.md"), b"x").unwrap();
        std::os::unix::fs::symlink(&real, root.join("nested").join(".brain")).unwrap();

        let mut conn = open_seeded_conn();
        let virtual_path = root.join("nested").join(".brain").join("x.md");
        enqueue_vault_event(&mut conn, modify(), &virtual_path, Some(&root)).unwrap();

        assert!(
            staged_paths(&conn).is_empty(),
            "symlinked-out .brain leaked through the gate"
        );
    }

    /// Spec D2: canonicalize INJECTS `.brain` for a trusted link whose
    /// target lives under one. The gate must not reject it.
    ///
    /// Per spec D2a the stored path is the EXTERNAL CANONICAL path, not
    /// `documents/specs/x.md`. This asserts the gate's verdict AND pins
    /// that pre-existing divergence so it stays visible.
    #[cfg(unix)]
    #[test]
    fn gate_allows_trusted_link_into_brain_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join("documents")).unwrap();
        let target = tmp.path().join("ext").join(".brain").join("specs-target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("x.md"), b"content").unwrap();
        std::os::unix::fs::symlink(&target, root.join("documents").join("specs")).unwrap();

        let mut conn = open_seeded_conn();
        let virtual_path = root.join("documents").join("specs").join("x.md");
        enqueue_vault_event(&mut conn, modify(), &virtual_path, Some(&root)).unwrap();

        let staged = staged_paths(&conn);
        assert_eq!(
            staged.len(),
            1,
            "trusted link into .brain was gated: {staged:?}"
        );
        // Issue #204: the watcher now stores the VIRTUAL path, matching
        // the walker's space (reconcile.rs:74-77 documents the column as
        // virtual). Pre-fix this asserted the CANONICAL path, which kept
        // the divergence visible per the old D2a "out of scope" framing.
        // The canonical form MUST NOT appear here — that would re-create
        // the phantom row whose reconcile-pass delete D2a warned about.
        let canonical_target = std::fs::canonicalize(target.join("x.md")).unwrap();
        assert_eq!(
            staged[0],
            virtual_path.to_string_lossy(),
            "watcher must stage the virtual path the walker and reconcile use"
        );
        assert_ne!(
            staged[0],
            canonical_target.to_string_lossy(),
            "the external canonical target must not be the staged form (issue #204)"
        );
    }

    /// Spec D3: no resolvable root -> gate skipped, behavior unchanged.
    #[test]
    fn gate_skipped_when_vault_root_unresolvable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let p = write_at(&root, ".brain/errors.md", b"x");

        // NOTE: CURATED_VAULT_ROOT must be unset for this test, but tests
        // run in threads sharing one process env — `enqueue_out_of_vault_*`
        // SETS it via `temp_env`, so leaving it unset here races (the exact
        // intermittent-failure class documented at queue.rs:187-194). Pin
        // it to None: that both takes the `temp_env` lock and forces the
        // env var unset for the duration, deterministically.
        let _env = VAULT_ENV_LOCK.lock().unwrap();
        temp_env::with_var("CURATED_VAULT_ROOT", None::<&str>, || {
            let mut conn = open_seeded_conn();
            enqueue_vault_event(&mut conn, modify(), &p, None).unwrap();

            assert_eq!(
                staged_paths(&conn).len(),
                1,
                "gate must be skipped, not fail-closed, without a vault root"
            );
        });
    }

    /// Spec D2b: a path under no form of the root stages (fail-open).
    #[test]
    fn gate_fails_open_for_unrelativizable_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let p = write_at(&outside, ".brain/x.md", b"x");

        let mut conn = open_seeded_conn();
        // Root passed explicitly, but the path is outside it. The existing
        // containment guard is what rejects out-of-vault paths; the GATE
        // itself must fail open rather than fail closed.
        enqueue_vault_event(&mut conn, modify(), &p, Some(&outside)).unwrap();
        assert_eq!(
            staged_paths(&conn).len(),
            0,
            "control: .brain under its own root is gated"
        );

        let mut conn2 = open_seeded_conn();
        enqueue_vault_event(&mut conn2, modify(), &p, Some(&root)).unwrap();
        // Containment guard rejects it (path is outside `root`), so nothing
        // stages -- but it must be the CONTAINMENT guard doing so, not a
        // fail-closed gate. Assert no panic / no error, which is the
        // contract D2b protects.
        assert!(staged_paths(&conn2).is_empty());
    }

    /// Deletes stay ungated: a pre-staged .brain row must still heal.
    #[test]
    fn remove_event_still_deletes_pre_staged_brain_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let p = write_at(&root, ".brain/errors.log", b"x");
        // Issue #204: the seed row must use the VIRTUAL path (`abs(p)`),
        // matching the form the watcher now stores. Pre-fix the watcher
        // stored `canonical`, which on macOS (/var → /private/var) diverges
        // from `p` and the DELETE no-ops.
        let abs = std::path::absolute(&p).unwrap();

        let mut conn = open_seeded_conn();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'deadbeef', 'user_doc', 'pending')",
            rusqlite::params![abs.to_string_lossy()],
        )
        .unwrap();
        assert_eq!(staged_paths(&conn).len(), 1);

        enqueue_vault_event(
            &mut conn,
            notify::EventKind::Remove(notify::event::RemoveKind::Any),
            &p,
            Some(&root),
        )
        .unwrap();

        assert!(
            staged_paths(&conn).is_empty(),
            "Remove must run BEFORE the gate so old rows can heal"
        );
    }

    // ---- issue #203: extension gate ------------------------------------

    /// Spec tests 1, 3, 4, 5: non-ingestable files never stage; an
    /// ingestable sibling in the same vault still does.
    #[test]
    fn extension_gate_rejects_non_ingestable_and_stages_control() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();

        let mut conn = open_seeded_conn();
        for rel in ["app.log", "SHOUT.LOG", "Makefile", "photo.png"] {
            let p = write_at(&root, rel, b"x");
            enqueue_vault_event(&mut conn, modify(), &p, Some(&root)).unwrap();
        }
        assert!(
            staged_paths(&conn).is_empty(),
            "non-ingestable extensions (incl. uppercase and extensionless) \
             must not stage: {:?}",
            staged_paths(&conn)
        );

        let md = write_at(&root, "note.md", b"y");
        enqueue_vault_event(&mut conn, modify(), &md, Some(&root)).unwrap();
        let abs = std::path::absolute(&md).unwrap();
        assert_eq!(
            staged_paths(&conn),
            vec![abs.to_string_lossy().into_owned()]
        );
    }

    /// Spec test 2: a row staged before the gate existed is still healed
    /// by its Remove event.
    #[test]
    fn extension_gate_does_not_block_remove_of_pre_gate_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let p = write_at(&root, "app.log", b"x");
        let abs = std::path::absolute(&p).unwrap();

        let mut conn = open_seeded_conn();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'deadbeef', 'user_doc', 'pending')",
            rusqlite::params![abs.to_string_lossy()],
        )
        .unwrap();

        enqueue_vault_event(
            &mut conn,
            notify::EventKind::Remove(notify::event::RemoveKind::Any),
            &p,
            Some(&root),
        )
        .unwrap();
        assert!(staged_paths(&conn).is_empty(), "Remove must heal the row");
    }

    /// Spec test 6: the gate does not depend on vault-root resolution
    /// (`ct watch` passes `None`; the desktop never sets the env var).
    #[test]
    fn extension_gate_active_without_vault_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = write_at(tmp.path(), "app.log", b"x");

        let _env = VAULT_ENV_LOCK.lock().unwrap();
        temp_env::with_var("CURATED_VAULT_ROOT", None::<&str>, || {
            let mut conn = open_seeded_conn();
            enqueue_vault_event(&mut conn, modify(), &p, None).unwrap();
            assert!(staged_paths(&conn).is_empty());
        });
    }

    /// Spec test 7: the gate runs before `std::fs::read`. Without it, the
    /// read's NotFound arm would DELETE this seeded row; with it, the event
    /// returns before any I/O and the row is untouched.
    #[test]
    fn extension_gate_precedes_file_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("gone.log"); // never written to disk
        let abs = std::path::absolute(&p).unwrap();

        let mut conn = open_seeded_conn();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'deadbeef', 'user_doc', 'pending')",
            rusqlite::params![abs.to_string_lossy()],
        )
        .unwrap();

        enqueue_vault_event(&mut conn, modify(), &p, Some(&root)).unwrap();
        assert_eq!(
            staged_paths(&conn),
            vec![abs.to_string_lossy().into_owned()],
            "gate must return before the NotFound delete arm"
        );
    }

    // ---- issue #211: deletion provenance -------------------------------

    /// Spec test 10, Remove branch.
    #[test]
    fn remove_event_records_deleted_source_for_pending_proposal() {
        use crate::db::proposals::test_support::{deleted_source_rows, seed_pending_proposal};
        use crate::db::proposals::ProposalSourceRole::Trigger;

        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let p = write_at(&root, "cited.md", b"cited");
        let path_str = std::path::absolute(&p)
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let mut conn = open_seeded_conn();
        let doc_id = upsert_document(&conn, &path_str, "h-cited").unwrap();
        seed_pending_proposal(&conn, "prop-remove", &[(doc_id, Trigger)]);

        enqueue_vault_event(
            &mut conn,
            notify::EventKind::Remove(notify::event::RemoveKind::Any),
            &p,
            Some(&root),
        )
        .unwrap();

        assert!(staged_paths(&conn).is_empty(), "Remove must delete the row");
        assert_eq!(
            deleted_source_rows(&conn, "prop-remove"),
            vec![(path_str, "h-cited".into(), "trigger".into())]
        );
    }

    /// Spec test 10, NotFound branch (a Modify for a file that vanished).
    #[test]
    fn vanished_file_records_deleted_source_for_pending_proposal() {
        use crate::db::proposals::test_support::{deleted_source_rows, seed_pending_proposal};
        use crate::db::proposals::ProposalSourceRole::Trigger;

        let dir = tempfile::TempDir::new().unwrap();
        // Canonicalized for the same reason as `vanished_file_is_treated_as_a_delete`.
        let root = dir.path().canonicalize().unwrap();
        let p = write_at(&root, "racy.md", b"# racy");

        let mut conn = open_seeded_conn();
        enqueue_vault_event(&mut conn, modify(), &p, Some(&root)).unwrap();
        let path_str = staged_paths(&conn).pop().expect("staged while present");
        let doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                [&path_str],
                |r| r.get(0),
            )
            .unwrap();
        seed_pending_proposal(&conn, "prop-vanished", &[(doc_id, Trigger)]);

        std::fs::remove_file(&p).unwrap();
        enqueue_vault_event(&mut conn, modify(), &p, Some(&root)).unwrap();

        assert!(
            staged_paths(&conn).is_empty(),
            "the row must not outlive the file"
        );
        assert_eq!(deleted_source_rows(&conn, "prop-vanished").len(), 1);
    }
}

// ---------------------------------------------------------------------------
// Confirmation queue tests (user-confirmation-queue plan Task 4; spec rev 4.1
// §3.2). The queue is a QUERY over llm_wiki_entries, not a table: live entries
// whose provenance is not human-tier, minus deny-suppressed rows.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod confirmation_queue_tests {
    use super::{fact_text_hash, queue_rows, QueueRow};
    use crate::db::connection::open_in_memory;
    use rusqlite::params;
    use std::collections::HashSet;

    const DAY_MS: i64 = 86_400_000;

    fn conn() -> rusqlite::Connection {
        open_in_memory().expect("open migrated in-memory brain db")
    }

    fn insert_entry(conn: &rusqlite::Connection, id: &str, source_type: &str, title: &str, created_at: i64) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, source_type, created_at, updated_at)
             VALUES (?1, 'ent_queue', ?2, ?3, ?4, ?5, ?5)",
            params![id, title, format!("body of {title}"), source_type, created_at],
        )
        .unwrap();
    }

    /// Evidence-chain fixture: one proposal bound to one source document
    /// (curated_proposal_sources) — the `deposit_status_row` join chain's
    /// proposal→documents leg.
    fn seed_proposal_doc(conn: &rusqlite::Connection, proposal_id: &str, doc_path: &str) {
        let doc_id = crate::db::queries::upsert_document(conn, doc_path, "hash-q").unwrap();
        conn.execute(
            "INSERT INTO curated_proposals (id, kind, model, created_at)
             VALUES (?1, 'new_entity', 'test', ?2)",
            params![proposal_id, crate::wisdom_deposit::now_ms()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
             VALUES (?1, ?2, 'evidence')",
            params![proposal_id, doc_id],
        )
        .unwrap();
    }

    /// One librarian_evidence row binding `entry_id` to the proposal, whose
    /// evidence_json carries the chunk hash + quote exactly like
    /// `evidence_json_with_hashes` writes them (commit.rs).
    fn seed_evidence_row(
        conn: &rusqlite::Connection,
        entry_id: &str,
        proposal_id: &str,
        chunk_hash: &str,
        quote: &str,
    ) {
        let evidence_json = serde_json::json!({
            "proposal_id": proposal_id,
            "evidence": [{
                "chunk_id": 1,
                "content_hash": chunk_hash,
                "quote": quote,
                "start_line": 1,
                "end_line": 2,
                "source_kind": null,
            }]
        })
        .to_string();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, unanchored, created_at)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![entry_id, proposal_id, evidence_json, crate::wisdom_deposit::now_ms()],
        )
        .unwrap();
    }

    fn ids_of(rows: Vec<QueueRow>) -> HashSet<String> {
        rows.into_iter().map(|r| r.fact_id).collect()
    }

    #[test]
    fn queue_membership_live_not_human_tier() {
        let conn = conn();
        let now = crate::wisdom_deposit::now_ms();

        // In: agent-tier provenance.
        insert_entry(&conn, "q-inferred", "librarian_inferred", "Inferred fact", now);
        insert_entry(&conn, "q-immutable", "immutable_document", "Document fact", now);
        // In: out-of-vocabulary provenance counts as unverified (spec §3.2).
        // (The NULL case is admitted by the same SQL clause but is not
        // insertable on the current schema: source_type is NOT NULL there.)
        insert_entry(&conn, "q-future-tier", "some_future_tier", "Future tier fact", now);

        // Out: human tier.
        insert_entry(&conn, "q-stated", "user_stated", "Stated fact", now);
        insert_entry(&conn, "q-confirmed", "user_confirmed", "Confirmed fact", now);

        // Out: not live.
        insert_entry(&conn, "q-superseded", "librarian_inferred", "Superseded fact", now);
        conn.execute(
            "UPDATE llm_wiki_entries SET superseded_by = 'q-inferred', superseded_at = ?1
             WHERE id = 'q-superseded'",
            params![now],
        )
        .unwrap();
        insert_entry(&conn, "q-expired", "librarian_inferred", "Expired fact", now);
        conn.execute(
            "UPDATE llm_wiki_entries SET valid_to = ?1 WHERE id = 'q-expired'",
            params![now - 1],
        )
        .unwrap();
        insert_entry(&conn, "q-deleted", "librarian_inferred", "Deleted fact", now);
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = ?1 WHERE id = 'q-deleted'",
            params![now],
        )
        .unwrap();
        insert_entry(&conn, "q-draft", "librarian_inferred", "Draft fact", now);
        conn.execute(
            "UPDATE llm_wiki_entries SET lifecycle_status = 'draft' WHERE id = 'q-draft'",
            [],
        )
        .unwrap();

        let ids = ids_of(queue_rows(&conn));
        assert!(ids.contains("q-inferred"), "librarian_inferred in: {ids:?}");
        assert!(ids.contains("q-immutable"), "immutable_document in: {ids:?}");
        assert!(
            ids.contains("q-future-tier"),
            "out-of-vocab provenance in: {ids:?}"
        );
        assert!(!ids.contains("q-stated"), "user_stated out: {ids:?}");
        assert!(!ids.contains("q-confirmed"), "user_confirmed out: {ids:?}");
        assert!(!ids.contains("q-superseded"), "superseded out: {ids:?}");
        assert!(!ids.contains("q-expired"), "expired out: {ids:?}");
        assert!(!ids.contains("q-deleted"), "deleted out: {ids:?}");
        assert!(!ids.contains("q-draft"), "non-stable lifecycle out: {ids:?}");

        // ORDER BY age DESC: the older fact comes FIRST.
        insert_entry(&conn, "q-older", "librarian_inferred", "Older fact", now - 30 * DAY_MS);
        let rows = queue_rows(&conn);
        let pos = |id: &str| {
            rows.iter()
                .position(|r| r.fact_id == id)
                .unwrap_or_else(|| panic!("{id} present"))
        };
        assert!(
            pos("q-older") < pos("q-inferred"),
            "ORDER BY age DESC puts the oldest fact first"
        );
    }

    #[test]
    fn denied_fact_excluded_sibling_included() {
        let conn = conn();
        let now = crate::wisdom_deposit::now_ms();

        // Two facts extracted from the SAME document chunk: same doc path +
        // same chunk hash, different fact text.
        insert_entry(&conn, "q-sib-a", "librarian_inferred", "Denied fact title", now);
        insert_entry(&conn, "q-sib-b", "librarian_inferred", "Sibling fact title", now);
        seed_proposal_doc(&conn, "p-sib", "documents/note.md");
        seed_evidence_row(&conn, "q-sib-a", "p-sib", "chunkhash-sib", "shared quote");
        seed_evidence_row(&conn, "q-sib-b", "p-sib", "chunkhash-sib", "shared quote");

        // Deny fact A only: key = (doc path, chunk hash, A's fact-text hash).
        conn.execute(
            "INSERT INTO queue_denials (document_path, chunk_hash, fact_text_hash, denied_by, denied_at)
             VALUES ('documents/note.md', 'chunkhash-sib', ?1, 'test', ?2)",
            params![
                fact_text_hash("Denied fact title", "body of Denied fact title"),
                now
            ],
        )
        .unwrap();

        let ids = ids_of(queue_rows(&conn));
        assert!(
            !ids.contains("q-sib-a"),
            "denied fact is suppressed: {ids:?}"
        );
        assert!(
            ids.contains("q-sib-b"),
            "sibling fact (same doc+chunk, different text) stays in: {ids:?}"
        );
    }

    #[test]
    fn evidence_quote_and_attribution_surfaces_on_row() {
        let conn = conn();
        let now = crate::wisdom_deposit::now_ms();

        insert_entry(
            &conn,
            "q-attr",
            "librarian_inferred",
            "Attributed fact",
            now - 2 * DAY_MS,
        );
        seed_proposal_doc(&conn, "p-attr", "documents/talk.md");
        seed_evidence_row(&conn, "q-attr", "p-attr", "chunkhash-attr", "the quoted sentence");
        conn.execute(
            "INSERT INTO deposit_attribution (document_path, speaker, kind, source, deposited_at)
             VALUES ('documents/talk.md', 'kurt', 'verbatim', 'discord:123', ?1)",
            params![now],
        )
        .unwrap();

        let rows = queue_rows(&conn);
        assert_eq!(rows.len(), 1, "one queue row, no join fan-out: {rows:?}");
        let r = &rows[0];
        assert_eq!(r.fact_id, "q-attr");
        assert_eq!(r.title, "Attributed fact");
        assert_eq!(r.provenance.as_deref(), Some("librarian_inferred"));
        assert_eq!(r.entity_id, "ent_queue");
        assert_eq!(
            r.evidence_quote.as_deref(),
            Some("the quoted sentence"),
            "evidence quote via the deposit_status_row join chain"
        );
        assert_eq!(r.source_document.as_deref(), Some("documents/talk.md"));
        assert_eq!(
            r.attributed_speaker.as_deref(),
            Some("kurt"),
            "attributed deposit speaker joins on the source document path"
        );
        assert_eq!(
            r.instruction_source, None,
            "approval_instruction_source was DROPPED (spec rev 4.1 §6) — no carrier exists"
        );
        assert_eq!(r.age_days, 2, "age_days floors the ms age to whole days");
    }
}

// Task 5 (user-confirmation-queue plan): confirm/deny/undeny action tests.
#[cfg(test)]
mod confirmation_action_tests {
    use super::{confirm_fact_at, deny_fact_at, fact_text_hash, queue_rows, undeny_fact_at, DenialKey};
    use crate::db::connection::open_in_memory;
    use rusqlite::params;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn conn() -> rusqlite::Connection {
        open_in_memory().expect("open migrated in-memory brain db")
    }

    /// Scratch vault root for the JSONL ledgers (explicit form — no env).
    fn vault() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        (tmp, root)
    }

    fn insert_entry(conn: &rusqlite::Connection, id: &str, title: &str, created_at: i64) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, source_type, created_at, updated_at)
             VALUES (?1, 'ent_queue', ?2, ?3, 'librarian_inferred', ?4, ?4)",
            params![id, title, format!("body of {title}"), created_at],
        )
        .unwrap();
    }

    /// `archive_wisdom_in_tx` requires a live entity (production reality:
    /// every fact belongs to one).
    fn seed_entity(conn: &rusqlite::Connection) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_queue', 'Queue', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
    }

    fn seed_proposal_doc(conn: &rusqlite::Connection, proposal_id: &str, doc_path: &str) {
        let doc_id = crate::db::queries::upsert_document(conn, doc_path, "hash-q").unwrap();
        conn.execute(
            "INSERT INTO curated_proposals (id, kind, model, created_at)
             VALUES (?1, 'new_entity', 'test', ?2)",
            params![proposal_id, crate::wisdom_deposit::now_ms()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
             VALUES (?1, ?2, 'evidence')",
            params![proposal_id, doc_id],
        )
        .unwrap();
    }

    fn seed_evidence_row(
        conn: &rusqlite::Connection,
        entry_id: &str,
        proposal_id: &str,
        chunk_hash: &str,
        quote: &str,
    ) {
        let evidence_json = serde_json::json!({
            "proposal_id": proposal_id,
            "evidence": [{
                "chunk_id": 1,
                "content_hash": chunk_hash,
                "quote": quote,
                "start_line": 1,
                "end_line": 2,
                "source_kind": null,
            }]
        })
        .to_string();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, unanchored, created_at)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![entry_id, proposal_id, evidence_json, crate::wisdom_deposit::now_ms()],
        )
        .unwrap();
    }

    fn ids_of(rows: Vec<super::QueueRow>) -> HashSet<String> {
        rows.into_iter().map(|r| r.fact_id).collect()
    }

    fn ledger_lines(vault: &std::path::Path, name: &str) -> Vec<serde_json::Value> {
        let path = vault.join("stated").join(name);
        let raw = std::fs::read_to_string(path).unwrap_or_default();
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("ledger line is JSON"))
            .collect()
    }

    #[test]
    fn confirm_stamps_the_full_shape() {
        let conn = conn();
        let (_tmp, vault) = vault();
        let now = crate::wisdom_deposit::now_ms();
        insert_entry(&conn, "c1", "Confirmed fact", now);

        confirm_fact_at(&conn, &vault, "c1").unwrap();

        // The brain row: provenance + confidence + verified append + mirror;
        // lifecycle_status and updated_at UNTOUCHED (spec §3.3).
        let (source_type, confidence, okf_verified, last_verified_by, last_verified_at, lifecycle_status, updated_at): (
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT source_type, confidence, okf_verified, last_verified_by,
                        last_verified_at, lifecycle_status, updated_at
                   FROM llm_wiki_entries WHERE id = 'c1'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(source_type, "user_confirmed");
        assert_eq!(confidence, "confirmed", "D7: the app's own direct-write convention");
        let verified: serde_json::Value = serde_json::from_str(&okf_verified).unwrap();
        assert_eq!(verified.as_array().unwrap().len(), 1);
        assert_eq!(verified[0]["by"], "human:kurt", "spec §3 pins the confirmer");
        assert!(
            verified[0]["at"]
                .as_str()
                .and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok())
                .is_some(),
            "okf_verified at is ISO-8601: {:?}",
            verified[0]["at"]
        );
        assert_eq!(last_verified_by.as_deref(), Some("human:kurt"), "mirror");
        assert!(
            last_verified_at.is_some_and(|at| at >= now),
            "last_verified_at mirrors in ms: {last_verified_at:?}"
        );
        assert_eq!(lifecycle_status, "stable", "lifecycle_status is NOT touched");
        assert_eq!(updated_at, now, "updated_at is not bumped (drafts.rs shape)");

        // Outbox Update row carrying the stamped state (Rule 5 sync).
        let (table, operation, payload): (String, String, String) = conn
            .query_row(
                "SELECT table_name, operation, payload FROM llm_wiki_outbox
                  WHERE record_id = 'c1' ORDER BY rowid",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((table.as_str(), operation.as_str()), ("entries", "UPDATE"));
        let p: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(p["source_type"], "user_confirmed");
        assert_eq!(p["confidence"], "confirmed");
        assert_eq!(p["last_verified_by"], "human:kurt");

        // One event.
        let (event_type, related): (String, Option<String>) = conn
            .query_row(
                "SELECT event_type, related_entry_id FROM llm_wiki_events
                  WHERE entity_id = 'ent_queue'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_type, "fact_confirmed");
        assert_eq!(related.as_deref(), Some("c1"));

        // The durable ledger line (spec §8): fact_id, content_hash, by, at.
        let lines = ledger_lines(&vault, ".confirmations.jsonl");
        assert_eq!(lines.len(), 1, "exactly one ledger line: {lines:?}");
        assert_eq!(lines[0]["fact_id"], "c1");
        assert_eq!(
            lines[0]["content_hash"],
            crate::db::stated_ledger::confirmation_content_hash(
                "Confirmed fact",
                "body of Confirmed fact"
            ),
            "ledger is keyed by content hash, the rebuild's stable key"
        );
        assert_eq!(lines[0]["by"], "human:kurt");
        assert!(lines[0]["at"].is_string());
    }

    #[test]
    fn confirm_is_idempotent() {
        let conn = conn();
        let (_tmp, vault) = vault();
        let now = crate::wisdom_deposit::now_ms();
        insert_entry(&conn, "c1", "Confirmed fact", now);
        // A pre-existing agent trust entry so the append is observable.
        conn.execute(
            "UPDATE llm_wiki_entries SET okf_verified = ?1 WHERE id = 'c1'",
            params![r#"[{"by":"agent:librarian","at":"2026-01-01T00:00:00.000Z"}]"#],
        )
        .unwrap();

        confirm_fact_at(&conn, &vault, "c1").unwrap();
        confirm_fact_at(&conn, &vault, "c1").unwrap();

        let (events, ledger): (i64, Vec<serde_json::Value>) = (
            conn.query_row(
                "SELECT COUNT(*) FROM llm_wiki_events WHERE entity_id = 'ent_queue'",
                [],
                |r| r.get(0),
            )
            .unwrap(),
            ledger_lines(&vault, ".confirmations.jsonl"),
        );
        assert_eq!(events, 1, "no duplicate event on re-confirm");
        assert_eq!(ledger.len(), 1, "no duplicate ledger line on re-confirm");

        let okf_verified: String = conn
            .query_row(
                "SELECT okf_verified FROM llm_wiki_entries WHERE id = 'c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let verified: serde_json::Value = serde_json::from_str(&okf_verified).unwrap();
        assert_eq!(
            verified.as_array().unwrap().len(),
            2,
            "agent entry preserved, exactly one human append"
        );
    }

    #[test]
    fn deny_suppresses_and_undeny_restores() {
        let conn = conn();
        let (_tmp, vault) = vault();
        seed_entity(&conn);
        let now = crate::wisdom_deposit::now_ms();

        // Two facts from the SAME chunk — denying one must not hide the other.
        insert_entry(&conn, "q-denied", "Denied fact title", now);
        insert_entry(&conn, "q-sibling", "Sibling fact title", now);
        seed_proposal_doc(&conn, "p-deny", "documents/note.md");
        seed_evidence_row(&conn, "q-denied", "p-deny", "chunkhash-deny", "shared quote");
        seed_evidence_row(&conn, "q-sibling", "p-deny", "chunkhash-deny", "shared quote");

        deny_fact_at(&conn, &vault, "q-denied").unwrap();

        let ids = ids_of(queue_rows(&conn));
        assert!(!ids.contains("q-denied"), "denied fact is out of the queue: {ids:?}");
        assert!(ids.contains("q-sibling"), "sibling stays in: {ids:?}");

        // Archived (spec §3.3 deny = archive + suppress) + suppression row.
        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'q-denied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted_at.is_some(), "deny archives the fact");

        let key = DenialKey {
            document_path: "documents/note.md".into(),
            chunk_hash: "chunkhash-deny".into(),
            fact_text_hash: fact_text_hash("Denied fact title", "body of Denied fact title"),
        };
        let in_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM queue_denials
                  WHERE document_path = ?1 AND chunk_hash = ?2 AND fact_text_hash = ?3",
                params![key.document_path, key.chunk_hash, key.fact_text_hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_table, 1, "queue_denials row keyed by the triple");

        let lines = ledger_lines(&vault, ".denials.jsonl");
        assert_eq!(lines.len(), 1, "durable denial ledger line: {lines:?}");
        assert_eq!(lines[0]["document_path"], key.document_path);
        assert_eq!(lines[0]["chunk_hash"], key.chunk_hash);
        assert_eq!(lines[0]["fact_text_hash"], key.fact_text_hash);
        assert_eq!(lines[0]["denied_by"], "human:kurt");
        assert!(lines[0]["denied_at"].is_string());

        // Un-deny: suppression drops everywhere, the fact is un-archived and
        // returns to the queue.
        undeny_fact_at(&conn, &vault, &key).unwrap();

        let ids = ids_of(queue_rows(&conn));
        assert!(ids.contains("q-denied"), "un-denied fact is back: {ids:?}");
        assert!(ids.contains("q-sibling"), "sibling unaffected: {ids:?}");
        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'q-denied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted_at.is_none(), "un-denying un-archives the fact");
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM queue_denials", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "suppression row dropped");
        let lines = ledger_lines(&vault, ".denials.jsonl");
        assert!(
            lines.is_empty(),
            "ledger no longer carries the denial: {lines:?}"
        );
    }
}
