//! Stated verification ledgers (user-confirmation-queue Task 2, spec rev 4.1
//! §2/B2 + §8): the app-written `.confirmations.jsonl` / `.attestations.jsonl`
//! / `.denials.jsonl` mirrors under the vault's `stated/` directory, and the
//! brain-side rebuild that re-derives `user_confirmed` stamps, attestation
//! rows, denial rows, and the deposit-attribution index from them.
//!
//! There is NO vault fact file for confirmations (spec §8, B2-rev4): the JSONL
//! ledger is the only durable carrier, the brain rows are derived, and the
//! match key is the SHA-256 of title+body — `fact_id` is advisory (rev 4.1
//! gate N3: fact ids are random and change across rebuilds).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::hasher::hash_bytes;

/// Ledger directory inside the vault root (spec §8: `stated/.confirmations.jsonl`).
pub const STATED_DIR: &str = "stated";
pub const CONFIRMATIONS_LEDGER: &str = ".confirmations.jsonl";
pub const ATTESTATIONS_LEDGER: &str = ".attestations.jsonl";
pub const DENIALS_LEDGER: &str = ".denials.jsonl";

/// The stable confirmation match key (spec §8 "content_hash(title+body)"):
/// SHA-256 over the title bytes immediately followed by the body bytes. Fact
/// ids are random and change across rebuilds; this hash is what both the live
/// confirm path and the rebuild match on (rev 4.1 gate N3).
pub fn confirmation_content_hash(title: &str, body: &str) -> String {
    let mut bytes = Vec::with_capacity(title.len() + body.len());
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(body.as_bytes());
    hash_bytes(&bytes)
}

/// One line of `stated/.confirmations.jsonl` (spec §8 field set, pinned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmationLine {
    pub fact_id: String,
    pub content_hash: String,
    pub by: String,
    pub at: String,
}

/// One line of `stated/.attestations.jsonl` (spec §8: content-hash keyed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationLine {
    pub document_path: String,
    pub content_hash: String,
    pub by: String,
    pub at: String,
}

/// One line of `stated/.denials.jsonl` (spec §8: document path + chunk hash +
/// fact-text hash + who/when).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenialLine {
    pub document_path: String,
    pub chunk_hash: String,
    pub fact_text_hash: String,
    pub denied_by: String,
    pub denied_at: String,
}

/// Per-table counts from [`rebuild_rederive`]. The spec (rev 4.1) does not pin
/// these fields, so this is the minimal set that makes the rebuild observable:
/// what each ledger contributed and where lines were dropped or re-derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct RebuildReport {
    /// Lines read from `stated/.confirmations.jsonl`.
    pub confirmations_read: usize,
    /// `llm_wiki_entries` rows whose hash matched a ledger line and were
    /// stamped through the FULL shared helper (D1 v2): `source_type =
    /// 'user_confirmed'` plus confidence/okf_verified/outbox/event, idempotent
    /// on already-stamped rows.
    pub confirmations_applied: usize,
    /// Lines read from `stated/.attestations.jsonl`.
    pub attestations_read: usize,
    /// Attestation lines whose file's CURRENT hash no longer matches (or whose
    /// file is gone) — dropped, per spec §8 ("the person re-attests").
    pub attestations_dropped: usize,
    /// Lines read from `stated/.denials.jsonl`.
    pub denials_read: usize,
    /// `deposit_attribution` rows repopulated from vault deposit files'
    /// frontmatter (the authoritative carrier, Task-1 ruling).
    pub deposit_attribution_rows: usize,
}

/// Append one JSON line to a vault ledger. The file is created on first append
/// and NEVER truncated — single-line appends only, one JSON object per line.
/// Free text is escaped by serde_json (Task-1 review ruling A3: never
/// hand-concatenate interpolations into serialized lines).
fn append_line<T: Serialize>(vault_root: &Path, ledger: &str, line: &T) -> Result<()> {
    let dir = vault_root.join(STATED_DIR);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating ledger dir {}", dir.display()))?;
    let path = dir.join(ledger);
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening ledger {}", path.display()))?;
    // ONE write syscall for line+newline (Tier-1.5 xhigh MINOR-4): writeln! on
    // an unbuffered File issues two writes, and a concurrent appender can
    // interleave between them, tearing the line — read_lines treats that as a
    // hard error, which would defer the rebuild and fail deny/undeny loudly.
    let mut buf = serde_json::to_string(line)?;
    buf.push('\n');
    f.write_all(buf.as_bytes())
        .with_context(|| format!("appending to {}", path.display()))?;
    Ok(())
}

/// Read every line of a ledger as `T`. A missing file is an empty ledger
/// (nothing recorded yet); a malformed line is an error — silently skipping it
/// would make the rebuild lie about what was re-derived.
fn read_lines<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading ledger {}", path.display()))?;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str::<T>(line).with_context(|| {
            format!("{} line {}: not a valid ledger line", path.display(), n + 1)
        })?);
    }
    Ok(out)
}

/// Append one attestation to `stated/.attestations.jsonl` (spec §8). Thin public
/// wrapper over the private [`append_line`] primitive so the APP's capture
/// flow (db::wisdom::capture_statement, Task 9) writes THROUGH the shared
/// primitive rather than forking it.
pub fn append_attestation(vault_root: &Path, line: &AttestationLine) -> Result<()> {
    append_line(vault_root, ATTESTATIONS_LEDGER, line)
}

