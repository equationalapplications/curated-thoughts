//! One-shot repair of `llm_wiki_entries.source_ref` blobs mangled by the JS
//! engine's setup-time rewrite (issue #186).
//!
//! Detection is a **positive token-shape test**: a `librarian_inferred` row is
//! damaged iff its `source_ref` is not already the new token. That asks "is
//! this the new shape" rather than "does it look mangled", so it stays correct
//! regardless of the mangled blobs' internal layout. Spec §2.5.1.
//!
//! Every query here carries `source_type = 'librarian_inferred'`. A legitimate
//! document-sourced ref can itself be exactly 255 chars (long vault paths
//! normalize to the cap), so shape/length heuristics without that predicate
//! would classify good rows as damaged and delete them.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

/// GLOB matching the normative token shape `^librarian-[0-9a-f]{32}$`
/// (spec §2.2). SQLite GLOB has no repetition operator, so the 32 hex
/// positions are spelled out.
pub const TOKEN_GLOB: &str = "librarian-\
[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]\
[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]\
[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]\
[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RepairCensus {
    /// Rows needing repair: librarian_inferred, non-NULL, not already a token.
    pub damaged: i64,
    /// NULL refs — engine-era data, counted for visibility only, never touched.
    pub null_ref: i64,
    /// Damaged rows whose ref still parses as JSON (evidence survived intact).
    pub valid_json: i64,
    /// Mangled empty-evidence shape; `proposal_id` survived truncation.
    pub shape_proposal_id: i64,
    /// Mangled non-empty shape; `proposal_id` was truncated away.
    pub shape_chunk_id: i64,
    /// Damaged rows whose pristine payload is still in `llm_wiki_outbox`.
    pub outbox_recoverable: i64,
}

fn count(conn: &Connection, sql: &str) -> Result<i64> {
    Ok(conn
        .query_row(sql, [], |r| r.get(0))
        .optional()?
        .unwrap_or(0))
}

pub fn repair_census(conn: &Connection) -> Result<RepairCensus> {
    let damaged_scope = format!(
        "FROM llm_wiki_entries
          WHERE source_type = 'librarian_inferred'
            AND source_ref IS NOT NULL
            AND source_ref NOT GLOB '{TOKEN_GLOB}'"
    );
    Ok(RepairCensus {
        damaged: count(conn, &format!("SELECT COUNT(*) {damaged_scope}"))?,
        null_ref: count(
            conn,
            "SELECT COUNT(*) FROM llm_wiki_entries
              WHERE source_type = 'librarian_inferred' AND source_ref IS NULL",
        )?,
        valid_json: count(
            conn,
            &format!("SELECT COUNT(*) {damaged_scope} AND json_valid(source_ref)"),
        )?,
        shape_proposal_id: count(
            conn,
            &format!("SELECT COUNT(*) {damaged_scope} AND source_ref LIKE 'evidenceproposal_id%'"),
        )?,
        shape_chunk_id: count(
            conn,
            &format!("SELECT COUNT(*) {damaged_scope} AND source_ref LIKE 'evidencechunk_id%'"),
        )?,
        outbox_recoverable: count(
            conn,
            &format!(
                "SELECT COUNT(*) FROM llm_wiki_entries e
                   JOIN llm_wiki_outbox o
                     ON o.record_id = e.id
                    AND o.table_name = 'entries'
                    AND o.operation = 'INSERT'
                  WHERE e.source_type = 'librarian_inferred'
                    AND e.source_ref IS NOT NULL
                    AND e.source_ref NOT GLOB '{TOKEN_GLOB}'
                    AND json_valid(json_extract(o.payload, '$.source_ref'))"
            ),
        )?,
    })
}

/// Keys emitted by `evidence_json_with_hashes` (commit.rs:628-641), in the
/// order serde_json writes them — **alphabetically**, because
/// `serde_json::Map` is a `BTreeMap` unless `preserve_order` is active and it
/// is not active for this crate's runtime graph. That is why every mangled
/// blob begins `evidence`, and why `proposal_id` sorts last. Spec §2.5.4.
const EVIDENCE_KEYS: &[&str] = &[
    "chunk_id",
    "content_hash",
    "end_line",
    "quote",
    "source_kind",
    "start_line",
];

