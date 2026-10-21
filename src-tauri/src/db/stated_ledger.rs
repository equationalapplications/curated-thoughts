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

use std::collections::HashSet;
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
    /// stamped `source_type = 'user_confirmed'`.
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
    writeln!(f, "{}", serde_json::to_string(line)?)
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
    std::fs::write(&path, out).with_context(|| format!("rewriting ledger {}", path.display()))?;
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
    append_line(
        vault_root,
        CONFIRMATIONS_LEDGER,
        &ConfirmationLine {
            fact_id: fact_id.to_string(),
            content_hash: content_hash.to_string(),
            by: by.to_string(),
            at: at.to_string(),
        },
    )?;
    // Upsert the brain-side stamp now (spec §3: the confirm updates the brain
    // row AND appends to the ledger). Matching is by content hash — fact_id is
    // advisory even here — so the live stamp and the rebuild stamp agree.
    let mut hashes = HashSet::new();
    hashes.insert(content_hash.to_string());
    apply_confirmations_by_hash(conn, &hashes)?;
    Ok(())
}

/// Stamp `source_type = 'user_confirmed'` on every non-deleted entry whose
/// current title+body hash is in `hashes`. Returns the number of rows stamped.
fn apply_confirmations_by_hash(conn: &Connection, hashes: &HashSet<String>) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, title, body FROM llm_wiki_entries WHERE deleted_at IS NULL",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    let mut stamped = 0;
    for (id, title, body) in rows {
        if hashes.contains(&confirmation_content_hash(&title, &body)) {
            conn.execute(
                "UPDATE llm_wiki_entries SET source_type = 'user_confirmed' WHERE id = ?1",
                params![id],
            )?;
            stamped += 1;
        }
    }
    Ok(stamped)
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
    let tx = conn.unchecked_transaction()?;
    let mut report = RebuildReport::default();
    let stated = vault_root.join(STATED_DIR);

    // 1. Confirmations → user_confirmed stamps (dedup by hash; duplicate
    //    appends of the same confirmation stamp the same row once).
    let confirmations: Vec<ConfirmationLine> = read_lines(&stated.join(CONFIRMATIONS_LEDGER))?;
    report.confirmations_read = confirmations.len();
    let hashes: HashSet<String> = confirmations.into_iter().map(|l| l.content_hash).collect();
    report.confirmations_applied = apply_confirmations_by_hash(&tx, &hashes)?;

    // 2. Attestations → stated_attestations, re-verifying current file hashes.
    tx.execute("DELETE FROM stated_attestations", [])?;
    let attestations: Vec<AttestationLine> = read_lines(&stated.join(ATTESTATIONS_LEDGER))?;
    report.attestations_read = attestations.len();
    for line in attestations {
        let file = vault_root.join(&line.document_path);
        let kept = std::fs::read(&file)
            .map(|bytes| hash_bytes(&bytes) == line.content_hash)
            .unwrap_or(false); // unreadable/missing file cannot be verified → dropped
        if kept {
            tx.execute(
                "INSERT OR REPLACE INTO stated_attestations
                     (document_path, content_hash, attested_by, attested_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![line.document_path, line.content_hash, line.by, line.at],
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
/// repopulate `deposit_attribution` keyed by vault-relative document path.
/// A file whose frontmatter is absent or invalid contributes no row (the file
/// wins, but an unreadable attribution is not one).
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
        let rel_path = rel.to_string_lossy().replace('\\', "/");
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
            params![rel_path, a.0, a.1, a.2, deposited_at],
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
                         WHERE document_path = 'notes/deposit.md'",
                        [],
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
        let rows: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT document_path FROM stated_attestations ORDER BY document_path")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(rows, vec!["notes/kept.md".to_string()]);
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
}
