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
}
