//! Cascading edge deletion: an edge in `llm_wiki_edges` is purged only once
//! **both** of its endpoints are dead — that is, neither `source_id` nor
//! `target_id` resolves to a live row in any of the three endpoint tables. A
//! half-live edge (one endpoint still alive) is deliberately retained, so the
//! surviving side keeps its connection. Every site that deletes, archives, or
//! regenerates content calls into here **inside its own transaction**, so a
//! crash can never strand an edge whose last endpoint just died.
//!
//! Edges are not replicated (no outbox rows) — `commit_edge_add` and the bundle
//! import path insert them without CDC, so purges match. See the design spec §2.
//!
//! ## Heterogeneous endpoint contract (remediation R1)
//!
//! `llm_wiki_edges.source_id` / `target_id` may resolve to a row in any of
//! three tables: `llm_wiki_entries`, `curated_entities`, or `llm_wiki_tasks`.
//! Each table carries a `deleted_at INTEGER` column, so a row that exists but
//! is soft-deleted counts as absent for the purposes of edge-purge.
//!
//! An edge is **dead** iff BOTH endpoints are absent from ALL THREE tables
//! (with `deleted_at IS NULL` gates). The asymmetric helper below is reused by
//! `purge_edges_for_entry` (single-id cascade) and `purge_dead_edges`
//! (entity-content reset) so the three-table truth lives in one Rust file.
//!
//! ## Soft delete vs hard delete
//!
//! The partner-alive retention rule above is written for *soft* deletion: the
//! dead row is still present, `deleted_at` can be cleared, and the edge is
//! worth keeping. The two sites that **hard**-delete rows — `wiki_forget` and
//! the bundle import's `clear_entity_content` — call
//! `purge_edges_for_hard_deleted` instead, because an id that no longer exists
//! anywhere can never come back and no later cascade could ever collect its
//! edges.

use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection};

/// SQL fragments evaluating whether the named `llm_wiki_edges` column is
/// **alive**: present in `llm_wiki_entries`, `curated_entities`, or
/// `llm_wiki_tasks` with `deleted_at IS NULL`.
///
/// Edge endpoints are heterogeneous — see the module docs. An endpoint id may
/// belong to any of the three tables; an edge is preserved as long as ONE
/// endpoint resolves to at least one of the three.
///
/// Each fragment is a bare `OR` chain with no surrounding parentheses.
/// Callers must parenthesize it before negating, e.g. `NOT ({fragment})`.
///
/// The text depends only on the column name, so these are `const` rather than
/// re-`format!`-built on every cascade call.
const SOURCE_ALIVE_SQL: &str = "EXISTS (SELECT 1 FROM llm_wiki_entries e  WHERE e.id  = source_id AND e.deleted_at  IS NULL) \
      OR EXISTS (SELECT 1 FROM curated_entities ce WHERE ce.id = source_id AND ce.deleted_at IS NULL) \
      OR EXISTS (SELECT 1 FROM llm_wiki_tasks st WHERE st.id = source_id AND st.deleted_at IS NULL)";

const TARGET_ALIVE_SQL: &str = "EXISTS (SELECT 1 FROM llm_wiki_entries e  WHERE e.id  = target_id AND e.deleted_at  IS NULL) \
      OR EXISTS (SELECT 1 FROM curated_entities ce WHERE ce.id = target_id AND ce.deleted_at IS NULL) \
      OR EXISTS (SELECT 1 FROM llm_wiki_tasks st WHERE st.id = target_id AND st.deleted_at IS NULL)";

/// Max entry ids bound into one batch purge statement. Each id is bound twice
/// (one IN clause per endpoint column), so 2 * this must stay under SQLite's
/// SQLITE_MAX_VARIABLE_NUMBER (32766 on the bundled build).
const BATCH_PURGE_CHUNK: usize = 8000;

