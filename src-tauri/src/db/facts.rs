//! Manual fact CRUD from Brain mode entity pages — mirrors commit.rs write conventions
//! (ms timestamps on llm_wiki_entries, outbox rows, curated_entities touch).

use crate::db::commit::{
    fact_title_from_body, generate_llm_id, now_timestamps, push_entries_outbox,
    wiki_fact_outbox_payload,
};
use crate::db::entities::EntityFact;
use crate::db::outbox_format::OutboxOperation;
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

/// source_ref for user-authored facts: same JSON shape as proposal commits, no evidence.
const MANUAL_SOURCE_REF: &str = r#"{"proposal_id":null,"evidence":[]}"#;

fn assert_entity_active(conn: &Connection, entity_id: &str) -> Result<()> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM curated_entities WHERE id = ?1 AND deleted_at IS NULL",
            [entity_id],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_none() {
        bail!("entity not found or archived: {entity_id}");
    }
    Ok(())
}

fn touch_entity(conn: &Connection, entity_id: &str, now_secs: i64) -> Result<()> {
    conn.execute(
        "UPDATE curated_entities SET updated_at = ?1 WHERE id = ?2",
        params![now_secs, entity_id],
    )?;
    Ok(())
}

/// Insert a user-authored fact with outbox row; returns the new fact.
pub fn add_fact(conn: &mut Connection, entity_id: &str, body: &str) -> Result<EntityFact> {
    let body = body.trim();
    if body.is_empty() {
        bail!("fact body must not be empty");
    }
    let (now_secs, now_ms) = now_timestamps();
    let fact_id = generate_llm_id("fact_");
    let title = fact_title_from_body(body);

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    assert_entity_active(&tx, entity_id)?;
    tx.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embedding
         ) VALUES (?1, ?2, ?3, ?4, '[]', 'confirmed', 'user_stated', NULL, ?5, ?6, ?6, NULL, 0, NULL, NULL, NULL)",
        params![fact_id, entity_id, title, body, MANUAL_SOURCE_REF, now_ms],
    )?;
    push_entries_outbox(
        &tx,
        entity_id,
        &fact_id,
        OutboxOperation::Insert,
        wiki_fact_outbox_payload(
            &fact_id,
            entity_id,
            &title,
            body,
            &[],
            "confirmed",
            "user_stated",
            None,
            MANUAL_SOURCE_REF,
            None,
            None,
            None,
            None,
            now_ms,
            now_ms,
            None,
            // Manual Brain-mode inserts start without OKF provenance;
            // the OKF v0.2 fields default to null until something
            // explicitly populates them (import, verified annotation, etc.).
            None,
            None,
            None,
            None,
            None,
        ),
        now_ms,
    )?;
    touch_entity(&tx, entity_id, now_secs)?;
    tx.commit()?;

    Ok(EntityFact {
        id: fact_id,
        title,
        body: body.to_string(),
        tags: Vec::new(),
        confidence: "confirmed".into(),
        source_type: "user_stated".into(),
        source_docs: Vec::new(),
        updated_at: now_ms,
        lifecycle_status: "stable".into(),
        stale_after: None,
        generated_by: None,
        okf_sources: Vec::new(),
        okf_verified: Vec::new(),
        okf_usage_window: None,
        last_verified_at: None,
        last_verified_by: None,
    })
}