/// Path 4b: `{"evidence":[],"proposal_id":"prop_…"}` mangles to
/// `evidenceproposal_idprop_<24hex>`. Short enough that the 255-char cap never
/// truncated it, so the id is recoverable verbatim.
pub fn extract_proposal_id_from_empty_shape(mangled: &str) -> Option<String> {
    let rest = mangled.strip_prefix("evidenceproposal_id")?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if id.is_empty() || !id.starts_with("prop_") {
        return None;
    }
    Some(id)
}

/// Path 4c: for non-empty evidence, `proposal_id` sorted last, sat at the tail
/// of the blob, and was truncated away by `.slice(0, 255)`. What survives in
/// the head is the first evidence item's `content_hash` — the join key back to
/// `curated_proposal_items.evidence`. Preferred over `chunk_id`, which is a
/// legacy rowid and may be absent.
pub fn extract_leading_content_hash(mangled: &str) -> Option<String> {
    let idx = mangled.find("content_hash")?;
    let rest = &mangled[idx + "content_hash".len()..];
    let hash: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    // Stop before a following key token bleeds in (hex-only already excludes
    // every key in EVIDENCE_KEYS except a pathological all-hex prefix).
    debug_assert!(EVIDENCE_KEYS.iter().all(|k| !hash.starts_with(k)));
    if hash.is_empty() {
        None
    } else {
        Some(hash)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RepairReport {
    pub from_outbox: usize,
    pub from_valid_json: usize,
    pub from_proposal_id: usize,
    pub from_content_hash: usize,
    pub deleted: usize,
    pub ambiguous: usize,
}

/// Resolve the owning proposal for a `content_hash` recovered from a truncated
/// ref. Ambiguity (a hash appearing in several proposals) is broken by
/// proximity in `created_at` and reported, never silently picked.
fn proposal_for_content_hash(
    conn: &Connection,
    hash: &str,
    entry_created_at: i64,
) -> Result<(Option<String>, bool)> {
    let mut stmt = conn.prepare(
        "SELECT i.proposal_id, p.created_at
           FROM curated_proposal_items i
           JOIN curated_proposals p ON p.id = i.proposal_id
          WHERE i.evidence LIKE '%' || ?1 || '%'",
    )?;
    let mut rows: Vec<(String, i64)> = stmt
        .query_map([hash], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(Result::ok)
        .collect();
    rows.dedup_by(|a, b| a.0 == b.0);
    let ambiguous = rows.len() > 1;
    rows.sort_by_key(|(_, created)| (created - entry_created_at).abs());
    Ok((rows.first().map(|(id, _)| id.clone()), ambiguous))
}

/// Rebuild an evidence blob from every item of a proposal.
///
/// The entry→item mapping was destroyed by the mangling, so the reconstruction
/// rule is to attach the proposal's FULL item evidence to each surviving entry.
/// The result may not byte-equal the original per-entry blob — acceptable:
/// grounding re-checks chunk existence at repair time and superseding is
/// per-entry. Spec §2.5.4.
fn rebuild_evidence_for_proposal(conn: &Connection, proposal_id: &str) -> Result<Option<String>> {
    let mut stmt =
        conn.prepare("SELECT evidence FROM curated_proposal_items WHERE proposal_id = ?1")?;
    let mut merged: Vec<serde_json::Value> = Vec::new();
    for raw in stmt
        .query_map([proposal_id], |r| r.get::<_, String>(0))?
        .filter_map(Result::ok)
    {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(&raw) {
            merged.extend(items);
        }
    }
    if merged.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::json!({ "proposal_id": proposal_id, "evidence": merged }).to_string(),
    ))
}

/// One-shot repair, run inside the V18 migration step. Idempotent: rows that
/// already carry the token are out of scope, so a re-run is a no-op.
pub fn run_evidence_repair(conn: &Connection, now_ms: i64) -> Result<RepairReport> {
    let mut report = RepairReport::default();

    let damaged: Vec<(String, String, i64)> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT id, source_ref, created_at FROM llm_wiki_entries
              WHERE source_type = 'librarian_inferred'
                AND source_ref IS NOT NULL
                AND source_ref NOT GLOB '{TOKEN_GLOB}'"
        ))?;
        let damaged: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(Result::ok)
            .collect();
        damaged
    };

    for (entry_id, mangled, created_at) in damaged {
        // 4a. Outbox-first: the pristine, untruncated payload if it survived.
        let from_outbox: Option<String> = conn
            .query_row(
                "SELECT json_extract(payload, '$.source_ref') FROM llm_wiki_outbox
                  WHERE record_id = ?1 AND table_name = 'entries' AND operation = 'INSERT'
                  ORDER BY created_at DESC LIMIT 1",
                [&entry_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .unwrap_or(None)
            .flatten()
            .filter(|s| serde_json::from_str::<serde_json::Value>(s).is_ok());

        let (evidence_json, proposal_id, bucket) = if let Some(pristine) = from_outbox {
            let pid = serde_json::from_str::<serde_json::Value>(&pristine)
                .ok()
                .and_then(|v| {
                    v.get("proposal_id")
                        .and_then(|p| p.as_str())
                        .map(String::from)
                });
            match pid {
                Some(pid) => (Some(pristine), Some(pid), 0),
                None => (None, None, 4),
            }
        } else if serde_json::from_str::<serde_json::Value>(&mangled).is_ok() {
            // Valid JSON survived: migrate verbatim, never re-derive.
            let pid = serde_json::from_str::<serde_json::Value>(&mangled)
                .ok()
                .and_then(|v| {
                    v.get("proposal_id")
                        .and_then(|p| p.as_str())
                        .map(String::from)
                });
            match pid {
                Some(pid) => (Some(mangled.clone()), Some(pid), 1),
                None => (None, None, 4),
            }
        } else if let Some(pid) = extract_proposal_id_from_empty_shape(&mangled) {
            (rebuild_evidence_for_proposal(conn, &pid)?, Some(pid), 2)
        } else if let Some(hash) = extract_leading_content_hash(&mangled) {
            let (pid, ambiguous) = proposal_for_content_hash(conn, &hash, created_at)?;
            if ambiguous {
                report.ambiguous += 1;
            }
            match pid {
                Some(pid) => (rebuild_evidence_for_proposal(conn, &pid)?, Some(pid), 3),
                None => (None, None, 4),
            }
        } else {
            (None, None, 4)
        };

        match (evidence_json, proposal_id) {
            (Some(json), Some(pid)) => {
                crate::db::commit::insert_librarian_evidence(
                    conn, &entry_id, &pid, &json, false, now_ms,
                )?;
                conn.execute(
                    "UPDATE llm_wiki_entries SET source_ref = ?1
                      WHERE id = ?2 AND source_type = 'librarian_inferred'",
                    rusqlite::params![
                        crate::db::commit::librarian_source_ref_token(&entry_id),
                        &entry_id
                    ],
                )?;
                match bucket {
                    0 => report.from_outbox += 1,
                    1 => report.from_valid_json += 1,
                    2 => report.from_proposal_id += 1,
                    _ => report.from_content_hash += 1,
                }
            }
            _ => {
                // Resolves by no path: export happens in Task 7 before this
                // runs; here the row and its evidence go together.
                crate::db::commit::delete_librarian_evidence(conn, &[entry_id.clone()])?;
                conn.execute(
                    "DELETE FROM llm_wiki_entries
                      WHERE id = ?1 AND source_type = 'librarian_inferred'",
                    [&entry_id],
                )?;
                report.deleted += 1;
            }
        }
    }

    Ok(report)
}

/// A supported export is **brain-complete**: entries, evidence, chunks and
/// proposals together. A partial export (entries without chunks) would make
/// legitimately-anchored facts look like orphans and the deletion path would
/// destroy good data.
///
/// Non-emptiness is required only for `chunks` and `documents`. Embedding
/// tables are legitimately empty on any brain whose embed sweep has not run —
/// requiring them would false-positive on healthy databases and block repair
/// forever in the fail-safe direction. Spec §2.5.4.
pub fn brain_is_complete(conn: &Connection) -> Result<bool> {
    for table in [
        "chunks",
        "documents",
        "curated_proposals",
        "curated_proposal_items",
    ] {
        let present: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .optional()?;
        if present.is_none() {
            return Ok(false);
        }
    }
    let has_chunk_refs = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM llm_wiki_entries
              WHERE source_type = 'librarian_inferred'
                AND source_ref IS NOT NULL
                AND source_ref NOT GLOB '{TOKEN_GLOB}'
                AND source_ref LIKE '%chunk_id%'"
        ),
    )? > 0;
    if !has_chunk_refs {
        return Ok(true);
    }
    Ok(count(conn, "SELECT COUNT(*) FROM chunks")? > 0
        && count(conn, "SELECT COUNT(*) FROM documents")? > 0)
}

