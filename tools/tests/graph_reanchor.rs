//! Migration correctness: the orphan predicate must treat a soft-deleted
//! endpoint as dead, not alive.
//!
//! Edge endpoints are heterogeneous (R1): an endpoint id may live in
//! `llm_wiki_entries`, `curated_entities`, or `llm_wiki_tasks`. An edge is
//! preserved as long as ONE endpoint resolves to at least one of the three
//! (with `deleted_at IS NULL`).

use rusqlite::{params, Connection};
use tauri_app_lib::db::connection::open_in_memory;

use curated_thoughts_tools::graph_reanchor::{count_orphan_edges, purge_orphan_edges};

fn seed_entry(conn: &Connection, id: &str, deleted_at_ms: Option<i64>) {
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embedding
         ) VALUES (?1, 'ent-1', 'T', 'B', '[]', 'inferred', 'librarian_inferred',
                   NULL, NULL, 100, 100, NULL, 0, ?2, NULL, NULL)",
        params![id, deleted_at_ms],
    )
    .unwrap();
}

fn seed_entity(conn: &Connection, id: &str, deleted_at_ms: Option<i64>) {
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at, deleted_at)
         VALUES (?1, 'n', 'concept', '', 100, 100, ?2)",
        params![id, deleted_at_ms],
    )
    .unwrap();
}

fn seed_task(conn: &Connection, id: &str, deleted_at_ms: Option<i64>) {
    conn.execute(
        "INSERT INTO llm_wiki_tasks (id, entity_id, description, status, priority,
            created_at, updated_at, resolved_at, deleted_at)
         VALUES (?1, 'ent-1', 'd', 'pending', 0, 100, 100, NULL, ?2)",
        params![id, deleted_at_ms],
    )
    .unwrap();
}

fn seed_edge(conn: &Connection, id: &str, source: &str, target: &str) {
    conn.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES (?1, 'ent-1', ?2, ?3, 'related_to', 100)",
        params![id, source, target],
    )
    .unwrap();
}

fn edge_ids(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT id FROM llm_wiki_edges ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
}

#[test]
fn purges_edges_whose_endpoints_are_absent_in_every_table() {
    // Shape of today's 41 production orphans (spec §4 Step 1, "all endpoints
    // are hard-absent"): BOTH `source_id` and `target_id` have no row in
    // llm_wiki_entries, curated_entities, or llm_wiki_tasks. The both-dead
    // predicate matches the runtime `db::edge_purge::purge_dead_edges`.
    let conn = open_in_memory().unwrap();
    seed_edge(&conn, "edge_orphan", "ghost_a", "ghost_b");

    let removed = purge_orphan_edges(&conn).unwrap();

    assert_eq!(removed, 1);
    assert!(edge_ids(&conn).is_empty());
    assert_eq!(count_orphan_edges(&conn).unwrap(), 0);
}

#[test]
fn keeps_edges_with_one_absent_endpoint_if_other_is_live() {
    // Counterpart to the soft-delete case above: a hard-absent endpoint is
    // "more dead" than a soft-deleted one, so the same both-endpoints-dead
    // contract holds — the live partner preserves the edge. Production has
    // no edges of this shape today (the 41 orphans have BOTH endpoints
    // absent), but pinning it here keeps a future regression from silently
    // widening the predicate to OR.
    let conn = open_in_memory().unwrap();
    seed_entry(&conn, "fact_live", None);
    seed_edge(&conn, "edge_one_dead", "ent_gone", "fact_live");

    let removed = purge_orphan_edges(&conn).unwrap();

    assert_eq!(
        removed, 0,
        "live partner preserves the edge — only edges with BOTH endpoints dead are purged"
    );
    assert_eq!(edge_ids(&conn), vec!["edge_one_dead".to_string()]);
}

#[test]
fn keeps_edges_with_one_live_endpoint_even_if_other_is_soft_deleted() {
    // Both-endpoints-dead contract: an edge survives the repair tool as long
    // as ONE endpoint is alive. A soft-deleted partner alone does not make
    // the edge an orphan. (The runtime's symmetric purge uses the same rule.)
    let conn = open_in_memory().unwrap();
    seed_entry(&conn, "fact_live", None);
    seed_entry(&conn, "fact_ghost", Some(200_000)); // soft-deleted
    seed_edge(&conn, "edge_ghost", "fact_live", "fact_ghost");

    let removed = purge_orphan_edges(&conn).unwrap();

    assert_eq!(
        removed, 0,
        "live partner preserves the edge — only edges with BOTH endpoints dead are purged"
    );
    assert_eq!(edge_ids(&conn), vec!["edge_ghost".to_string()]);
}

#[test]
fn keeps_edges_between_two_live_entries() {
    let conn = open_in_memory().unwrap();
    seed_entry(&conn, "fact_a", None);
    seed_entry(&conn, "fact_b", None);
    seed_edge(&conn, "edge_ab", "fact_a", "fact_b");

    let removed = purge_orphan_edges(&conn).unwrap();

    assert_eq!(removed, 0);
    assert_eq!(edge_ids(&conn), vec!["edge_ab".to_string()]);
    assert_eq!(count_orphan_edges(&conn).unwrap(), 0);
}

#[test]
fn purge_is_idempotent() {
    // Same both-absent fixture as `purges_edges_whose_endpoints_are_absent_*`.
    let conn = open_in_memory().unwrap();
    seed_edge(&conn, "edge_orphan", "ghost_a", "ghost_b");

    assert_eq!(purge_orphan_edges(&conn).unwrap(), 1);
    assert_eq!(
        purge_orphan_edges(&conn).unwrap(),
        0,
        "second run is a no-op"
    );
}

#[test]
fn keeps_edges_pointing_only_at_a_live_curated_entity() {
    // The R1 proof: an endpoint id present ONLY in curated_entities is alive.
    // A naive entry-only predicate would delete this edge.
    let conn = open_in_memory().unwrap();
    seed_entity(&conn, "ce_only", None);
    seed_edge(&conn, "edge_to_entity", "ce_only", "ce_only");

    let removed = purge_orphan_edges(&conn).unwrap();
    assert_eq!(
        removed, 0,
        "endpoint alive in curated_entities — edge survives"
    );
    assert_eq!(edge_ids(&conn), vec!["edge_to_entity".to_string()]);
}

#[test]
fn keeps_edges_pointing_only_at_a_live_task() {
    let conn = open_in_memory().unwrap();
    seed_task(&conn, "task_only", None);
    seed_edge(&conn, "edge_to_task", "task_only", "task_only");

    let removed = purge_orphan_edges(&conn).unwrap();
    assert_eq!(
        removed, 0,
        "endpoint alive in llm_wiki_tasks — edge survives"
    );
    assert_eq!(edge_ids(&conn), vec!["edge_to_task".to_string()]);
}

#[test]
fn treats_soft_deleted_endpoint_in_every_table_as_dead() {
    let conn = open_in_memory().unwrap();
    seed_entity(&conn, "ce_arc", Some(1));
    seed_task(&conn, "task_arc", Some(1));
    seed_entry(&conn, "fact_arc", Some(1));
    seed_edge(&conn, "edge_all_soft_deleted", "ce_arc", "task_arc");

    let removed = purge_orphan_edges(&conn).unwrap();
    assert_eq!(
        removed, 1,
        "soft-deleted in all three tables counts as dead"
    );
    assert!(edge_ids(&conn).is_empty());
}