/// Rewrite a fact's body (title re-derived); pushes full-payload outbox UPDATE.
pub fn update_fact(conn: &mut Connection, entity_id: &str, fact_id: &str, body: &str) -> Result<()> {
    let body = body.trim();
    if body.is_empty() {
        bail!("fact body must not be empty");
    }
    let (now_secs, now_ms) = now_timestamps();
    let title = fact_title_from_body(body);

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    assert_entity_active(&tx, entity_id)?;
    let existing = tx
        .query_row(
            "SELECT tags, confidence, source_type, COALESCE(source_ref, ''), created_at,
                    source_hash, okf_type, okf_sources, okf_verified, okf_usage_window,
                    lifecycle_status, stale_after, generated_by,
                    last_verified_at, last_verified_by
             FROM llm_wiki_entries
             WHERE id = ?1 AND entity_id = ?2 AND deleted_at IS NULL",
            params![fact_id, entity_id],
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
        bail!("fact not found or archived: {fact_id}");
    };

    tx.execute(
        "UPDATE llm_wiki_entries SET title = ?1, body = ?2, updated_at = ?3 WHERE id = ?4",
        params![title, body, now_ms, fact_id],
    )?;
    let tags: Vec<String> = serde_json::from_str(&tags_raw).unwrap_or_default();
    push_entries_outbox(
        &tx,
        entity_id,
        fact_id,
        OutboxOperation::Update,
        wiki_fact_outbox_payload(
            fact_id,
            entity_id,
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
    touch_entity(&tx, entity_id, now_secs)?;
    tx.commit()?;
    Ok(())
}

/// Soft-delete a fact; pushes minimal outbox DELETE (same shape as commit_fact_archive).
pub fn archive_fact(conn: &mut Connection, entity_id: &str, fact_id: &str) -> Result<()> {
    let (now_secs, now_ms) = now_timestamps();

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    assert_entity_active(&tx, entity_id)?;
    let changes = tx.execute(
        "UPDATE llm_wiki_entries
         SET deleted_at = ?1, updated_at = ?1
         WHERE id = ?2 AND entity_id = ?3 AND deleted_at IS NULL",
        params![now_ms, fact_id, entity_id],
    )?;
    if changes == 0 {
        bail!("fact not found or already archived: {fact_id}");
    }
    push_entries_outbox(
        &tx,
        entity_id,
        fact_id,
        OutboxOperation::Delete,
        serde_json::json!({
            "id": fact_id,
            "entity_id": entity_id,
            "deleted_at": now_ms,
        }),
        now_ms,
    )?;
    touch_entity(&tx, entity_id, now_secs)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::db::entities::{create_entity, get_entity, CreateEntityInput};

    fn make_entity(conn: &Connection) -> String {
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
    fn add_fact_inserts_row_outbox_and_touches_entity() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        conn.execute(
            "UPDATE curated_entities SET updated_at = 1 WHERE id = ?1",
            [&entity_id],
        )
        .unwrap();

        let fact = add_fact(&mut conn, &entity_id, "  The subject ships on Fridays.  ").unwrap();
        assert!(fact.id.starts_with("fact_"));
        assert_eq!(fact.body, "The subject ships on Fridays.");
        assert_eq!(fact.title, "The subject ships on Fridays.");
        assert_eq!(fact.confidence, "confirmed");
        assert_eq!(fact.source_type, "user_stated");

        let loaded = get_entity(&conn, &entity_id).unwrap().unwrap();
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(outbox_count(&conn, &fact.id, "INSERT"), 1);
        assert!(loaded.updated_at > 1, "entity updated_at must be touched");
    }

    #[test]
    fn add_fact_rejects_empty_body_and_missing_entity() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        assert!(add_fact(&mut conn, &entity_id, "   ").is_err());
        assert!(add_fact(&mut conn, "ent_missing", "Body").is_err());
    }

    #[test]
    fn update_fact_rewrites_body_and_pushes_outbox_update() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        let fact = add_fact(&mut conn, &entity_id, "Old body.").unwrap();

        update_fact(&mut conn, &entity_id, &fact.id, "New body with more detail.").unwrap();

        let loaded = get_entity(&conn, &entity_id).unwrap().unwrap();
        assert_eq!(loaded.facts[0].body, "New body with more detail.");
        assert_eq!(loaded.facts[0].title, "New body with more detail.");
        assert_eq!(outbox_count(&conn, &fact.id, "UPDATE"), 1);
    }

    #[test]
    fn update_fact_rejects_unknown_or_archived_fact() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        assert!(update_fact(&mut conn, &entity_id, "fact_missing", "x").is_err());
        let fact = add_fact(&mut conn, &entity_id, "Body.").unwrap();
        archive_fact(&mut conn, &entity_id, &fact.id).unwrap();
        assert!(update_fact(&mut conn, &entity_id, &fact.id, "x").is_err());
    }

    #[test]
    fn archive_fact_soft_deletes_and_pushes_outbox_delete() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        let fact = add_fact(&mut conn, &entity_id, "Ephemeral.").unwrap();

        archive_fact(&mut conn, &entity_id, &fact.id).unwrap();

        let loaded = get_entity(&conn, &entity_id).unwrap().unwrap();
        assert!(loaded.facts.is_empty(), "archived fact must not be listed");
        assert_eq!(outbox_count(&conn, &fact.id, "DELETE"), 1);
        assert!(archive_fact(&mut conn, &entity_id, &fact.id).is_err(), "double archive errors");
    }
}