/// Append one denial to `stated/.denials.jsonl` (spec §8). Thin public wrapper
/// over the private [`append_line`] primitive so the deny action (queue.rs,
/// Task 5) writes THROUGH the shared primitive rather than forking it.
pub fn append_denial(vault_root: &Path, line: &DenialLine) -> Result<()> {
    append_line(vault_root, DENIALS_LEDGER, line)
}

/// Read the durable denial lines. `pub(crate)` so the Task-5 deny action
/// (queue.rs) can presence-check the LEDGER — not just the brain table —
/// before appending (Task-5 Low-2 closeout: a retry whose earlier brain
/// transaction failed must not append a duplicate ledger line).
pub(crate) fn read_denial_lines(vault_root: &Path) -> Result<Vec<DenialLine>> {
    read_lines(&vault_root.join(STATED_DIR).join(DENIALS_LEDGER))
}

/// Remove every denial line matching
/// `(document_path, chunk_hash, fact_text_hash)` from
/// `stated/.denials.jsonl` — the un-deny path. The ledger has no tombstone
/// shape (`DenialLine`'s field set is pinned by spec §8 and Task 2) and the
/// rebuild repopulates suppression verbatim, so un-denying REWRITES the file
/// without the matching line(s). The file is left untouched when nothing
/// matches (returns 0). Malformed lines still error, exactly like the reader.
pub fn remove_denial(
    vault_root: &Path,
    document_path: &str,
    chunk_hash: &str,
    fact_text_hash: &str,
) -> Result<usize> {
    let path = vault_root.join(STATED_DIR).join(DENIALS_LEDGER);
    let lines: Vec<DenialLine> = read_lines(&path)?;
    let kept: Vec<&DenialLine> = lines
        .iter()
        .filter(|l| {
            !(l.document_path == document_path
                && l.chunk_hash == chunk_hash
                && l.fact_text_hash == fact_text_hash)
        })
        .collect();
    let removed = lines.len() - kept.len();
    if removed == 0 {
        return Ok(0);
    }
    let mut out = String::new();
    for l in &kept {
        out.push_str(&serde_json::to_string(l)?);
        out.push('\n');
    }
    // Task-5 Low-1 closeout: the rewrite is ATOMIC — write to a same-directory
    // temp file (fsynced, original permission bits preserved) and `rename` it
    // over the ledger, so a crash mid-write can never leave a truncated
    // `.denials.jsonl` behind.
    crate::vault::safe_path::safe_write_bytes(&path, out.as_bytes())
        .with_context(|| format!("rewriting ledger {}", path.display()))?;
    Ok(removed)
}

/// Resolve the configured vault root from the environment (the same resolver
/// `AppDb::open` uses), so ledger writers can find `stated/`. `pub(crate)` so
/// the Task 5 confirm/deny actions (queue.rs) reuse the SAME resolver rather
/// than re-deriving a root.
pub(crate) fn resolve_vault_root() -> Result<std::path::PathBuf> {
    let paths = crate::retrieval::resolve_brain_paths();
    let root = crate::vault::config::VaultConfig::new(paths.config_path).vault_root()?;
    root.ok_or_else(|| {
        anyhow!("no vault root configured; cannot locate the stated/ jsonl ledgers")
    })
}

/// Append the confirmation to the vault-mirror ledger (spec §8) and upsert the
/// brain-side `user_confirmed` stamp. The vault root comes from the configured
/// brain config; [`record_confirmation_at`] is the explicit-root form.
pub fn record_confirmation(
    conn: &Connection,
    fact_id: &str,
    content_hash: &str,
    by: &str,
    at: &str,
) -> Result<()> {
    let vault_root = resolve_vault_root()?;
    record_confirmation_at(conn, &vault_root, fact_id, content_hash, by, at)
}

/// [`record_confirmation`] with an explicit vault root (the form tests and any
/// caller that already holds the root should use).
pub fn record_confirmation_at(
    conn: &Connection,
    vault_root: &Path,
    fact_id: &str,
    content_hash: &str,
    by: &str,
    at: &str,
) -> Result<()> {
    let line = ConfirmationLine {
        fact_id: fact_id.to_string(),
        content_hash: content_hash.to_string(),
        by: by.to_string(),
        at: at.to_string(),
    };
    append_line(vault_root, CONFIRMATIONS_LEDGER, &line)?;
    // Upsert the brain-side stamp now (spec §3: the confirm updates the brain
    // row AND appends to the ledger). Matching is by content hash — fact_id is
    // advisory even here — so the live stamp and the rebuild stamp agree.
    // D1 v2: the hash leg stamps through the FULL shared helper, so twins
    // present right now get the same stamp the rebuild will re-derive.
    // D1-V2b (review F1 MAJOR): the hash leg runs the FULL three-statement
    // stamp (UPDATE + outbox + event). On the bare connection that ran in
    // per-statement autocommit, so a mid-stamp failure (SQLITE_BUSY, disk
    // I/O, outbox payload failure) could commit the guarded UPDATE but not
    // the outbox/event — the row then read as fully stamped, the confirm
    // retry pre-check (queue.rs) returned Ok forever, and the rebuild's
    // `changed == 0` guard skipped it: permanent divergence, outbox/event
    // never emitted. Wrap the leg in its OWN transaction: any mid-stamp
    // error rolls back ALL THREE statements, so a stamp that emits
    // outbox/event can never be observed partially applied. (The rebuild
    // leg passes `&tx` from `rebuild_rederive` and is unaffected; every
    // caller of this function holds no open transaction of its own.)
    let tx = conn.unchecked_transaction()?;
    apply_confirmations_by_hash(&tx, std::slice::from_ref(&line))?;
    tx.commit()?;
    Ok(())
}