/// Delete every edge whose endpoint `entry_id` is dead in every valid home,
/// even when the OTHER endpoint is still alive.
///
/// Before remediation R1 this was the naive `WHERE source_id = ?1 OR target_id
/// = ?1` cascade, which destroyed live edges whenever `entry_id` happened to
/// also be a curated-entity id or a task id. The new shape preserves edges
/// whose other endpoint is alive in `llm_wiki_entries`, `curated_entities`,
/// or `llm_wiki_tasks`.
///
/// Takes `&Connection` rather than `&mut Connection` so it composes inside a
/// caller's open transaction — the entry deletion and this purge must commit or
/// roll back together, otherwise a crash between them mints an orphan.
pub fn purge_edges_for_entry(conn: &Connection, entry_id: &str) -> Result<usize> {
    let sql = format!(
        "DELETE FROM llm_wiki_edges
          WHERE (source_id = ?1 AND NOT ({target_alive}))
             OR (target_id = ?1 AND NOT ({source_alive}))",
        target_alive = TARGET_ALIVE_SQL,
        source_alive = SOURCE_ALIVE_SQL,
    );
    let removed = conn.execute(&sql, params![entry_id])?;
    Ok(removed)
}

/// Purge edges for a set of dying entries, for the predicate-driven deletion
/// sites (`prune_old_librarian_inferred`) that do not have a single id to hand.
///
/// One DELETE with both IN clauses per chunk, rather than one statement per
/// id. Each matching row contributes 1 to the count (the WHERE OR semantics
/// make the row match at most once, and a row deleted by an earlier chunk is
/// gone), so the "counted once across both ids" invariant holds.
pub fn purge_edges_for_entries(conn: &Connection, entry_ids: &[String]) -> Result<usize> {
    let mut total = 0;
    for chunk in entry_ids.chunks(BATCH_PURGE_CHUNK) {
        let placeholders: String = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM llm_wiki_edges
          WHERE (source_id IN ({placeholders}) AND NOT ({target_alive}))
             OR (target_id IN ({placeholders}) AND NOT ({source_alive}))",
            target_alive = TARGET_ALIVE_SQL,
            source_alive = SOURCE_ALIVE_SQL,
        );
        // Bind the chunk twice — once for each IN clause.
        let bound: Vec<&str> = chunk
            .iter()
            .chain(chunk.iter())
            .map(String::as_str)
            .collect();
        total += conn.execute(&sql, params_from_iter(bound))?;
    }
    Ok(total)
}

/// Purge edges anchored on ids that were **hard-deleted** (row physically
/// gone), regardless of whether the partner endpoint is still alive.
///
/// This is the counterpart to [`purge_edges_for_entries`], not a duplicate of
/// it. The partner-alive retention rule above exists because the usual
/// deletion sites *soft*-delete: the row is still there, `deleted_at` can be
/// cleared, and the edge is worth keeping so the surviving side does not lose
/// its connection when the partner comes back. A hard-deleted id can never
/// come back, so an edge anchored on one references nothing forever — and no
/// later cascade can clean it, because every other purge predicate needs the
/// dead endpoint to be *findable* to fire. Left alone, these rows accumulate
/// permanently: `purge_dead_edges` and the `graph_reanchor` migration both
/// require BOTH endpoints dead, so neither ever collects a half-dangling one.
///
/// (They are not user-visible today — `wiki_graph::wiki_traverse_graph` inner-
/// joins `llm_wiki_entries` on both endpoints, so a dangling edge is filtered
/// out at read time. This is a storage-hygiene fix and a guard against a
/// future reader that trusts the edge table.)
///
/// The `NOT (alive)` guard on the deleted side is still required: an entry id
/// may collide with a live `curated_entities` or `llm_wiki_tasks` id, and the
/// naive `WHERE source_id = ?1 OR target_id = ?1` form is exactly the R1 bug
/// this module was written to fix. Here the aliveness check applies to the
/// *deleted* endpoint rather than its partner.
///
/// Takes `&Connection` so it composes inside the caller's transaction.
pub fn purge_edges_for_hard_deleted(conn: &Connection, deleted_ids: &[String]) -> Result<usize> {
    let mut total = 0;
    for chunk in deleted_ids.chunks(BATCH_PURGE_CHUNK) {
        let placeholders: String = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM llm_wiki_edges
          WHERE (source_id IN ({placeholders}) AND NOT ({source_alive}))
             OR (target_id IN ({placeholders}) AND NOT ({target_alive}))",
            source_alive = SOURCE_ALIVE_SQL,
            target_alive = TARGET_ALIVE_SQL,
        );
        // Bind the chunk twice — once for each IN clause.
        let bound: Vec<&str> = chunk
            .iter()
            .chain(chunk.iter())
            .map(String::as_str)
            .collect();
        total += conn.execute(&sql, params_from_iter(bound))?;
    }
    Ok(total)
}

