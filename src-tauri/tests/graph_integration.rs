// Phase 4 spec: Integration & "Impact Radius" Validation
//
// These tests verify that get_impact_radius correctly traces the recursive CTE
// up to 5 hops deep via the Tauri command layer. They are the integration
// counterpart to the unit tests in src-tauri/src/graph.rs and must stay in
// sync with any changes to the CALLEE_CTE / CALLER_CTE queries or the command's
// max_hops clamping logic (currently clamped to 5).
//
// CI breakage checklist: if you change graph.rs CTEs, update these tests.
// If you change the max_hops cap in get_impact_radius, update the clamping test.

mod helpers;
use helpers::TestApp;
use serde_json::json;
use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::{
    insert_chunk, insert_relationship, mark_document_indexed, upsert_document,
};

fn make_chunk(name: &str) -> Chunk {
    Chunk {
        text: format!("fn {}() {{}}", name),
        start_line: 1,
        end_line: 3,
        symbol_name: Some(name.to_string()),
        defined_symbol: Some(name.to_lowercase()),
        strategy: ChunkStrategyTag::AstSymbolRust,
    }
}

/// Seeds a linear 5-hop chain: a→b→c→d→e→f (5 edges).
/// Returns chunk IDs (a, b, c, d, e, f).
fn seed_five_hop_chain(
    conn: &rusqlite::Connection,
    entity_id: &str,
    path_suffix: &str,
) -> (i64, i64, i64, i64, i64, i64) {
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let doc = upsert_document(
        conn,
        &format!("/vault/documents/chain_{path_suffix}.rs"),
        "h",
    )
    .unwrap();
    mark_document_indexed(conn, doc).unwrap();
    let names = ["a", "b", "c", "d", "e", "f"];
    let ids: Vec<i64> = names
        .iter()
        .enumerate()
        .map(|(i, n)| insert_chunk(conn, doc, &make_chunk(n), i, entity_id).unwrap())
        .collect();
    for i in 0..5 {
        insert_relationship(conn, ids[i], ids[i + 1], "CALLS", names[i + 1], entity_id).unwrap();
    }
    (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5])
}

#[test]
fn impact_radius_callees_traverses_full_five_hop_chain() {
    let app = TestApp::new();
    let conn = app.open_db();
    let (a, b, c, d, e, f) = seed_five_hop_chain(&conn, "tier_fact", "callees5");
    drop(conn);

    let neighbors: Vec<serde_json::Value> = app.invoke(
        "get_impact_radius",
        json!({
            "rootChunkId": a,
            "entityId": "tier_fact",
            "direction": "callees",
            "maxHops": 5
        }),
    );

    let depths: std::collections::HashMap<i64, i64> = neighbors
        .iter()
        .map(|n| {
            (
                n["chunk_id"].as_i64().unwrap(),
                n["depth"].as_i64().unwrap(),
            )
        })
        .collect();

    assert_eq!(depths.get(&b), Some(&1), "b must be at depth 1");
    assert_eq!(depths.get(&c), Some(&2), "c must be at depth 2");
    assert_eq!(depths.get(&d), Some(&3), "d must be at depth 3");
    assert_eq!(depths.get(&e), Some(&4), "e must be at depth 4");
    assert_eq!(depths.get(&f), Some(&5), "f must be at depth 5");
}

#[test]
fn impact_radius_callers_traverses_full_five_hop_chain() {
    let app = TestApp::new();
    let conn = app.open_db();
    let (a, b, c, d, e, f) = seed_five_hop_chain(&conn, "tier_fact", "callers5");
    drop(conn);

    let neighbors: Vec<serde_json::Value> = app.invoke(
        "get_impact_radius",
        json!({
            "rootChunkId": f,
            "entityId": "tier_fact",
            "direction": "callers",
            "maxHops": 5
        }),
    );

    let depths: std::collections::HashMap<i64, i64> = neighbors
        .iter()
        .map(|n| {
            (
                n["chunk_id"].as_i64().unwrap(),
                n["depth"].as_i64().unwrap(),
            )
        })
        .collect();

    assert_eq!(depths.get(&e), Some(&1), "e must be at depth 1");
    assert_eq!(depths.get(&d), Some(&2), "d must be at depth 2");
    assert_eq!(depths.get(&c), Some(&3), "c must be at depth 3");
    assert_eq!(depths.get(&b), Some(&4), "b must be at depth 4");
    assert_eq!(depths.get(&a), Some(&5), "a must be at depth 5");
}

#[test]
fn impact_radius_max_hops_param_limits_traversal() {
    let app = TestApp::new();
    let conn = app.open_db();
    let (a, b, c, d, e, f) = seed_five_hop_chain(&conn, "tier_fact", "hoplimit");
    drop(conn);

    let neighbors: Vec<serde_json::Value> = app.invoke(
        "get_impact_radius",
        json!({
            "rootChunkId": a,
            "entityId": "tier_fact",
            "direction": "callees",
            "maxHops": 3
        }),
    );

    let ids: Vec<i64> = neighbors
        .iter()
        .map(|n| n["chunk_id"].as_i64().unwrap())
        .collect();

    assert!(ids.contains(&b), "b (depth 1) must appear");
    assert!(ids.contains(&c), "c (depth 2) must appear");
    assert!(ids.contains(&d), "d (depth 3) must appear");
    assert!(
        !ids.contains(&e),
        "e (depth 4) must not appear with max_hops=3"
    );
    assert!(
        !ids.contains(&f),
        "f (depth 5) must not appear with max_hops=3"
    );
    assert_eq!(ids.len(), 3);
}

#[test]
fn impact_radius_max_hops_clamped_to_five_by_command() {
    let app = TestApp::new();
    let conn = app.open_db();

    // Seed a 6-hop chain: n0→n1→…→n6
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let doc = upsert_document(&conn, "/vault/documents/deep_chain.rs", "h2").unwrap();
    mark_document_indexed(&conn, doc).unwrap();
    let nodes: Vec<i64> = (0..7usize)
        .map(|i| insert_chunk(&conn, doc, &make_chunk(&format!("n{i}")), i, "tier_fact").unwrap())
        .collect();
    for i in 0..6 {
        insert_relationship(
            &conn,
            nodes[i],
            nodes[i + 1],
            "CALLS",
            &format!("n{}", i + 1),
            "tier_fact",
        )
        .unwrap();
    }
    drop(conn);

    // Passing max_hops=10; command clamps to 5, so node at hop 6 must not appear.
    let neighbors: Vec<serde_json::Value> = app.invoke(
        "get_impact_radius",
        json!({
            "rootChunkId": nodes[0],
            "entityId": "tier_fact",
            "direction": "callees",
            "maxHops": 10
        }),
    );

    let ids: Vec<i64> = neighbors
        .iter()
        .map(|n| n["chunk_id"].as_i64().unwrap())
        .collect();

    assert!(
        ids.contains(&nodes[5]),
        "node at hop 5 must appear even with clamped max_hops"
    );
    assert!(
        !ids.contains(&nodes[6]),
        "node at hop 6 must NOT appear — max_hops is clamped to 5"
    );
}