/// Retry guard for the confirm path — mirror of the deny-side ledger check
/// (`read_denial_lines` in `queue.rs`'s `deny_fact_at`). A confirm retry whose
/// earlier brain transaction failed must not append a duplicate
/// `.confirmations.jsonl` line: the rebuild is hash-idempotent, so a duplicate
/// is harmless — this is ledger hygiene, not lost-state repair.
pub fn confirmation_ledger_has_key(vault_root: &Path, content_hash: &str) -> Result<bool> {
    let stated = vault_root.join(STATED_DIR);
    Ok(read_lines::<ConfirmationLine>(&stated.join(CONFIRMATIONS_LEDGER))?
        .iter()
        .any(|l| l.content_hash == content_hash))
}

/// Stamp every non-deleted entry whose current title+body hash matches a
/// ledger line through the FULL shared stamp helper (D1 v2 ruling,
/// 2026-10-22): `source_type` + `confidence` + `okf_verified` entry + outbox
/// Update + `fact_confirmed` event — identical to the clicked path, via the
/// ONE helper `queue::stamp_user_confirmed` (no forked bare upsert).
/// `by`/`at` come from the ledger line (first line per hash wins — duplicate
/// appends stamp the same row once); `at` is parsed back to ms so the stamp
/// carries the confirmation's own moment. Idempotent: the helper's
/// `NOT (fully stamped)` guard makes an already-stamped row a no-op with no
/// new outbox row or event (event-once, spec §4). Returns the number of
/// hash-matched rows the leg stamped (idempotent re-stamps count — the
/// report's "rows the hash leg stamped" semantics is unchanged). Takes
/// `&Connection` so the rebuild passes `&tx` (outbox rows and events land
/// atomically with the rebuild); the record path passes its own short
/// transaction from `record_confirmation_at` (D1-V2b: the record-path stamp
/// is atomic too).
fn apply_confirmations_by_hash(conn: &Connection, lines: &[ConfirmationLine]) -> Result<usize> {
    let mut line_by_hash: HashMap<&str, &ConfirmationLine> = HashMap::new();
    for line in lines {
        line_by_hash
            .entry(line.content_hash.as_str())
            .or_insert(line);
    }
    let mut stmt = conn.prepare(
        "SELECT id, entity_id, title, body FROM llm_wiki_entries WHERE deleted_at IS NULL",
    )?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    let mut stamped = 0;
    for (id, entity_id, title, body) in rows {
        if let Some(line) = line_by_hash.get(confirmation_content_hash(&title, &body).as_str()) {
            let now_ms = rfc3339_to_ms(&line.at).unwrap_or_else(crate::wisdom_deposit::now_ms);
            super::queue::stamp_user_confirmed(conn, &id, &entity_id, &line.by, now_ms, &line.at)?;
            stamped += 1;
        }
    }
    Ok(stamped)
}