/// Back up every damaged row to `<out_dir>/<entry_id>.json` before any
/// mutation. Spec §2.5.2.
pub fn export_damaged_rows(conn: &Connection, out_dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(out_dir)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, entity_id, title, body, source_ref, created_at
           FROM llm_wiki_entries
          WHERE source_type = 'librarian_inferred'
            AND source_ref IS NOT NULL
            AND source_ref NOT GLOB '{TOKEN_GLOB}'"
    ))?;
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "entity_id": r.get::<_, String>(1)?,
                "title": r.get::<_, String>(2)?,
                "body": r.get::<_, String>(3)?,
                "source_ref": r.get::<_, String>(4)?,
                "created_at": r.get::<_, i64>(5)?,
            }))
        })?
        .filter_map(Result::ok)
        .collect();
    for row in &rows {
        let id = row["id"].as_str().unwrap_or("unknown");
        std::fs::write(
            out_dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(row)?,
        )?;
    }
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::commit::librarian_source_ref_token;
    use crate::db::connection::open_in_memory;
    use rusqlite::Connection;

    fn seed_entry(conn: &Connection, id: &str, source_type: &str, source_ref: Option<&str>) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, access_count)
             VALUES (?1,'ent','t','b','[]','inferred',?2,?3,1,1,0)",
            rusqlite::params![id, source_type, source_ref],
        )
        .unwrap();
    }

    #[test]
    fn census_counts_shapes_and_ignores_healthy_token_rows() {
        let conn = open_in_memory().unwrap();
        seed_entry(
            &conn,
            "fact_ok",
            "librarian_inferred",
            Some(&librarian_source_ref_token("fact_ok")),
        );
        seed_entry(
            &conn,
            "fact_a",
            "librarian_inferred",
            Some("evidenceproposal_idprop_0123456789abcdef01234567"),
        );
        seed_entry(
            &conn,
            "fact_b",
            "librarian_inferred",
            Some("evidencechunk_id12content_hashdeadbeefquotehello"),
        );
        seed_entry(
            &conn,
            "fact_c",
            "librarian_inferred",
            Some(r#"{"evidence":[],"proposal_id":"prop_z"}"#),
        );
        seed_entry(&conn, "fact_n", "librarian_inferred", None);

        let c = repair_census(&conn).unwrap();
        assert_eq!(c.damaged, 3, "token row and NULL row are not damaged");
        assert_eq!(c.null_ref, 1);
        assert_eq!(c.valid_json, 1);
        assert_eq!(c.shape_proposal_id, 1);
        assert_eq!(c.shape_chunk_id, 1);
    }

    #[test]
    fn census_never_touches_document_sourced_rows() {
        let conn = open_in_memory().unwrap();
        // A legitimate document ref that itself hits the 255-char cap: the
        // CodeRabbit census gap, pinned as a regression. Spec §2.5.1.
        let long_path = "a".repeat(255);
        seed_entry(&conn, "fact_doc", "document", Some(&long_path));

        let c = repair_census(&conn).unwrap();
        assert_eq!(
            c.damaged, 0,
            "document-sourced rows are out of scope entirely"
        );
    }

    #[test]
    fn extracts_proposal_id_from_empty_evidence_shape() {
        // {"evidence":[],"proposal_id":"prop_<24 hex>"} mangles to this.
        let mangled = "evidenceproposal_idprop_0123456789abcdef01234567";
        assert_eq!(
            extract_proposal_id_from_empty_shape(mangled).as_deref(),
            Some("prop_0123456789abcdef01234567")
        );
        assert_eq!(
            extract_proposal_id_from_empty_shape("evidencechunk_id1"),
            None
        );
    }

    #[test]
    fn extracts_leading_content_hash_from_truncated_shape() {
        let mangled = "evidencechunk_id42content_hashdeadbeefcafe0123quotehello world";
        assert_eq!(
            extract_leading_content_hash(mangled).as_deref(),
            Some("deadbeefcafe0123")
        );
        assert_eq!(
            extract_leading_content_hash("evidenceproposal_idprop_x"),
            None
        );
    }

    #[test]
    fn repair_prefers_outbox_payload_verbatim() {
        let conn = open_in_memory().unwrap();
        seed_entry(
            &conn,
            "fact_o",
            "librarian_inferred",
            Some("evidencechunk_id1content_hashaa"),
        );
        let pristine =
            r#"{"evidence":[{"chunk_id":1,"content_hash":"aa"}],"proposal_id":"prop_o"}"#;
        conn.execute(
            "INSERT INTO llm_wiki_outbox (id, entity_id, table_name, record_id, operation,
                 payload, created_at)
             VALUES ('o1','ent','entries','fact_o','INSERT',?1,1)",
            [serde_json::json!({ "id": "fact_o", "source_ref": pristine }).to_string()],
        )
        .unwrap();

        let report = run_evidence_repair(&conn, 999).unwrap();
        assert_eq!(report.from_outbox, 1);

        let stored: String = conn
            .query_row(
                "SELECT evidence_json FROM librarian_evidence WHERE entry_id='fact_o'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, pristine, "outbox recovery must be byte-exact");

        let ref_after: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id='fact_o'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ref_after, librarian_source_ref_token("fact_o"));
    }

    #[test]
    fn repair_recovers_chunk_shape_via_content_hash() {
        let conn = open_in_memory().unwrap();
        // kind is constrained to ('new_entity','update_entity') by DDL.
        conn.execute(
            "INSERT INTO curated_proposals (id, kind, model, status, created_at)
             VALUES ('prop_h','new_entity','m','pending',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_items (id, proposal_id, item_type, payload, evidence)
             VALUES ('item_h','prop_h','fact_add','{}',
                     '[{\"chunk_id\":7,\"content_hash\":\"feedface00\"}]')",
            [],
        )
        .unwrap();
        seed_entry(
            &conn,
            "fact_h",
            "librarian_inferred",
            Some("evidencechunk_id7content_hashfeedface00quotex"),
        );

        let report = run_evidence_repair(&conn, 999).unwrap();
        assert_eq!(report.from_content_hash, 1);
        assert_eq!(report.deleted, 0);

        let proposal_id: String = conn
            .query_row(
                "SELECT proposal_id FROM librarian_evidence WHERE entry_id='fact_h'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(proposal_id, "prop_h");
    }

    #[test]
    fn repair_deletes_rows_that_resolve_by_no_path() {
        let conn = open_in_memory().unwrap();
        seed_entry(
            &conn,
            "fact_x",
            "librarian_inferred",
            Some("evidencechunk_id9content_hashnosuchhash00quote"),
        );

        let report = run_evidence_repair(&conn, 999).unwrap();
        assert_eq!(report.deleted, 1);
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id='fact_x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn brain_complete_requires_chunks_and_documents_but_not_embeddings() {
        let conn = open_in_memory().unwrap();
        // Empty chunks/documents on a DB with chunk-derived refs => incomplete.
        seed_entry(
            &conn,
            "fact_bc",
            "librarian_inferred",
            Some("evidencechunk_id1content_hashaa"),
        );
        assert!(!brain_is_complete(&conn).unwrap());

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES ('d.md','h','user_doc','indexed')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy,
                 entity_id, content_hash)
             VALUES (?1,'c',0,1,1,'prose','ent','aa')",
            [doc_id],
        )
        .unwrap();

        // Embeddings stay empty — legitimately so on any brain whose embed
        // sweep has not run. Requiring them would false-positive. Spec §2.5.4.
        assert!(brain_is_complete(&conn).unwrap());
    }

    #[test]
    fn export_writes_a_file_per_damaged_row_before_mutation() {
        let conn = open_in_memory().unwrap();
        seed_entry(
            &conn,
            "fact_e",
            "librarian_inferred",
            Some("evidencechunk_id1content_hashaa"),
        );
        let dir = tempfile::TempDir::new().unwrap();

        let n = export_damaged_rows(&conn, dir.path()).unwrap();
        assert_eq!(n, 1);
        let written = std::fs::read_to_string(dir.path().join("fact_e.json")).unwrap();
        assert!(written.contains("evidencechunk_id1content_hashaa"));
    }

    #[test]
    fn repair_is_idempotent() {
        let conn = open_in_memory().unwrap();
        seed_entry(
            &conn,
            "fact_i",
            "librarian_inferred",
            Some(r#"{"evidence":[],"proposal_id":"prop_i"}"#),
        );

        let first = run_evidence_repair(&conn, 999).unwrap();
        assert_eq!(first.from_valid_json, 1);
        let second = run_evidence_repair(&conn, 1000).unwrap();
        assert_eq!(
            second,
            RepairReport::default(),
            "re-running must be a no-op"
        );
    }
}
