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
///
/// Equivalent to `add_fact_with_profile(conn, entity_id, body, None)` — the
/// entry lands with a NULL embedding for the sweep to fill.
pub fn add_fact(conn: &mut Connection, entity_id: &str, body: &str) -> Result<EntityFact> {
    add_fact_with_profile(conn, entity_id, body, None)
}

/// Insert a user-authored fact, optionally embedding it at write time.
///
/// The embedding is computed BEFORE the transaction opens — `embed_batch` is a
/// blocking network call and must never run under a write lock. A failure
/// leaves the blob NULL and the fact still commits: curation is durable, the
/// embedding is a derived artifact the sweep retries.
pub fn add_fact_with_profile(
    conn: &mut Connection,
    entity_id: &str,
    body: &str,
    profile: Option<&crate::embedder::EmbedProfile>,
) -> Result<EntityFact> {
    let body = body.trim();
    if body.is_empty() {
        bail!("fact body must not be empty");
    }
    let (now_secs, now_ms) = now_timestamps();
    let fact_id = generate_llm_id("fact_");
    let title = fact_title_from_body(body);

    // Outside the transaction, deliberately.
    let embedding_blob: Option<Vec<u8>> = profile.and_then(|p| {
        let text = crate::embed_sweep::embed_text_for_entry(&title, body);
        match crate::embedder::embed_batch(p, vec![text]) {
            Ok(vectors) => vectors
                .first()
                .map(|v| crate::wiki_graph::f32_vec_to_blob(v)),
            Err(e) => {
                eprintln!("add_fact: entry embedding failed, leaving NULL for the sweep: {e}");
                None
            }
        }
    });

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    assert_entity_active(&tx, entity_id)?;
    tx.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embedding
         ) VALUES (?1, ?2, ?3, ?4, '[]', 'confirmed', 'user_stated', NULL, ?5, ?6, ?6, NULL, 0, NULL, ?7, NULL)",
        params![fact_id, entity_id, title, body, MANUAL_SOURCE_REF, now_ms, embedding_blob],
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
            // `lifecycle_status` defaults to "stable" so outbox consumers
            // reconstruct the persisted lifecycle state without an extra
            // round-trip to the database.
            Some("stable"),
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
pub fn update_fact(
    conn: &mut Connection,
    entity_id: &str,
    fact_id: &str,
    body: &str,
) -> Result<()> {
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

    // Wipe the blob so the sweep re-derives it; mirrors `commit_fact_update` (commit 207477a).
    tx.execute(
        "UPDATE llm_wiki_entries
            SET title = ?1, body = ?2, updated_at = ?3, embedding_blob = NULL
          WHERE id = ?4",
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

    // Edges die with their endpoints, inside this same transaction (spec §2).
    crate::db::edge_purge::purge_edges_for_entry(&tx, fact_id)?;

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

    // -------------------------------------------------------------------------
    // add_fact_with_profile tests (Task 10)
    // -------------------------------------------------------------------------

    #[test]
    fn add_fact_with_profile_stores_an_embedding() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let mut conn = open_in_memory().unwrap();
            let entity_id = make_entity(&conn);
            let profile = crate::embedder::EmbedProfile::default();

            let fact =
                add_fact_with_profile(&mut conn, &entity_id, "A user-stated fact.", Some(&profile))
                    .unwrap();

            let blob_len: Option<i64> = conn
                .query_row(
                    "SELECT length(embedding_blob) FROM llm_wiki_entries WHERE id = ?1",
                    [&fact.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(blob_len, Some(32));
        });
    }

    #[test]
    fn add_fact_without_a_profile_leaves_the_blob_null() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);

        let fact = add_fact(&mut conn, &entity_id, "A user-stated fact.").unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = ?1",
                [&fact.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(blob, None, "the sweep fills it later");
    }

    // -------------------------------------------------------------------------
    // pre-existing tests
    // -------------------------------------------------------------------------

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

        // Outbox payload must carry the persisted lifecycle_status so a
        // consumer that reconstructs records from insert events does not
        // lose it.
        let payload_lifecycle: String = conn
            .query_row(
                "SELECT json_extract(payload, '$.lifecycle_status')
                 FROM llm_wiki_outbox
                 WHERE record_id = ?1 AND table_name = 'entries' AND operation = 'INSERT'",
                [&fact.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(payload_lifecycle, "stable");
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

        update_fact(
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
    fn update_fact_clears_embedding_blob_so_sweep_rederives_it() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let mut conn = open_in_memory().unwrap();
            let entity_id = make_entity(&conn);
            let profile = crate::embedder::EmbedProfile::default();

            // Seed a fact with a real (non-NULL) embedding blob.
            let fact = add_fact_with_profile(
                &mut conn,
                &entity_id,
                "Original body.",
                Some(&profile),
            )
            .unwrap();
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
            update_fact(&mut conn, &entity_id, &fact.id, "Edited body.").unwrap();

            let blob_after: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT embedding_blob FROM llm_wiki_entries WHERE id = ?1",
                    [&fact.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                blob_after, None,
                "update_fact must NULL embedding_blob so the sweep re-derives it",
            );
        });
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
        assert!(
            archive_fact(&mut conn, &entity_id, &fact.id).is_err(),
            "double archive errors"
        );
    }

    #[test]
    fn archive_fact_purges_edges_touching_the_fact() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        let fact = add_fact(&mut conn, &entity_id, "The archived fact body.").unwrap();
        let other = add_fact(&mut conn, &entity_id, "The surviving fact body.").unwrap();

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_out', ?1, ?2, ?3, 'related_to', 100)",
            params![entity_id, fact.id, other.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_in', ?1, ?2, ?3, 'related_to', 100)",
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

        archive_fact(&mut conn, &entity_id, &fact.id).unwrap();

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "both edges touching the archived fact must go");
    }

    #[test]
    fn archive_fact_leaves_unrelated_edges_alone() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        let fact = add_fact(&mut conn, &entity_id, "The archived fact body.").unwrap();
        let b = add_fact(&mut conn, &entity_id, "Fact B body.").unwrap();
        let c = add_fact(&mut conn, &entity_id, "Fact C body.").unwrap();

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_bc', ?1, ?2, ?3, 'related_to', 100)",
            params![entity_id, b.id, c.id],
        )
        .unwrap();

        archive_fact(&mut conn, &entity_id, &fact.id).unwrap();

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "an edge between two live facts must survive");
    }
}
