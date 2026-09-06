//! `run_wiki_forget` cascade helper.
//!
//! `forget_entries_by_source_refs` mirrors the pattern established in
//! `commit_fact_archive`, `archive_fact`, `heal_lost_librarian_inferred`, and
//! `prune_old_librarian_inferred`: it collects doomed entry ids BEFORE deleting
//! them, then purges their edges inside the same transaction so a crash between
//! the two can never mint an orphan.

use anyhow::Result;
use rusqlite::Connection;

/// Delete every entry whose `source_ref` matches one of the given values,
/// purging the edges that touch them in the same transaction.
///
/// Returns the number of entries deleted. An empty input list is a no-op
/// and returns 0. Edges are not replicated (spec §6) — this purge is
/// local-only, exactly like the inserts `commit_edge_add` issues without
/// an outbox push.
///
/// Entries **are** replicated: one `OutboxOperation::Delete` row is pushed per
/// deleted entry, inside the same transaction, so replicas converge on an
/// explicit erasure. Without it, downstream prisma-outbox replicas keep serving
/// facts the user ran "forget this source file" on (#132) — a privacy defect,
/// not just a consistency one.
pub fn forget_entries_by_source_refs(
    conn: &Connection,
    source_refs: &[String],
    now_ms: i64,
) -> Result<usize> {
    if source_refs.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    // Collect doomed ids AND their entity ids BEFORE the DELETE — once deleted,
    // they're gone, and the outbox is keyed on entity, so pushing the wrong
    // partition would mis-attribute the delete.
    let placeholders: String = std::iter::repeat_n("?", source_refs.len())
        .collect::<Vec<_>>()
        .join(",");
    let select_sql =
        format!("SELECT id, entity_id FROM llm_wiki_entries WHERE source_ref IN ({placeholders})");
    let mut stmt = tx.prepare(&select_sql)?;
    let doomed: Vec<(String, String)> = stmt
        .query_map(rusqlite::params_from_iter(source_refs.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    // Push one Delete row per doomed entry, sourcing entity_id from the row
    // itself. Same shape as `prune_old_librarian_inferred` (lib.rs:1779).
    for (id, entity_id) in &doomed {
        crate::db::commit::push_entries_outbox(
            &tx,
            entity_id,
            id,
            crate::db::outbox_format::OutboxOperation::Delete,
            serde_json::json!({ "id": id }),
            now_ms,
        )?;
    }

    // FK CASCADE is not relied upon (spec §2.1): brain.db has connections whose
    // `PRAGMA foreign_keys` state we do not control, so the evidence row is
    // deleted explicitly alongside its entry.
    let doomed_ids: Vec<String> = doomed.iter().map(|(id, _)| id.clone()).collect();
    crate::db::commit::delete_librarian_evidence(&tx, &doomed_ids)?;

    let delete_sql = format!("DELETE FROM llm_wiki_entries WHERE source_ref IN ({placeholders})");
    let removed = tx.execute(&delete_sql, rusqlite::params_from_iter(source_refs.iter()))?;

    if !doomed.is_empty() {
        // HARD delete, so the hard-delete purge: `purge_edges_for_entries`
        // keeps an edge whose partner is still alive, which is right for a
        // soft delete but strands the edge forever here — the doomed id is
        // gone from every table, so no future cascade can ever find it again.
        let doomed_ids: Vec<String> = doomed.iter().map(|(id, _)| id.clone()).collect();
        crate::db::edge_purge::purge_edges_for_hard_deleted(&tx, &doomed_ids)?;
    }

    tx.commit()?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    /// Inserts a live entry (deleted_at IS NULL).
    fn seed_entry(conn: &Connection, id: &str, entity_id: &str, source_ref: Option<&str>) {
        let source_ref_val: Option<String> = source_ref.map(|s| s.to_string());
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred',
                       NULL, ?3, 100, 100, NULL, 0, NULL, NULL, NULL)",
            rusqlite::params![id, entity_id, source_ref_val],
        )
        .unwrap();
    }

    fn seed_edge(conn: &Connection, id: &str, entity_id: &str, source: &str, target: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4, 'related_to', 100)",
            rusqlite::params![id, entity_id, source, target],
        )
        .unwrap();
    }

    fn entry_ids(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT id FROM llm_wiki_entries ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
    }

    fn edge_ids(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT id FROM llm_wiki_edges ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
    }

    #[test]
    fn forget_entries_by_source_refs_purges_their_edges() {
        let conn = open_in_memory().unwrap();
        // Three entries: two to forget (same source_ref), one unrelated.
        seed_entry(&conn, "fact_a", "ent-1", Some("/vault/a.pdf"));
        seed_entry(&conn, "fact_b", "ent-2", Some("/vault/a.pdf"));
        seed_entry(&conn, "fact_c", "ent-3", Some("/vault/c.pdf"));
        // fact_c is soft-deleted here so this test also covers the
        // both-endpoints-dead shape. The partner-alive shape has its own test
        // below — under `purge_edges_for_hard_deleted` it is purged too.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id = 'fact_c'",
            [],
        )
        .unwrap();
        // Two edges from the doomed entries to the (now-dead) unrelated one.
        seed_edge(&conn, "edge_ac", "ent-1", "fact_a", "fact_c");
        seed_edge(&conn, "edge_bc", "ent-2", "fact_b", "fact_c");
        // One edge between two unrelated entries — must survive (fact_a and
        // fact_b never appear on its endpoints so the cascade never touches
        // it, and the broader `purge_orphan_edges` is not invoked here).
        seed_edge(
            &conn,
            "edge_c_unrelated",
            "ent-3",
            "fact_c",
            "some_other_live_id",
        );

        let removed =
            forget_entries_by_source_refs(&conn, &["/vault/a.pdf".to_string()], 1_756_000_000_000)
                .unwrap();

        assert_eq!(removed, 2, "two entries forgotten");
        // The two doomed entries are gone.
        assert!(entry_ids(&conn)
            .iter()
            .all(|id| id != "fact_a" && id != "fact_b"));
        // Edges touching doomed endpoints are gone.
        assert!(edge_ids(&conn)
            .iter()
            .all(|id| id != "edge_ac" && id != "edge_bc"));
        // The unrelated edge survives.
        assert!(edge_ids(&conn).contains(&"edge_c_unrelated".to_string()));
    }

    #[test]
    fn forget_purges_edges_even_when_the_partner_is_still_alive() {
        // Forget HARD-deletes. Under the soft-delete cascade
        // (`purge_edges_for_entries`) an edge to a live partner survives, and
        // because the forgotten id no longer exists in any table, nothing
        // would ever collect that edge again — it dangles forever.
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_forgotten", "ent-1", Some("/vault/a.pdf"));
        seed_entry(&conn, "fact_live", "ent-2", Some("/vault/keep.pdf"));
        seed_edge(&conn, "edge_out", "ent-1", "fact_forgotten", "fact_live");
        seed_edge(&conn, "edge_in", "ent-1", "fact_live", "fact_forgotten");

        let removed =
            forget_entries_by_source_refs(&conn, &["/vault/a.pdf".to_string()], 1_756_000_000_000)
                .unwrap();

        assert_eq!(removed, 1);
        assert!(
            edge_ids(&conn).is_empty(),
            "edges anchored on a forgotten (hard-deleted) entry must not survive, \
             in either direction, even though fact_live is still alive"
        );
        // The live partner itself is untouched.
        assert!(entry_ids(&conn).contains(&"fact_live".to_string()));
    }

    #[test]
    fn forget_entries_by_source_refs_with_no_matches_is_a_noop() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1", Some("/vault/a.pdf"));
        seed_edge(
            &conn,
            "edge_unrelated",
            "ent-1",
            "fact_a",
            "some_other_live_id",
        );

        let removed = forget_entries_by_source_refs(
            &conn,
            &["/no/such/file.pdf".to_string()],
            1_756_000_000_000,
        )
        .unwrap();

        assert_eq!(removed, 0);
        assert_eq!(edge_ids(&conn), vec!["edge_unrelated".to_string()]);
        // The unrelated edge survives because fact_a was not deleted.
    }

    #[test]
    fn forget_entries_by_source_refs_starts_its_own_transaction_and_commits() {
        // Verify forget_entries_by_source_refs opens a transaction and commits it.
        // After the call, the entry should be gone and the change durable.
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1", Some("/vault/a.pdf"));
        seed_entry(&conn, "fact_c", "ent-3", Some("/vault/c.pdf"));
        // fact_c soft-deleted so the assertion covers the fully-dead shape.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id = 'fact_c'",
            [],
        )
        .unwrap();
        seed_edge(&conn, "edge_ac", "ent-1", "fact_a", "fact_c");

        forget_entries_by_source_refs(&conn, &["/vault/a.pdf".to_string()], 1_756_000_000_000)
            .unwrap();

        // Entry is gone and edge is gone — committed state is durable.
        assert!(entry_ids(&conn).iter().all(|id| id != "fact_a"));
        assert!(edge_ids(&conn).iter().all(|id| id != "edge_ac"));
    }

    #[test]
    fn forget_entries_by_source_refs_rolls_back_if_purge_fails() {
        // The DELETE and the edge purge share one transaction: if the purge
        // fails, the entry deletion must roll back with it so a crash can
        // never leave entries gone but their edges stranded.
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1", Some("/vault/a.pdf"));
        seed_entry(&conn, "fact_c", "ent-3", Some("/vault/c.pdf"));
        seed_edge(&conn, "edge_ac", "ent-1", "fact_a", "fact_c");

        // Inject a real failure into the purge step: drop the table it reads.
        // `purge_edges_for_entries` then errors, `?` propagates out of the
        // function, and `tx` is dropped without `commit()` — a rollback.
        conn.execute("DROP TABLE llm_wiki_edges", []).unwrap();

        let err =
            forget_entries_by_source_refs(&conn, &["/vault/a.pdf".to_string()], 1_756_000_000_000)
                .expect_err("purge failure must propagate");
        assert!(
            err.to_string().contains("llm_wiki_edges"),
            "expected the purge step to be the failing operation, got: {err}"
        );

        // The DELETE rolled back with it — fact_a is still here.
        assert!(
            entry_ids(&conn).iter().any(|id| id == "fact_a"),
            "entry deletion must roll back when the edge purge fails"
        );
    }

    /// The multi-entity case is not hypothetical here: `source_ref` is a vault
    /// file path, and one ingested file can produce entries across several
    /// entities. A push that reused the first row's partition key would
    /// mis-attribute every delete after the first.
    #[test]
    fn forget_pushes_one_delete_outbox_row_per_entry_with_its_own_entity_id() {
        let conn = open_in_memory().unwrap();
        let now_ms: i64 = 1_756_000_000_000;
        seed_entry(&conn, "fact_a", "ent-1", Some("/vault/a.pdf"));
        seed_entry(&conn, "fact_b", "ent-2", Some("/vault/a.pdf"));
        seed_entry(&conn, "fact_keep", "ent-3", Some("/vault/keep.pdf"));

        let removed =
            forget_entries_by_source_refs(&conn, &["/vault/a.pdf".to_string()], now_ms).unwrap();
        assert_eq!(removed, 2);

        let rows: Vec<(String, String, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT record_id, entity_id, operation, payload
                     FROM llm_wiki_outbox
                     WHERE table_name = 'entries'
                     ORDER BY record_id",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        assert_eq!(
            rows.len(),
            2,
            "one Delete row per forgotten entry, and no more"
        );
        assert_eq!(rows[0].0, "fact_a");
        assert_eq!(
            rows[0].1, "ent-1",
            "entity_id is read per row, not assumed uniform"
        );
        assert_eq!(rows[0].2, "DELETE");
        assert_eq!(rows[1].0, "fact_b");
        assert_eq!(rows[1].1, "ent-2", "the second row keeps its own partition");
        assert_eq!(rows[1].2, "DELETE");
        let payload: serde_json::Value = serde_json::from_str(&rows[0].3).unwrap();
        assert_eq!(payload["id"], "fact_a");
    }

    #[test]
    fn forget_with_no_matches_pushes_no_outbox_rows() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_keep", "ent-1", Some("/vault/keep.pdf"));

        let removed = forget_entries_by_source_refs(
            &conn,
            &["/no/such/file.pdf".to_string()],
            1_756_000_000_000,
        )
        .unwrap();

        assert_eq!(removed, 0);
        let count: i64 = conn
            .query_row("SELECT count(*) FROM llm_wiki_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn forget_entries_by_source_refs_leaves_no_orphaned_evidence() {
        let conn = open_in_memory().unwrap();
        // brain.db has connections whose `PRAGMA foreign_keys` state we do not
        // control (spec §2.1); replicate the OFF case so the test cannot pass
        // via FK CASCADE.
        conn.execute("PRAGMA foreign_keys=OFF", []).unwrap();
        seed_entry(&conn, "fact_p", "ent", Some("/vault/p.pdf"));
        crate::db::commit::insert_librarian_evidence(
            &conn,
            "fact_p",
            "prop_p",
            r#"{"evidence":[],"proposal_id":"prop_p"}"#,
            false,
            1,
        )
        .unwrap();

        let removed =
            forget_entries_by_source_refs(&conn, &["/vault/p.pdf".to_string()], 1_756_000_000_000)
                .unwrap();
        assert_eq!(removed, 1);

        let orphaned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM librarian_evidence WHERE entry_id='fact_p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphaned, 0, "evidence row must be deleted with its entry");
    }

    #[test]
    fn forget_rollback_leaves_no_outbox_rows() {
        // Same failure injection as forget_entries_by_source_refs_rolls_back_if_purge_fails:
        // the outbox push must be inside the transaction that the purge failure aborts.
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1", Some("/vault/a.pdf"));
        conn.execute("DROP TABLE llm_wiki_edges", []).unwrap();

        let err =
            forget_entries_by_source_refs(&conn, &["/vault/a.pdf".to_string()], 1_756_000_000_000)
                .unwrap_err();
        assert!(err.to_string().contains("llm_wiki_edges"));

        let outbox_count: i64 = conn
            .query_row("SELECT count(*) FROM llm_wiki_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outbox_count, 0, "a rolled-back forget replicates nothing");
        let entry_count: i64 = conn
            .query_row("SELECT count(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entry_count, 1, "and deletes nothing");
    }
}
