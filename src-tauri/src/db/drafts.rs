//! Draft promotion with outbox replication (spec CT-REQ-DRAFT-01).
//!
//! Upstream `WikiMemory.promoteDraft` updates `lifecycle_status` and
//! `okf_verified` without an outbox row (DAO discipline). CT's outbox
//! replicas carry both fields, so promotion runs here instead: status, trust
//! append and outbox push commit in one transaction.

use crate::db::commit::{push_entries_outbox, wiki_fact_outbox_payload};
use crate::db::outbox_format::OutboxOperation;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Reviewer actor recorded on promotion. The `human:` prefix is what makes
/// core derive trust tier `human-reviewed`.
pub const LOCAL_REVIEWER: &str = "human:local";

#[derive(Debug, PartialEq, Eq)]
pub enum PromoteDraftError {
    NotFound,
    NotDraft,
}

impl std::fmt::Display for PromoteDraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFound => "not_found",
            Self::NotDraft => "not_draft",
        })
    }
}

impl std::error::Error for PromoteDraftError {}

pub fn promote_draft(
    conn: &Connection,
    entry_id: &str,
    entity_id: &str,
    now_ms: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    let status: Option<String> = tx
        .query_row(
            "SELECT lifecycle_status FROM llm_wiki_entries
             WHERE id = ?1 AND entity_id = ?2 AND deleted_at IS NULL",
            params![entry_id, entity_id],
            |r| r.get(0),
        )
        .optional()?;
    match status.as_deref() {
        None => return Err(PromoteDraftError::NotFound.into()),
        Some("draft") => {}
        Some(_) => return Err(PromoteDraftError::NotDraft.into()),
    }

    let at_iso = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
        .ok_or_else(|| anyhow!("promote_draft: timestamp out of range: {now_ms}"))?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    // Same shape as core's writeOkfTrust: append {by, at}, and mirror the
    // latest reviewer into last_verified_by/at (ms). updated_at untouched.
    tx.execute(
        "UPDATE llm_wiki_entries
            SET lifecycle_status = 'stable',
                okf_verified = json_insert(
                    COALESCE(NULLIF(okf_verified, ''), '[]'),
                    '$[#]',
                    json_object('by', ?3, 'at', ?4)
                ),
                last_verified_by = ?3,
                last_verified_at = ?5
          WHERE id = ?1 AND entity_id = ?2",
        params![entry_id, entity_id, LOCAL_REVIEWER, at_iso, now_ms],
    )?;

    let payload = tx.query_row(
        "SELECT title, body, tags, confidence, source_type, source_hash,
                COALESCE(source_ref, ''), okf_type, okf_sources, okf_verified,
                okf_usage_window, created_at, updated_at, deleted_at,
                lifecycle_status, stale_after, generated_by,
                last_verified_at, last_verified_by
           FROM llm_wiki_entries WHERE id = ?1 AND entity_id = ?2",
        params![entry_id, entity_id],
        |r| {
            let tags_json: Option<String> = r.get(2)?;
            let tags: Vec<String> = tags_json
                .as_deref()
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or_default();
            let title: String = r.get(0)?;
            let body: String = r.get(1)?;
            let confidence: String = r.get(3)?;
            let source_type: String = r.get(4)?;
            let source_hash: Option<String> = r.get(5)?;
            let source_ref: String = r.get(6)?;
            let okf_type: Option<String> = r.get(7)?;
            let okf_sources: Option<String> = r.get(8)?;
            let okf_verified: Option<String> = r.get(9)?;
            let okf_usage_window: Option<String> = r.get(10)?;
            let lifecycle_status: Option<String> = r.get(14)?;
            let generated_by: Option<String> = r.get(16)?;
            let last_verified_by: Option<String> = r.get(18)?;
            Ok(wiki_fact_outbox_payload(
                entry_id,
                entity_id,
                &title,
                &body,
                &tags,
                &confidence,
                &source_type,
                source_hash.as_deref(),
                &source_ref,
                okf_type.as_deref(),
                okf_sources.as_deref(),
                okf_verified.as_deref(),
                okf_usage_window.as_deref(),
                r.get(11)?,
                r.get(12)?,
                r.get(13)?,
                lifecycle_status.as_deref(),
                r.get(15)?,
                generated_by.as_deref(),
                r.get(17)?,
                last_verified_by.as_deref(),
            ))
        },
    )?;

    push_entries_outbox(
        &tx,
        entity_id,
        entry_id,
        OutboxOperation::Update,
        payload,
        now_ms,
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn seed(conn: &Connection, id: &str, entity_id: &str, status: &str, deleted_at: Option<i64>) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding,
                lifecycle_status, okf_verified
             ) VALUES (?1, ?2, 'Title', 'Body', '[\"a\"]', 'inferred', 'immutable_document',
                       NULL, 'documents/x.md', 100, 200, NULL, 0, ?4, NULL, NULL,
                       ?3, '[{\"by\":\"agent:librarian\",\"at\":\"2026-01-01T00:00:00.000Z\"}]')",
            params![id, entity_id, status, deleted_at],
        )
        .unwrap();
    }

    fn outbox_rows(conn: &Connection, record_id: &str) -> Vec<(String, String, serde_json::Value)> {
        let mut stmt = conn
            .prepare(
                "SELECT table_name, operation, payload FROM llm_wiki_outbox WHERE record_id = ?1",
            )
            .unwrap();
        stmt.query_map([record_id], |r| {
            let payload: String = r.get(2)?;
            Ok((
                r.get(0)?,
                r.get(1)?,
                serde_json::from_str(&payload).unwrap(),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    }

    const NOW: i64 = 1_758_500_000_000;

    #[test]
    fn promotes_draft_appends_trust_and_pushes_update_outbox_row() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "fact_1", "tier_fact", "draft", None);

        promote_draft(&conn, "fact_1", "tier_fact", NOW).unwrap();

        let (status, verified, by, at, updated_at): (String, String, String, i64, i64) = conn
            .query_row(
                "SELECT lifecycle_status, okf_verified, last_verified_by, last_verified_at, updated_at
                 FROM llm_wiki_entries WHERE id = 'fact_1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(status, "stable");
        let verified: serde_json::Value = serde_json::from_str(&verified).unwrap();
        assert_eq!(
            verified.as_array().unwrap().len(),
            2,
            "existing entry preserved"
        );
        assert_eq!(verified[1]["by"], LOCAL_REVIEWER);
        assert_eq!(verified[1]["at"], "2025-09-22T00:13:20.000Z");
        assert_eq!(by, LOCAL_REVIEWER);
        assert_eq!(at, NOW);
        assert_eq!(updated_at, 200, "updated_at is not bumped");

        let rows = outbox_rows(&conn, "fact_1");
        assert_eq!(rows.len(), 1);
        let (table, op, payload) = &rows[0];
        assert_eq!(table, "entries");
        assert_eq!(op, "UPDATE");
        assert_eq!(payload["lifecycle_status"], "stable");
        assert_eq!(payload["last_verified_by"], LOCAL_REVIEWER);
        assert_eq!(payload["tags"], serde_json::json!(["a"]));
        let payload_verified: serde_json::Value =
            serde_json::from_str(payload["okf_verified"].as_str().unwrap()).unwrap();
        assert_eq!(payload_verified[1]["by"], LOCAL_REVIEWER);
    }

    #[test]
    fn missing_or_deleted_or_foreign_entry_is_not_found_and_writes_nothing() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "fact_del", "tier_fact", "draft", Some(150));
        seed(&conn, "fact_other", "tier_wisdom", "draft", None);

        for (id, entity) in [
            ("nope", "tier_fact"),
            ("fact_del", "tier_fact"),
            ("fact_other", "tier_fact"),
        ] {
            let err = promote_draft(&conn, id, entity, NOW).unwrap_err();
            assert_eq!(
                err.downcast_ref::<PromoteDraftError>(),
                Some(&PromoteDraftError::NotFound)
            );
            assert!(outbox_rows(&conn, id).is_empty());
        }
    }

    #[test]
    fn stable_entry_is_not_draft_and_writes_nothing() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "fact_s", "tier_fact", "stable", None);

        let err = promote_draft(&conn, "fact_s", "tier_fact", NOW).unwrap_err();
        assert_eq!(err.to_string(), "not_draft");
        let verified: String = conn
            .query_row(
                "SELECT okf_verified FROM llm_wiki_entries WHERE id='fact_s'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&verified)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(outbox_rows(&conn, "fact_s").is_empty());
    }
}
