//! Manual wisdom CRUD from Brain mode entity pages — mirrors commit.rs write conventions
//! (ms timestamps on llm_wiki_entries, outbox rows, curated_entities touch).

use crate::db::commit::{
    fact_title_from_body, generate_llm_id, now_timestamps, push_entries_outbox,
    wiki_fact_outbox_payload,
};
use crate::db::entities::EntityWisdom;
use crate::db::outbox_format::OutboxOperation;
use crate::embedder::{embed_batch, EmbedProfile};
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

// User-authored wisdom carries NO source_ref: NULL is the "no provenance"
// value for manual facts. The old JSON sentinel
// `{"proposal_id":null,"evidence":[]}` was a live mangleable ref (review
// round 5, finding 1): the engine's setup rewrite collapsed it to the fixed
// string `proposal_idnullevidence` — identical for every manual row, violating
// the #186 invariant that a non-NULL source_ref must be a normalizer fixed
// point. NULL sits outside the engine's selector entirely. The V18 migration
// normalizes pre-existing sentinel/mangled rows on upgraded brains.

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

/// Insert a user-authored wisdom entry with outbox row; returns the new entry.
///
/// Equivalent to `add_wisdom_with_blob(conn, entity_id, body, None)` — the
/// entry lands with a NULL embedding for the sweep to fill.
pub fn add_wisdom(conn: &mut Connection, entity_id: &str, body: &str) -> Result<EntityWisdom> {
    add_wisdom_with_blob(conn, entity_id, body, None)
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

/// Insert a user-authored wisdom entry with an outbox row, taking a precomputed
/// embedding blob. Callers that want write-time embeddings must compute the
/// blob up front via [`precompute_entry_embedding`] — outside any lock.
pub fn add_wisdom_with_blob(
    conn: &mut Connection,
    entity_id: &str,
    body: &str,
    embedding_blob: Option<Vec<u8>>,
) -> Result<EntityWisdom> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let wisdom = add_wisdom_in_tx(&tx, entity_id, body, embedding_blob)?;
    tx.commit()?;
    Ok(wisdom)
}