/// Delete every edge with no live endpoint anywhere — symmetric, no id.
///
/// Used by `bundle_apply::apply_import` (remediation R1, post-loop inside the
/// import transaction): edges are stamped with `ctx.entity_id` even when they
/// span entities, so the old per-entity `DELETE FROM llm_wiki_edges WHERE
/// entity_id = ?1` would strand the partner entity's edges as orphan-class.
/// The correct contract is to purge only edges whose **both** endpoints are
/// dead across all three tables. Running once after the entity loop covers
/// every entity the import touched in a single scan.
///
/// Note: this runs on every import mode (Replace/Merge/Clone), not just
/// Replace — a pre-existing orphan edge unrelated to the bundle will be swept
/// by any import. Benign (the predicate restricts the delete to truly dead
/// edges) but worth knowing if the post-import edge count ever looks
/// surprising.
pub fn purge_dead_edges(conn: &Connection) -> Result<usize> {
    let sql = format!(
        "DELETE FROM llm_wiki_edges
          WHERE NOT ({alive_source})
            AND NOT ({alive_target})",
        alive_source = SOURCE_ALIVE_SQL,
        alive_target = TARGET_ALIVE_SQL,
    );
    let removed = conn.execute(&sql, [])?;
    Ok(removed)
}

/// Delete edges whose `edge_type` is absent from the entity's strict ontology
/// manifest.
///
/// The write-time gate in `db::commit` is non-retroactive by design, so a brain
/// that went strict *after* edges were written still holds ungated rows. This is
/// the retroactive counterpart. Returns 0 and touches nothing when the entity
/// has no strict ontology — an unreadable or non-strict ontology must never be
/// read as "everything is illegal".
///
/// Takes `&Connection` and opens an unchecked transaction so it composes inside
/// a caller's transaction as the rest of this module does.
pub fn purge_off_manifest_edges(conn: &Connection, entity_id: &str, now_ms: i64) -> Result<usize> {
    let Some(vocab) = crate::db::commit::resolve_strict_edge_vocabulary(conn, entity_id) else {
        return Ok(0);
    };

    let tx = conn.unchecked_transaction()?;

    // Collect first so each doomed edge can be logged (and, once edges gain
    // CDC, get its own outbox Delete row) rather than vanishing inside one
    // set-based DELETE.
    let doomed: Vec<(String, String)> = {
        let mut stmt =
            tx.prepare("SELECT id, edge_type FROM llm_wiki_edges WHERE entity_id = ?1")?;
        let mut rows = stmt.query([entity_id])?;
        let mut v = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let edge_type: String = row.get(1)?;
            if !vocab.contains(&edge_type.trim().to_lowercase()) {
                v.push((id, edge_type));
            }
        }
        v
    };

    if doomed.is_empty() {
        return Ok(0);
    }

    for (id, edge_type) in &doomed {
        warn_purging_off_manifest_edge(entity_id, id, edge_type);
        tx.execute("DELETE FROM llm_wiki_edges WHERE id = ?1", [id])?;
    }

    tx.commit()?;
    // Reserved for outbox wiring: edges carry no CDC rows today (see the module
    // docs), so there is no helper to call here.
    let _ = now_ms;
    Ok(doomed.len())
}