/// ISO-8601 → epoch ms — the inverse of queue.rs `confirm_iso_utc`
/// (millis precision, `Z` suffix; the roundtrip is exact). `None` on a
/// malformed stamp; the caller falls back to now. Sub-millisecond precision
/// in a ledger `at` is TRUNCATED to millis (accepted-as-is, D1-V2b ruling:
/// production lines are millis-exact via `confirm_iso_utc`).
fn rfc3339_to_ms(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Re-derive every stated-verification table from the durable carriers:
///
/// * `stated/.confirmations.jsonl` → `user_confirmed` stamps on
///   `llm_wiki_entries`, matched ONLY by content hash (spec §8; a mismatched
///   hash simply never matches — "the person re-confirms in the app");
/// * `stated/.attestations.jsonl` → `stated_attestations`, each line
///   re-verified against the file's CURRENT bytes; mismatch (or missing file)
///   → dropped;
/// * `stated/.denials.jsonl` → `queue_denials`, repopulated verbatim (the
///   ledger pins the suppression keys; the chunk/fact text itself is not
///   stored, so there is nothing to re-hash);
/// * vault deposit files' frontmatter → `deposit_attribution` (the FILE is the
///   authoritative carrier; this is the "re-ingest repopulates the index"
///   path, Task-1 ruling).
///
/// The whole rebuild runs in one transaction: it either lands or the previous
/// derived state survives.
pub fn rebuild_rederive(conn: &Connection, vault_root: &Path) -> Result<RebuildReport> {
    // B1 round 2 (mechanism v2): the rebuild must key derived rows in the
    // SAME canonical absolute form the watcher stages, regardless of how the
    // root was spelled by the caller (connection.rs passes `roots.canonical`,
    // but tests and future callers may hand a symlinked/relative/`..`-bearing
    // root). Canonicalize ONCE at the top — the ledger reads, the hash
    // re-verification, and the repopulate walk below all resolve to the
    // canonical forms, so capture-era == rebuild-era keys by construction.
    // Fail-closed on a vanished root via `?`.
    let vault_root = &vault_root.canonicalize()?;
    let tx = conn.unchecked_transaction()?;
    let mut report = RebuildReport::default();
    let stated = vault_root.join(STATED_DIR);

    // 1. Confirmations → user_confirmed stamps (dedup by hash; duplicate
    //    appends of the same confirmation stamp the same row once).
    //
    //    Orphan-stamp ruling (Task-5 verdict E2): this rebuild is the ONLY
    //    place an orphan `user_confirmed` stamp (stamp present, ledger line
    //    gone/mismatched) gets resolved — and it does so implicitly, by
    //    re-deriving stamps from the ledger (spec §8:511). The confirm path
    //    never resets an existing stamp: §4:628-630 makes an existing stamp
    //    terminal. No explicit reset logic exists or is needed here.
    let confirmations: Vec<ConfirmationLine> = read_lines(&stated.join(CONFIRMATIONS_LEDGER))?;
    report.confirmations_read = confirmations.len();
    report.confirmations_applied = apply_confirmations_by_hash(&tx, &confirmations)?;

    // 2. Attestations → stated_attestations, re-verifying current file hashes.
    //    The TABLE KEY is the watcher's virtual-absolute `documents.path`
    //    form (`virtual_path_key`, defect B1) so the Path B provenance join
    //    (`a.document_path = d.path`) matches the watcher/walker-staged row;
    //    the ledger carrier itself stays vault-relative (spec §3.8). A ledger
    //    `document_path` that is absolute or contains a `..` segment is never
    //    trusted for join-based trust decisions — skipped via the dropped
    //    counter, same as a hash mismatch.
    tx.execute("DELETE FROM stated_attestations", [])?;
    let attestations: Vec<AttestationLine> = read_lines(&stated.join(ATTESTATIONS_LEDGER))?;
    report.attestations_read = attestations.len();
    for line in attestations {
        if std::path::Path::new(&line.document_path).is_absolute()
            || line.document_path.split(['/', '\\']).any(|seg| seg == "..")
        {
            report.attestations_dropped += 1;
            continue;
        }
        let file = vault_root.join(&line.document_path);
        let kept = std::fs::read(&file)
            .map(|bytes| hash_bytes(&bytes) == line.content_hash)
            .unwrap_or(false); // unreadable/missing file cannot be verified → dropped
        if kept {
            let key = super::queue::virtual_path_key(&file)?;
            tx.execute(
                "INSERT OR REPLACE INTO stated_attestations
                     (document_path, content_hash, attested_by, attested_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![key, line.content_hash, line.by, line.at],
            )?;
        } else {
            report.attestations_dropped += 1;
        }
    }

    // 3. Denials → queue_denials (verbatim; INSERT OR REPLACE keeps a second
    //    rebuild idempotent against the same ledger).
    tx.execute("DELETE FROM queue_denials", [])?;
    let denials: Vec<DenialLine> = read_lines(&stated.join(DENIALS_LEDGER))?;
    report.denials_read = denials.len();
    for line in denials {
        tx.execute(
            "INSERT OR REPLACE INTO queue_denials
                 (document_path, chunk_hash, fact_text_hash, denied_by, denied_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                line.document_path,
                line.chunk_hash,
                line.fact_text_hash,
                line.denied_by,
                line.denied_at
            ],
        )?;
    }

    // 4. Deposit attribution → re-ingest from the files (authoritative).
    tx.execute("DELETE FROM deposit_attribution", [])?;
    report.deposit_attribution_rows = repopulate_deposit_attribution(&tx, vault_root)?;

    tx.commit()?;
    Ok(report)
}

/// Walk the vault for markdown files carrying attribution frontmatter and
/// repopulate `deposit_attribution` keyed by the watcher's virtual-absolute
/// `documents.path` form (`virtual_path_key`, defect B1) — the same key the
/// queue's attribution join (`da.document_path = d.path`, queue.rs:307)
/// matches against. A file whose frontmatter is absent or invalid contributes
/// no row (the file wins, but an unreadable attribution is not one).
fn repopulate_deposit_attribution(conn: &Connection, vault_root: &Path) -> Result<usize> {
    use walkdir::WalkDir;

    let mut count = 0;
    for entry in WalkDir::new(vault_root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() || entry.path().extension().map(|e| e != "md").unwrap_or(true)
        {
            continue;
        }
        let rel = match entry.path().strip_prefix(vault_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // Skip hidden dirs (.git, .brain, ...) — attribution notes live in the
        // visible vault tree. `stated/` holds app-written statements, not
        // deposits, but its files simply carry no attribution frontmatter, so
        // no special case is needed beyond correctness.
        if rel
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let text = match std::fs::read_to_string(entry.path()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let Some(a) = parse_attribution_frontmatter(&text) else {
            continue;
        };
        let key = match super::queue::virtual_path_key(entry.path()) {
            Ok(k) => k,
            Err(_) => continue, // unplaceable path contributes no row
        };
        let deposited_at = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(chrono::DateTime::<chrono::Utc>::from)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        conn.execute(
            "INSERT OR REPLACE INTO deposit_attribution
                 (document_path, speaker, kind, source, deposited_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, a.0, a.1, a.2, deposited_at],
        )?;
        count += 1;
    }
    Ok(count)
}

/// Parse the three attribution keys out of a leading YAML frontmatter block.
/// Returns `(speaker, kind, source)`, or None when there is no attribution
/// frontmatter or the kind is not in the pinned vocabulary.
fn parse_attribution_frontmatter(text: &str) -> Option<(String, String, String)> {
    let rest = text.strip_prefix("---\n")?;
    let block = rest.split("\n---").next()?;
    let mut speaker = None;
    let mut kind = None;
    let mut source = None;
    for line in block.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "attributed_speaker" => speaker = Some(value.to_string()),
            "attribution_kind" => kind = Some(value.to_string()),
            "attribution_source" => source = Some(value.to_string()),
            _ => {}
        }
    }
    let kind = crate::wisdom_deposit::AttributionKind::try_from(kind.as_deref()?).ok()?;
    Some((speaker?, kind.as_str().to_string(), source?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::hasher::hash_bytes;
    use crate::vault::config::VaultConfig;
    use rusqlite::{params, Connection};
    use std::path::Path;

    fn seed_fact(conn: &Connection, id: &str, title: &str, body: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding,
                lifecycle_status, okf_verified
             ) VALUES (?1, ?2, ?3, ?4, '[\"a\"]', 'inferred', 'librarian_inferred',
                       NULL, 'documents/x.md', 100, 200, NULL, 0, NULL, NULL, NULL,
                       'stable', '[{\"by\":\"agent:librarian\",\"at\":\"2026-01-01T00:00:00.000Z\"}]')",
            params![id, "e1", title, body],
        )
        .unwrap();
    }

    fn source_type_of(conn: &Connection, id: &str) -> String {
        conn.query_row(
            "SELECT source_type FROM llm_wiki_entries WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Delete every DERIVED brain row the rebuild is responsible for, leaving
    /// only the durable JSONL ledger.
    fn wipe_derived_state(conn: &Connection) {
        conn.execute_batch(
            "UPDATE llm_wiki_entries SET source_type = 'librarian_inferred';
             DELETE FROM deposit_attribution;
             DELETE FROM stated_attestations;
             DELETE FROM queue_denials;",
        )
        .unwrap();
    }

    fn append_confirmation_line(
        vault_root: &Path,
        fact_id: &str,
        content_hash: &str,
        by: &str,
        at: &str,
    ) {
        append_line(
            vault_root,
            CONFIRMATIONS_LEDGER,
            &ConfirmationLine {
                fact_id: fact_id.to_string(),
                content_hash: content_hash.to_string(),
                by: by.to_string(),
                at: at.to_string(),
            },
        )
        .unwrap();
    }

    fn append_attestation_line(
        vault_root: &Path,
        document_path: &str,
        content_hash: &str,
        by: &str,
        at: &str,
    ) {
        append_line(
            vault_root,
            ATTESTATIONS_LEDGER,
            &AttestationLine {
                document_path: document_path.to_string(),
                content_hash: content_hash.to_string(),
                by: by.to_string(),
                at: at.to_string(),
            },
        )
        .unwrap();
    }

    fn deposit_note(vault_root: &Path, rel: &str) {
        let p = vault_root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            "---\nattributed_speaker: kurt\nattribution_kind: verbatim\nattribution_source: discord:123\n---\n\n# Alpha\n\nAlpha body\n",
        )
        .unwrap();
    }

    #[test]
    fn confirmation_jsonl_append_and_rebuild_roundtrip() {
        // scratch brain + scratch vault root; record_confirmation twice; delete
        // brain rows; rebuild_rederive re-inserts; second rebuild is idempotent.
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path().join("vault");
        std::fs::create_dir_all(&vault_root).unwrap();
        let brain = tmp.path().join("brain");
        // Redirect the WHOLE brain dir (issue #178 guard): resolving any live
        // ~/.brain path from a test is a loud failure.
        let brain_str = brain.to_string_lossy().into_owned();
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_DIR", Some(brain_str.as_str())),
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
            ],
            || {
                // The resolver puts config at <brain>/config.json when only
                // CURATED_BRAIN_DIR is set — write the scratch vault there.
                VaultConfig::new(brain.join("config.json"))
                    .set_vault_path(vault_root.to_str().unwrap())
                    .unwrap();
                deposit_note(&vault_root, "notes/deposit.md");

                let conn = open_in_memory().unwrap();
                seed_fact(&conn, "f1", "Alpha", "Alpha body");
                let h = confirmation_content_hash("Alpha", "Alpha body");

                record_confirmation(&conn, "f1", &h, "human:kurt", "2026-10-21T00:00:00Z")
                    .unwrap();
                record_confirmation(&conn, "f1", &h, "human:kurt", "2026-10-21T00:01:00Z")
                    .unwrap();

                // The ledger is in the vault under stated/ (spec §8), one JSON
                // object per line, every field JSON-escaped by the serializer.
                let ledger = vault_root.join(STATED_DIR).join(CONFIRMATIONS_LEDGER);
                let text = std::fs::read_to_string(&ledger).unwrap();
                let lines: Vec<&str> = text.lines().collect();
                assert_eq!(lines.len(), 2);
                for line in &lines {
                    let v: serde_json::Value = serde_json::from_str(line).unwrap();
                    assert_eq!(v["fact_id"], "f1");
                    assert_eq!(v["content_hash"], h.as_str());
                    assert_eq!(v["by"], "human:kurt");
                }
                // record_confirmation upserts the brain-side stamp immediately.
                assert_eq!(source_type_of(&conn, "f1"), "user_confirmed");

                wipe_derived_state(&conn);

                let report = rebuild_rederive(&conn, &vault_root).unwrap();
                assert_eq!(report.confirmations_read, 2);
                assert_eq!(report.confirmations_applied, 1);
                assert_eq!(report.deposit_attribution_rows, 1);
                assert_eq!(source_type_of(&conn, "f1"), "user_confirmed");
                let (speaker, kind): (String, String) = conn
                    .query_row(
                        "SELECT speaker, kind FROM deposit_attribution
                         WHERE document_path = ?1",
                        // Rebuild keys through the CANONICAL root
                        // (`virtual_path_key_at_root`) — derive the expected
                        // key the same way, never the raw tempdir spelling
                        // (macOS: /var/... is a symlink to /private/var/...).
                        [super::super::queue::virtual_path_key_at_root(
                            &vault_root,
                            std::path::Path::new("notes/deposit.md"),
                        )
                        .unwrap()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                assert_eq!((speaker.as_str(), kind.as_str()), ("kurt", "verbatim"));

                // Second rebuild is idempotent: counts unchanged, state unchanged.
                let again = rebuild_rederive(&conn, &vault_root).unwrap();
                assert_eq!(report, again);
                assert_eq!(source_type_of(&conn, "f1"), "user_confirmed");
            },
        );
    }

    #[test]
    fn rebuild_drops_attestation_with_hash_mismatch() {
        // Write .attestations.jsonl lines for two files, then EDIT one;
        // rebuild → the edited file's row is dropped, the untouched one stays.
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let kept = vault_root.join("notes/kept.md");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, b"original bytes").unwrap();
        let changed = vault_root.join("notes/changed.md");
        std::fs::write(&changed, b"original bytes").unwrap();

        let h = hash_bytes(b"original bytes");
        append_attestation_line(vault_root, "notes/kept.md", &h, "human:kurt", "2026-10-21T00:00:00Z");
        append_attestation_line(vault_root, "notes/changed.md", &h, "human:kurt", "2026-10-21T00:00:00Z");

        std::fs::write(&changed, b"edited bytes").unwrap();

        let conn = open_in_memory().unwrap();
        let report = rebuild_rederive(&conn, vault_root).unwrap();
        assert_eq!(report.attestations_read, 2);
        assert_eq!(report.attestations_dropped, 1);
        // B1: rows are keyed by the watcher's virtual-absolute documents.path
        // form now; the ledger carrier stays relative. Derive the expected
        // keys through the SAME canonical-root family the rebuild uses —
        // never the raw tempdir spelling (macOS /var/... vs /private/var/...).
        let expected_keys: Vec<String> = ["notes/kept.md"]
            .iter()
            .map(|rel| {
                super::super::queue::virtual_path_key_at_root(vault_root, std::path::Path::new(rel))
                    .unwrap()
            })
            .collect();
        let rows: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT document_path FROM stated_attestations ORDER BY document_path")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(rows, expected_keys);
    }

    /// B1 hardening: a ledger `document_path` that is absolute or contains a
    /// `..` segment is never trusted for join-based trust decisions — the
    /// rebuild skips it via the dropped counter instead of staging a row that
    /// could key Path B provenance.
    #[test]
    fn rebuild_rejects_non_relative_attestation_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let kept = vault_root.join("notes/kept.md");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, b"original bytes").unwrap();
        let h = hash_bytes(b"original bytes");

        append_attestation_line(
            vault_root,
            &vault_root.join("notes/kept.md").to_string_lossy(),
            &h,
            "human:kurt",
            "2026-10-21T00:00:00Z",
        );
        append_attestation_line(
            vault_root,
            "../outside.md",
            &h,
            "human:kurt",
            "2026-10-21T00:00:00Z",
        );

        let conn = open_in_memory().unwrap();
        let report = rebuild_rederive(&conn, vault_root).unwrap();
        assert_eq!(report.attestations_read, 2);
        assert_eq!(
            report.attestations_dropped, 2,
            "absolute and `..` ledger paths are dropped, never staged"
        );
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM stated_attestations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn confirmation_rebuild_matches_by_content_hash_fact_id_advisory() {
        // Ledger line whose fact_id does not exist but hash matches a row →
        // the row is confirmed (rev 4.1 gate N3: the hash is the stable key).
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let conn = open_in_memory().unwrap();
        seed_fact(&conn, "f9", "Beta", "Beta body");
        let h = confirmation_content_hash("Beta", "Beta body");
        append_confirmation_line(vault_root, "ghost-fact-no-such-row", &h, "human:kurt", "2026-10-21T00:00:00Z");

        let report = rebuild_rederive(&conn, vault_root).unwrap();
        assert_eq!(report.confirmations_read, 1);
        assert_eq!(report.confirmations_applied, 1);
        assert_eq!(source_type_of(&conn, "f9"), "user_confirmed");
    }

    /// D1-v2 (ruling 2026-10-22): a same-text TWIN — identical title+body,
    /// different fact id — must receive the FULL stamp through the shared
    /// helper (`source_type` AND `confidence` AND an `okf_verified` entry AND
    /// an outbox Update row AND a `fact_confirmed` event), identical to the
    /// clicked fact, not the old bare `source_type` upsert.
    #[test]
    fn rebuild_stamps_by_hash_twin_through_full_helper() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let conn = open_in_memory().unwrap();
        seed_fact(&conn, "f1", "Alpha", "Alpha body");
        // The twin: identical text, different id (fixture entity shared).
        seed_fact(&conn, "f2", "Alpha", "Alpha body");

        // Clicked path confirms f1 (one ledger line); the rebuild then
        // re-derives the stamp for BOTH rows by content hash.
        crate::db::queue::confirm_fact_at(&conn, vault_root, "f1").unwrap();

        let report = rebuild_rederive(&conn, vault_root).unwrap();
        assert_eq!(report.confirmations_read, 1);
        assert_eq!(
            report.confirmations_applied, 2,
            "both the clicked fact and its twin stamp through the hash leg"
        );

        // The twin carries the FULL stamp, the same shape the clicked path
        // writes (queue.rs confirm stamp).
        let (source_type, confidence, okf_verified, last_verified_by): (
            String,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT source_type, confidence, okf_verified, last_verified_by
                   FROM llm_wiki_entries WHERE id = 'f2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(source_type, "user_confirmed");
        assert_eq!(
            confidence, "confirmed",
            "twin gets the full stamp, not the bare source_type upsert"
        );
        let verified: serde_json::Value = serde_json::from_str(&okf_verified).unwrap();
        let human = verified
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["by"] == "human:kurt")
            .expect("twin okf_verified gains the human:kurt entry")
            .clone();
        assert!(
            human["at"].as_str().is_some(),
            "okf_verified entry carries an ISO timestamp: {human:?}"
        );
        assert_eq!(last_verified_by.as_deref(), Some("human:kurt"), "mirror");

        // Rule-5 sync: an outbox Update row for the twin.
        let outbox: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox
                  WHERE record_id = 'f2' AND operation = 'UPDATE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outbox, 1, "twin stamp emits exactly one outbox Update row");

        // And one `fact_confirmed` event.
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events
                  WHERE related_entry_id = 'f2' AND event_type = 'fact_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            events, 1,
            "twin stamp emits exactly one fact_confirmed event"
        );
    }

    /// D1-V2b (review F1 MAJOR): the record path (`confirm_fact_at` →
    /// `record_confirmation_at` → `apply_confirmations_by_hash`) must stamp
    /// ATOMICALLY. A deterministic mid-stamp failure — a BEFORE INSERT
    /// trigger on `llm_wiki_events` raising ABORT after the guarded UPDATE
    /// and the outbox insert have run — must propagate AND leave the entries
    /// row UNSTAMPED with NO outbox row. On pre-fix code the UPDATE and the
    /// outbox row autocommitted first, so the row read as fully stamped
    /// without its event (permanent divergence: the confirm retry pre-check
    /// returned Ok forever and the rebuild's `changed == 0` guard skipped
    /// the row — outbox/event never emitted).
    #[test]
    fn record_path_mid_stamp_failure_rolls_back_the_full_stamp() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let conn = open_in_memory().unwrap();
        seed_fact(&conn, "f1", "Alpha", "Alpha body");

        conn.execute_batch(
            "CREATE TRIGGER d1v2b_abort_event BEFORE INSERT ON llm_wiki_events
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();

        let err = crate::db::queue::confirm_fact_at(&conn, vault_root, "f1")
            .expect_err("mid-stamp abort must propagate");
        assert!(
            err.to_string().contains("boom"),
            "the error is the trigger abort, not an earlier failure: {err}"
        );

        // The WHOLE stamp rolled back: row untouched, no orphan outbox row.
        assert_eq!(
            source_type_of(&conn, "f1"),
            "librarian_inferred",
            "a mid-stamp failure must NOT leave the row stamped"
        );
        let outbox: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox WHERE record_id = 'f1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outbox, 0, "no orphan outbox row may survive the abort");

        // The trigger is scoped to this test: dropped before any further
        // assertions, the user's natural retry heals the row completely (the
        // ledger line from the first attempt is the durable carrier; the
        // ledger-duplicate guard keeps it at exactly one line).
        conn.execute_batch("DROP TRIGGER d1v2b_abort_event;")
            .unwrap();
        crate::db::queue::confirm_fact_at(&conn, vault_root, "f1").unwrap();
        assert_eq!(source_type_of(&conn, "f1"), "user_confirmed");
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events
                  WHERE related_entry_id = 'f1' AND event_type = 'fact_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1, "retry emits exactly one fact_confirmed event");
    }

    /// D1-V2b mirror on the REBUILD path: the rebuild is one big transaction,
    /// so the same mid-stamp abort must roll back the WHOLE rebuild — prior
    /// derived state survives verbatim and `rebuild_rederive` returns Err.
    #[test]
    fn rebuild_mid_stamp_failure_rolls_back_whole_rebuild() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let conn = open_in_memory().unwrap();
        seed_fact(&conn, "f1", "Alpha", "Alpha body");
        let h = confirmation_content_hash("Alpha", "Alpha body");
        append_confirmation_line(vault_root, "f1", &h, "human:kurt", "2026-10-21T00:00:00Z");
        // Prior derived state the rebuild must not lose on its rollback
        // (rebuild step 3 DELETEs queue_denials inside the tx).
        conn.execute(
            "INSERT INTO queue_denials
                 (document_path, chunk_hash, fact_text_hash, denied_by, denied_at)
             VALUES ('docs/a.md', 'ck', 'fh', 'human:kurt', '2026-10-21T00:00:00Z')",
            [],
        )
        .unwrap();

        conn.execute_batch(
            "CREATE TRIGGER d1v2b_abort_event BEFORE INSERT ON llm_wiki_events
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();

        let err = rebuild_rederive(&conn, vault_root)
            .expect_err("a mid-stamp abort must fail the whole rebuild");
        assert!(
            err.to_string().contains("boom"),
            "the error is the trigger abort, not an earlier failure: {err}"
        );

        // Scoped to this test: drop the trigger before the recovery checks.
        conn.execute_batch("DROP TRIGGER d1v2b_abort_event;")
            .unwrap();

        // Prior derived state intact, row still unstamped.
        assert_eq!(
            source_type_of(&conn, "f1"),
            "librarian_inferred",
            "aborted rebuild leaves no partial stamp behind"
        );
        let denials: i64 = conn
            .query_row("SELECT COUNT(*) FROM queue_denials", [], |r| r.get(0))
            .unwrap();
        assert_eq!(denials, 1, "rollback restores the prior derived state");

        // Without the trigger the rebuild lands cleanly.
        let report = rebuild_rederive(&conn, vault_root).unwrap();
        assert_eq!(report.confirmations_applied, 1);
        assert_eq!(source_type_of(&conn, "f1"), "user_confirmed");
    }

    /// F3 (D1-V2b review, INFO): a ledger line with an unparsable `at` still
    /// stamps — `apply_confirmations_by_hash` falls back to now-ms instead of
    /// erroring, and the row carries a NON-EMPTY `okf_verified` entry.
    #[test]
    fn rebuild_malformed_at_falls_back_to_now_and_stamps() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let conn = open_in_memory().unwrap();
        seed_fact(&conn, "f1", "Alpha", "Alpha body");
        let h = confirmation_content_hash("Alpha", "Alpha body");
        append_confirmation_line(vault_root, "f1", &h, "human:kurt", "not-a-timestamp");

        let report = rebuild_rederive(&conn, vault_root).unwrap();
        assert_eq!(report.confirmations_read, 1);
        assert_eq!(
            report.confirmations_applied, 1,
            "the malformed-at fallback stamps, it does not error"
        );
        assert_eq!(source_type_of(&conn, "f1"), "user_confirmed");
        let (okf_verified,): (String,) = conn
            .query_row(
                "SELECT okf_verified FROM llm_wiki_entries WHERE id = 'f1'",
                [],
                |r| Ok((r.get(0)?,)),
            )
            .unwrap();
        let verified: serde_json::Value = serde_json::from_str(&okf_verified).unwrap();
        assert!(
            verified.as_array().is_some_and(|a| !a.is_empty()),
            "okf_verified is non-empty after the fallback stamp: {verified}"
        );
    }

    /// Event-once idempotence (D1-v2): a second rebuild over the same ledger
    /// emits NO new outbox rows or events — the shared helper's
    /// `NOT (fully stamped)` guard makes an already-stamped row a no-op
    /// (spec §4). Re-run cleanliness is the property that keeps rebuild
    /// re-runs clean.
    #[test]
    fn rebuild_full_stamp_is_event_once_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        let conn = open_in_memory().unwrap();
        seed_fact(&conn, "f1", "Alpha", "Alpha body");
        seed_fact(&conn, "f2", "Alpha", "Alpha body");
        crate::db::queue::confirm_fact_at(&conn, vault_root, "f1").unwrap();

        let report = rebuild_rederive(&conn, vault_root).unwrap();
        let twin_counts = || -> (i64, i64) {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM llm_wiki_outbox WHERE record_id = 'f2'),
                        (SELECT COUNT(*) FROM llm_wiki_events
                          WHERE related_entry_id = 'f2' AND event_type = 'fact_confirmed')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        let before = twin_counts();

        let again = rebuild_rederive(&conn, vault_root).unwrap();

        assert_eq!(
            before,
            twin_counts(),
            "second rebuild emits NO new outbox rows or events"
        );
        assert_eq!(
            report, again,
            "rebuild report is stable across re-runs (counts unchanged)"
        );
        assert_eq!(again.confirmations_applied, 2);
        // The full stamp itself persists untouched.
        assert_eq!(source_type_of(&conn, "f2"), "user_confirmed");
        let (confidence,): (String,) = conn
            .query_row(
                "SELECT confidence FROM llm_wiki_entries WHERE id = 'f2'",
                [],
                |r| Ok((r.get(0)?,)),
            )
            .unwrap();
        assert_eq!(confidence, "confirmed");
    }
}