/// Transaction-scoped core of [`add_wisdom_with_blob`]: inserts the entry and
/// its outbox row on the CALLER's transaction (no BEGIN, no COMMIT) so MCP
/// write tools can commit mutation + fail-closed audit atomically
/// (PR #185 review: an audit failure must roll the mutation back, not leave a
/// committed entry with no audit trail).
pub fn add_wisdom_in_tx(
    tx: &Transaction<'_>,
    entity_id: &str,
    body: &str,
    embedding_blob: Option<Vec<u8>>,
) -> Result<EntityWisdom> {
    let body = body.trim();
    if body.is_empty() {
        bail!("wisdom body must not be empty");
    }
    let (now_secs, now_ms) = now_timestamps();
    let wisdom_id = generate_llm_id("fact_");
    let title = fact_title_from_body(body);

    let entity_id = assert_entity_active(tx, entity_id)?;
    tx.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embedding
         ) VALUES (?1, ?2, ?3, ?4, '[]', 'confirmed', 'user_stated', NULL, NULL, ?5, ?5, NULL, 0, NULL, ?6, NULL)",
        params![wisdom_id, entity_id, title, body, now_ms, embedding_blob],
    )?;
    push_entries_outbox(
        tx,
        &entity_id,
        &wisdom_id,
        OutboxOperation::Insert,
        wiki_fact_outbox_payload(
            &wisdom_id,
            &entity_id,
            &title,
            body,
            &[],
            "confirmed",
            "user_stated",
            None,
            // NULL refs serialize as "" in the payload — the same convention
            // the update path already uses via COALESCE(source_ref, '') and
            // bundle apply via `.unwrap_or("")`.
            "",
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
    touch_entity(tx, &entity_id, now_secs)?;

    Ok(EntityWisdom {
        id: wisdom_id,
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

/// Insert a user-authored wisdom entry, computing the embedding blob inside the call.
/// New callers should compute the blob up front via
/// [`precompute_entry_embedding`] so the blocking network call does not run
/// under a write lock; this wrapper keeps the old test/library API.
pub fn add_wisdom_with_profile(
    conn: &mut Connection,
    entity_id: &str,
    body: &str,
    profile: Option<&EmbedProfile>,
) -> Result<EntityWisdom> {
    let embedding_blob = precompute_entry_embedding(profile, body);
    add_wisdom_with_blob(conn, entity_id, body, embedding_blob)
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
/// [`precompute_entry_embedding`], for the same reason as
/// [`add_wisdom_with_blob`]: `embed_batch` is a blocking network round-trip.
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
/// same atomic audit rationale as [`add_wisdom_in_tx`]).
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
    // entry no longer contains. Mirrors `commit_fact_update`.
    //
    // The lookup above matched anywhere in the survivor's redirect cluster,
    // so a pre-merge row may still be keyed to a loser — rekey it to the
    // survivor here so the persisted row and the outbox payload agree (the
    // #132 prisma-outbox divergence class `create_task` guards against).
    tx.execute(
        "UPDATE llm_wiki_entries
            SET title = ?1, body = ?2, updated_at = ?3, embedding_blob = ?4, entity_id = ?6
          WHERE id = ?5",
        params![title, body, now_ms, embedding_blob, wisdom_id, entity_id],
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
/// atomic audit rationale as [`add_wisdom_in_tx`]).
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
    use crate::db::connection::open_in_memory;
    use crate::db::entities::{create_entity, get_entity, CreateEntityInput};

    // -------------------------------------------------------------------------
    // add_wisdom_with_profile tests (Task 10)
    // -------------------------------------------------------------------------

    #[test]
    fn add_wisdom_with_profile_stores_an_embedding() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let mut conn = open_in_memory().unwrap();
            let entity_id = make_entity(&mut conn);
            let profile = crate::embedder::EmbedProfile::default();

            let fact = add_wisdom_with_profile(
                &mut conn,
                &entity_id,
                "A user-stated fact.",
                Some(&profile),
            )
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
    fn add_wisdom_without_a_profile_leaves_the_blob_null() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);

        let fact = add_wisdom(&mut conn, &entity_id, "A user-stated fact.").unwrap();

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
    fn add_wisdom_inserts_row_outbox_and_touches_entity() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        conn.execute(
            "UPDATE curated_entities SET updated_at = 1 WHERE id = ?1",
            [&entity_id],
        )
        .unwrap();

        let fact = add_wisdom(&mut conn, &entity_id, "  The subject ships on Fridays.  ").unwrap();
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
    fn add_wisdom_writes_null_source_ref_not_a_mangleable_sentinel() {
        // Review round 5, finding 1: the old JSON sentinel
        // `{"proposal_id":null,"evidence":[]}` was a live mangleable ref —
        // the engine's setup rewrite collapsed it to the fixed string
        // `proposal_idnullevidence`, identical for every manual row. NULL is
        // the "no provenance" value and sits outside the engine's selector.
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);

        let fact = add_wisdom(&mut conn, &entity_id, "A user-stated fact.").unwrap();

        let source_ref: Option<String> = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = ?1",
                [&fact.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            source_ref, None,
            "user-authored wisdom must carry a NULL source_ref"
        );
    }

    #[test]
    fn add_wisdom_rejects_empty_body_and_missing_entity() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        assert!(add_wisdom(&mut conn, &entity_id, "   ").is_err());
        assert!(add_wisdom(&mut conn, "ent_missing", "Body").is_err());
    }

    #[test]
    fn update_wisdom_rewrites_body_and_pushes_outbox_update() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        let fact = add_wisdom(&mut conn, &entity_id, "Old body.").unwrap();

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
            let profile = crate::embedder::EmbedProfile::default();

            // Seed a fact with a real (non-NULL) embedding blob.
            let fact =
                add_wisdom_with_profile(&mut conn, &entity_id, "Original body.", Some(&profile))
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
        let fact = add_wisdom(&mut conn, &entity_id, "Body.").unwrap();
        archive_wisdom(&mut conn, &entity_id, &fact.id).unwrap();
        assert!(update_wisdom(&mut conn, &entity_id, &fact.id, "x").is_err());
    }

    #[test]
    fn archive_wisdom_soft_deletes_and_pushes_outbox_delete() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        let fact = add_wisdom(&mut conn, &entity_id, "Ephemeral.").unwrap();

        archive_wisdom(&mut conn, &entity_id, &fact.id).unwrap();

        let loaded = get_entity(&conn, &entity_id).unwrap().unwrap();
        assert!(loaded.facts.is_empty(), "archived fact must not be listed");
        assert_eq!(outbox_count(&conn, &fact.id, "DELETE"), 1);
        assert!(
            archive_wisdom(&mut conn, &entity_id, &fact.id).is_err(),
            "double archive errors"
        );
    }

    #[test]
    fn archive_wisdom_purges_edges_touching_the_fact() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&mut conn);
        let fact = add_wisdom(&mut conn, &entity_id, "The archived fact body.").unwrap();
        let other = add_wisdom(&mut conn, &entity_id, "The surviving fact body.").unwrap();

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
        let fact = add_wisdom(&mut conn, &entity_id, "The archived fact body.").unwrap();
        let b = add_wisdom(&mut conn, &entity_id, "Fact B body.").unwrap();
        let c = add_wisdom(&mut conn, &entity_id, "Fact C body.").unwrap();

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
        let fact = add_wisdom(&mut conn, &surv, "Pre-merge body.").unwrap();
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
        let fact = add_wisdom(&mut conn, &surv, "Pre-merge body.").unwrap();
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
}