/// Every purged edge is announced individually — a retroactive delete is
/// destructive and an operator must be able to see exactly what went. Same
/// two-variant pattern as `db::commit`: `tracing` where a subscriber exists,
/// `eprintln!` in the Tauri build where none does.
#[cfg(feature = "mcp-server")]
fn warn_purging_off_manifest_edge(entity_id: &str, edge_id: &str, edge_type: &str) {
    tracing::warn!(
        target: "ct::edge_purge",
        entity_id = %entity_id,
        edge_id = %edge_id,
        edge_type = %edge_type,
        "purging off-manifest edge"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_purging_off_manifest_edge(entity_id: &str, edge_id: &str, edge_type: &str) {
    eprintln!(
        "[ct::edge_purge WARN] purging off-manifest edge: entity_id={entity_id:?} \
         edge_id={edge_id:?} edge_type={edge_type:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    /// Inserts a live entry. `deleted_at` is NULL (live).
    fn seed_entry(conn: &Connection, id: &str, entity_id: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, NULL, NULL)",
            params![id, entity_id],
        )
        .unwrap();
    }

    fn seed_entity(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at, deleted_at)
             VALUES (?1, 'n', 'concept', '', 1, 1, NULL)",
            params![id],
        )
        .unwrap();
    }

    fn seed_task(conn: &Connection, id: &str, entity_id: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_tasks (id, entity_id, description, status, priority,
                created_at, updated_at, resolved_at, deleted_at)
             VALUES (?1, ?2, 'd', 'pending', 0, 1, 1, NULL, NULL)",
            params![id, entity_id],
        )
        .unwrap();
    }

    fn seed_edge(conn: &Connection, id: &str, entity_id: &str, source: &str, target: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4, 'related_to', 100)",
            params![id, entity_id, source, target],
        )
        .unwrap();
    }

    /// Install a **strict** ontology manifest declaring `edge_types` for
    /// `entity_id`. Mirrors `db::commit::tests::seed_manifest`, narrowed to the
    /// only shape the off-manifest sweep cares about.
    fn seed_strict_ontology(conn: &Connection, entity_id: &str, edge_types: &[&str]) {
        let edges: Vec<serde_json::Value> = edge_types
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": t, "source_type": "fact", "target_type": "fact", "description": ""
                })
            })
            .collect();
        let manifest = serde_json::json!({
            "node_types": [{ "type": "fact", "description": "" }],
            "edge_types": edges,
        })
        .to_string();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES (?1, 'strict', ?2, 0)",
            params![entity_id, manifest],
        )
        .unwrap();
    }

    /// Live entry with a generated id, for the sweep tests. The older
    /// `seed_entry` takes an explicit id and is used by 10+ pre-existing tests;
    /// this is an additive sibling rather than a migration of all of them.
    fn seed_live_entry(conn: &Connection, entity_id: &str, label: &str) -> String {
        let id = format!("fact_{entity_id}_{label}");
        seed_entry(conn, &id, entity_id);
        id
    }

    /// Live edge with an explicit `edge_type` (the older `seed_edge` hardcodes
    /// `related_to`).
    fn seed_live_edge(
        conn: &Connection,
        entity_id: &str,
        source: &str,
        target: &str,
        edge_type: &str,
    ) -> String {
        let id = format!("edge_{entity_id}_{edge_type}");
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 100)",
            params![id, entity_id, source, target, edge_type],
        )
        .unwrap();
        id
    }

    fn edge_ids(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT id FROM llm_wiki_edges ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
    }

    #[test]
    fn cascade_only_removes_edges_whose_partner_is_also_dead() {
        // R1 contract: a cascade after deleting entry X removes ONLY edges
        // where X is on one end AND the OTHER endpoint is dead in every
        // valid home (llm_wiki_entries / curated_entities / llm_wiki_tasks).
        // Edges whose partner is still alive anywhere survive.
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_entry(&conn, "fact_c", "ent-1");
        seed_edge(&conn, "edge_out", "ent-1", "fact_a", "fact_b");
        seed_edge(&conn, "edge_in", "ent-1", "fact_c", "fact_a");
        seed_edge(&conn, "edge_other", "ent-1", "fact_b", "fact_c");

        // Soft-delete fact_a to mirror the production cascade call sites
        // (`commit::fact_archive` and `facts::archive_fact` soft-delete
        // before purging).
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'fact_a'",
            [],
        )
        .unwrap();

        let removed = purge_edges_for_entry(&conn, "fact_a").unwrap();

        // fact_b and fact_c are alive — partner-alive edges must survive
        // the heterogeneous contract.
        assert_eq!(
            removed, 0,
            "alive partners preserve the edge; only edges with both endpoints \
             dead are purged"
        );
        assert_eq!(edge_ids(&conn).len(), 3);
    }

    #[test]
    fn cascade_purges_edges_when_partner_is_also_dead() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_entry(&conn, "fact_c", "ent-1");
        seed_edge(&conn, "edge_out", "ent-1", "fact_a", "fact_b");
        seed_edge(&conn, "edge_in", "ent-1", "fact_c", "fact_a");
        seed_edge(&conn, "edge_other", "ent-1", "fact_b", "fact_c");

        // Soft-delete fact_a AND fact_b so both endpoints of edge_out are dead.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id IN ('fact_a', 'fact_b')",
            [],
        )
        .unwrap();

        let removed = purge_edges_for_entry(&conn, "fact_a").unwrap();

        // edge_out: fact_a dead AND fact_b dead → purged.
        // edge_in:  fact_a dead (target) BUT fact_c alive (source) → survives.
        // edge_other: fact_a not on either end → untouched.
        assert_eq!(removed, 1, "only the fully-dead edge is purged");
        let surviving: Vec<String> = edge_ids(&conn)
            .into_iter()
            .filter(|id| id != "edge_out")
            .collect();
        assert_eq!(surviving.len(), 2);
        assert!(surviving.contains(&"edge_in".to_string()));
        assert!(surviving.contains(&"edge_other".to_string()));
    }

    #[test]
    fn purging_an_entry_with_no_edges_is_a_no_op() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_edge(&conn, "edge_other", "ent-1", "fact_a", "fact_b");

        let removed = purge_edges_for_entry(&conn, "fact_unrelated").unwrap();

        assert_eq!(removed, 0);
        assert_eq!(edge_ids(&conn), vec!["edge_other".to_string()]);
    }

    #[test]
    fn purges_a_set_of_entries_and_counts_each_edge_once() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_entry(&conn, "fact_c", "ent-1");
        // edge_ab has both endpoints in the doomed set; edge_bc has one.
        seed_edge(&conn, "edge_ab", "ent-1", "fact_a", "fact_b");
        seed_edge(&conn, "edge_bc", "ent-1", "fact_b", "fact_c");

        // Soft-delete the doomed ids so the cascade sees them as dead.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id IN ('fact_a', 'fact_b')",
            [],
        )
        .unwrap();

        let doomed = vec!["fact_a".to_string(), "fact_b".to_string()];
        let removed = purge_edges_for_entries(&conn, &doomed).unwrap();

        // edge_ab is fully dead and is purged (counted once across both ids).
        // edge_bc is only half-dead (fact_b dead, fact_c alive) and survives.
        assert_eq!(removed, 1, "edge_ab purged once; edge_bc survives");
        assert_eq!(edge_ids(&conn), vec!["edge_bc".to_string()]);
    }

    #[test]
    fn purge_is_visible_to_the_enclosing_transaction_and_rolls_back_with_it() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        // Soft-delete both endpoints so the cascade would remove the edge.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id IN ('fact_a', 'fact_b')",
            [],
        )
        .unwrap();
        seed_edge(&conn, "edge_ab", "ent-1", "fact_a", "fact_b");

        let tx = conn.unchecked_transaction().unwrap();
        purge_edges_for_entry(&tx, "fact_a").unwrap();
        tx.rollback().unwrap();

        assert_eq!(
            edge_ids(&conn),
            vec!["edge_ab".to_string()],
            "a rolled-back purge must leave the edge in place"
        );
    }

    #[test]
    fn preserves_edges_pointing_at_curated_entity_ids() {
        let conn = open_in_memory().unwrap();
        // One endpoint is a live entry, the other is a live curated entity.
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_entity(&conn, "ce_only");
        seed_edge(&conn, "edge_to_entity", "ent-1", "fact_a", "ce_only");
        // Edge with both endpoints live in curated_entities only.
        seed_entity(&conn, "ce_a");
        seed_entity(&conn, "ce_b");
        seed_edge(&conn, "edge_entity_entity", "ent-1", "ce_a", "ce_b");
        // A truly dead edge — neither endpoint anywhere.
        seed_edge(&conn, "edge_dead", "ent-1", "ghost_a", "ghost_b");
        // A different entry→entity edge; must differ on (source_id,target_id).
        seed_edge(&conn, "edge_entry_to_entity", "ent-1", "fact_b", "ce_only");

        // Soft-delete fact_a to mirror the production cascade.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'fact_a'",
            [],
        )
        .unwrap();

        let removed = purge_edges_for_entry(&conn, "fact_a").unwrap();

        // edge_to_entity: fact_a dead (source) AND ce_only alive (target in
        // curated_entities) → survives.
        // edge_entity_entity: fact_a not on either end → untouched.
        // edge_entry_to_entity: fact_a not on either end → untouched.
        // edge_dead: fact_a not on either end → untouched.
        assert_eq!(
            removed, 0,
            "ce_only endpoint is alive in curated_entities — edges survive"
        );
        assert_eq!(edge_ids(&conn).len(), 4);
    }

    #[test]
    fn preserves_edges_pointing_at_task_ids() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_task(&conn, "task_only", "ent-1");
        seed_edge(&conn, "edge_to_task", "ent-1", "fact_a", "task_only");
        // A different entry→task edge (different (source,target) so UNIQUE holds).
        seed_edge(&conn, "edge_dead", "ent-1", "ghost_a", "ghost_b");

        // Soft-delete fact_a to mirror the production cascade.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'fact_a'",
            [],
        )
        .unwrap();

        let removed = purge_edges_for_entry(&conn, "fact_a").unwrap();
        // fact_a is dead (source), task_only is alive (target in llm_wiki_tasks)
        // → edge_to_task survives.
        assert_eq!(
            removed, 0,
            "task_only endpoint is alive in llm_wiki_tasks — edge survives"
        );
        assert_eq!(edge_ids(&conn).len(), 2);
    }

    #[test]
    fn preserves_edges_whose_entity_id_endpoint_alone_is_dead() {
        // The R1 proof test: `purge_edges_for_entry` is called with an
        // llm_wiki_entries id that does NOT exist in the entries table; the
        // edge's only home is curated_entities, so it must survive.
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ce_only_endpoint");
        seed_edge(
            &conn,
            "edge_pure_entity",
            "ent-1",
            "ce_only_endpoint",
            "ce_only_endpoint",
        );

        let removed = purge_edges_for_entry(&conn, "non_existent_entry_id").unwrap();
        assert_eq!(
            removed, 0,
            "edge stamped with an entity id must not be purged by an entry-id cascade"
        );
        assert_eq!(edge_ids(&conn), vec!["edge_pure_entity".to_string()]);
    }

    #[test]
    fn hard_delete_purge_removes_edges_whose_partner_is_still_alive() {
        // The soft-delete cascade deliberately keeps a half-live edge. A
        // hard-deleted endpoint is different: the id is gone for good, so the
        // edge references nothing and nothing else will ever collect it.
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_gone", "ent-1");
        seed_entry(&conn, "fact_alive", "ent-2");
        seed_edge(&conn, "edge_out", "ent-1", "fact_gone", "fact_alive");
        seed_edge(&conn, "edge_in", "ent-1", "fact_alive", "fact_gone");
        seed_entry(&conn, "fact_other", "ent-2");
        seed_edge(&conn, "edge_untouched", "ent-2", "fact_alive", "fact_other");

        // HARD delete, as `wiki_forget` and `clear_entity_content` do.
        conn.execute("DELETE FROM llm_wiki_entries WHERE id = 'fact_gone'", [])
            .unwrap();

        // The soft-delete cascade would keep both edges: fact_alive is alive.
        assert_eq!(
            purge_edges_for_entries(&conn, &["fact_gone".to_string()]).unwrap(),
            0,
            "the partner-alive rule keeps these — that is the gap being closed"
        );

        let removed = purge_edges_for_hard_deleted(&conn, &["fact_gone".to_string()]).unwrap();
        assert_eq!(removed, 2, "both dangling edges go, in either direction");
        assert_eq!(edge_ids(&conn), vec!["edge_untouched".to_string()]);
    }

    #[test]
    fn hard_delete_purge_spares_an_id_that_still_lives_in_another_table() {
        // R1: an entry id may collide with a live curated-entity or task id.
        // Deleting the entry row must not strip edges anchored on the entity.
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "shared_id");
        seed_entry(&conn, "fact_alive", "ent-1");
        seed_edge(&conn, "edge_shared", "ent-1", "shared_id", "fact_alive");

        let removed = purge_edges_for_hard_deleted(&conn, &["shared_id".to_string()]).unwrap();

        assert_eq!(
            removed, 0,
            "the id is still alive in curated_entities — the edge must survive"
        );
        assert_eq!(edge_ids(&conn), vec!["edge_shared".to_string()]);
    }

    #[test]
    fn hard_delete_purge_with_no_ids_is_a_no_op() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_edge(&conn, "edge_ab", "ent-1", "fact_a", "fact_b");

        assert_eq!(purge_edges_for_hard_deleted(&conn, &[]).unwrap(), 0);
        assert_eq!(edge_ids(&conn), vec!["edge_ab".to_string()]);
    }

    #[test]
    fn purge_dead_edges_removes_only_edges_with_no_live_endpoint_anywhere() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_entity(&conn, "ce_a");
        seed_task(&conn, "task_a", "ent-1");
        seed_edge(&conn, "edge_live_entry", "ent-1", "fact_a", "fact_b");
        seed_edge(&conn, "edge_to_entity", "ent-1", "fact_a", "ce_a");
        seed_edge(&conn, "edge_to_task", "ent-1", "task_a", "fact_b");
        // Truly dead edges.
        seed_edge(&conn, "edge_dead_both", "ent-1", "ghost_a", "ghost_b");

        let removed = purge_dead_edges(&conn).unwrap();
        assert_eq!(removed, 1, "only the edge with two dead endpoints goes");
        assert_eq!(edge_ids(&conn).len(), 3);
        assert!(!edge_ids(&conn).contains(&"edge_dead_both".to_string()));
    }

    #[test]
    fn purge_off_manifest_edges_removes_only_undeclared_types() {
        let conn = open_in_memory().unwrap();
        seed_strict_ontology(&conn, "ent_demo", &["depends_on"]);
        let a = seed_live_entry(&conn, "ent_demo", "A");
        let b = seed_live_entry(&conn, "ent_demo", "B");
        seed_live_edge(&conn, "ent_demo", &a, &b, "depends_on");
        seed_live_edge(&conn, "ent_demo", &a, &b, "fabricated_2026-09-09");

        let removed = purge_off_manifest_edges(&conn, "ent_demo", 1_725_000_000_000).unwrap();
        assert_eq!(removed, 1, "exactly the off-manifest edge must be purged");

        let remaining: Vec<String> = conn
            .prepare("SELECT edge_type FROM llm_wiki_edges WHERE entity_id = 'ent_demo'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(remaining, vec!["depends_on".to_string()]);
    }

    #[test]
    fn purge_off_manifest_edges_is_a_noop_without_a_strict_ontology() {
        let conn = open_in_memory().unwrap();
        let a = seed_live_entry(&conn, "ent_open", "A");
        let b = seed_live_entry(&conn, "ent_open", "B");
        seed_live_edge(&conn, "ent_open", &a, &b, "anything");

        let removed = purge_off_manifest_edges(&conn, "ent_open", 1_725_000_000_000).unwrap();
        assert_eq!(removed, 0, "non-strict brains must never be swept");
        assert_eq!(edge_ids(&conn).len(), 1);
    }
}
