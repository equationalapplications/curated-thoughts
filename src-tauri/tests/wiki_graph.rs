use std::collections::HashSet;

use rusqlite::Connection;
use tauri_app_lib::wiki_graph::{
    f32_vec_to_blob, wiki_get_ontology, wiki_search, wiki_traverse_graph, TraverseDirection,
    WikiManifest, WikiOntologyResult, MAX_TRAVERSAL_NODES,
};

fn open_wiki_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE llm_wiki_entity_manifests (
            entity_id TEXT PRIMARY KEY,
            mode TEXT NOT NULL,
            manifest_json TEXT NOT NULL,
            updated_at INTEGER
        );",
    )
    .unwrap();
    conn
}

#[test]
fn wiki_get_ontology_returns_parsed_manifest() {
    let conn = open_wiki_db();
    conn.execute(
        "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json) VALUES (?1, ?2, ?3)",
        (
            "tier_fact",
            "active",
            r#"{"node_types":["Fact"],"edge_types":["supports"]}"#,
        ),
    )
    .unwrap();

    let got = wiki_get_ontology(&conn, "tier_fact").unwrap();
    assert_eq!(
        got,
        WikiOntologyResult {
            mode: "active".into(),
            manifest: Some(WikiManifest {
                node_types: vec!["Fact".into()],
                edge_types: vec!["supports".into()],
            }),
        }
    );
}

#[test]
fn wiki_get_ontology_missing_row_returns_off_mode() {
    let conn = open_wiki_db();
    let got = wiki_get_ontology(&conn, "tier_wisdom").unwrap();
    assert_eq!(
        got,
        WikiOntologyResult {
            mode: "off".into(),
            manifest: None,
        }
    );
}

fn open_entries_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE llm_wiki_entries (
            id TEXT PRIMARY KEY,
            entity_id TEXT NOT NULL,
            title TEXT NOT NULL,
            deleted_at INTEGER,
            embedding_blob BLOB
        );",
    )
    .unwrap();
    conn
}

fn insert_entry(
    conn: &Connection,
    id: &str,
    entity_id: &str,
    title: &str,
    blob: &[u8],
    deleted_at: Option<i64>,
) {
    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, deleted_at, embedding_blob)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, entity_id, title, deleted_at, blob],
    )
    .unwrap();
}

#[test]
fn wiki_search_applies_tier_weight_before_sort() {
    let conn = open_entries_db();
    // query [1,0], fact [1,0] => sim 1.0 * 1.5 = 1.5
    let q = vec![1.0_f32, 0.0];
    let fact = f32_vec_to_blob(&[1.0, 0.0]);
    let wisdom = f32_vec_to_blob(&[0.0, 1.0]); // sim 0.0
    insert_entry(&conn, "f1", "tier_fact", "Fact node", &fact, None);
    insert_entry(&conn, "w1", "tier_wisdom", "Wisdom node", &wisdom, None);

    let hits = wiki_search(&conn, &q, &["tier_fact", "tier_wisdom"], 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "f1");
    assert!((hits[0].score - 1.5).abs() < 1e-5);
}

#[test]
fn wiki_search_skips_extra_trailing_bytes_without_error() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let mut bad = f32_vec_to_blob(&[1.0, 0.0]);
    bad.extend_from_slice(&[0_u8, 1]); // dim*4 + 2 bytes
    insert_entry(&conn, "bad", "tier_fact", "Bad trailing", &bad, None);
    let hits = wiki_search(&conn, &q, &["tier_fact"], 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn wiki_search_skips_dimension_mismatch_without_error() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let bad = f32_vec_to_blob(&[1.0, 0.0, 0.0]); // 3-dim blob, query is 2-dim
    insert_entry(&conn, "bad", "tier_fact", "Bad dim", &bad, None);
    let hits = wiki_search(&conn, &q, &["tier_fact"], 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn wiki_search_respects_entity_ids_in_list() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(&conn, "w1", "tier_wisdom", "Wisdom", &blob, None);
    let hits = wiki_search(&conn, &q, &["tier_fact"], 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn wiki_search_excludes_soft_deleted_rows() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(&conn, "gone", "tier_fact", "Deleted", &blob, Some(1));
    let hits = wiki_search(&conn, &q, &["tier_fact"], 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn wiki_search_clamps_limit() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    for i in 0..30 {
        insert_entry(
            &conn,
            &format!("e{i}"),
            "tier_fact",
            &format!("t{i}"),
            &blob,
            None,
        );
    }
    let hits = wiki_search(&conn, &q, &["tier_fact"], 25).unwrap();
    assert_eq!(hits.len(), 25);
}

fn open_graph_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE llm_wiki_entries (
            id TEXT PRIMARY KEY,
            entity_id TEXT NOT NULL,
            title TEXT NOT NULL,
            deleted_at INTEGER
        );
        CREATE TABLE llm_wiki_edges (
            id TEXT PRIMARY KEY,
            entity_id TEXT NOT NULL,
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            edge_type TEXT NOT NULL,
            created_at INTEGER
        );",
    )
    .unwrap();
    conn
}

fn insert_node(conn: &Connection, id: &str, entity_id: &str, title: &str, deleted: bool) {
    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, deleted_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, entity_id, title, deleted.then_some(1_i64)],
    )
    .unwrap();
}

fn insert_edge(
    conn: &Connection,
    id: &str,
    entity_id: &str,
    source_id: &str,
    target_id: &str,
    edge_type: &str,
) {
    conn.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, entity_id, source_id, target_id, edge_type],
    )
    .unwrap();
}

#[test]
fn wiki_traverse_graph_multi_hop_bfs() {
    let conn = open_graph_db();
    insert_node(&conn, "a", "tier_fact", "A", false);
    insert_node(&conn, "b", "tier_fact", "B", false);
    insert_node(&conn, "c", "tier_fact", "C", false);
    insert_edge(&conn, "e1", "tier_fact", "a", "b", "relates");
    insert_edge(&conn, "e2", "tier_fact", "b", "c", "relates");

    let got = wiki_traverse_graph(
        &conn,
        "tier_fact",
        "a",
        2,
        TraverseDirection::Outbound,
        &[],
    )
    .unwrap();

    let ids: HashSet<_> = got.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains("a") && ids.contains("b") && ids.contains("c"));
    assert_eq!(got.edges.len(), 2);
    assert!(!got.truncated);
}

#[test]
fn wiki_traverse_graph_direction_inbound_only() {
    let conn = open_graph_db();
    insert_node(&conn, "a", "tier_fact", "A", false);
    insert_node(&conn, "b", "tier_fact", "B", false);
    insert_edge(&conn, "e1", "tier_fact", "a", "b", "relates");

    let got = wiki_traverse_graph(
        &conn,
        "tier_fact",
        "b",
        1,
        TraverseDirection::Inbound,
        &[],
    )
    .unwrap();
    let ids: HashSet<_> = got.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, HashSet::from(["a", "b"]));
}

#[test]
fn wiki_traverse_graph_filters_edge_types() {
    let conn = open_graph_db();
    insert_node(&conn, "a", "tier_fact", "A", false);
    insert_node(&conn, "b", "tier_fact", "B", false);
    insert_node(&conn, "c", "tier_fact", "C", false);
    insert_edge(&conn, "e1", "tier_fact", "a", "b", "supports");
    insert_edge(&conn, "e2", "tier_fact", "a", "c", "contradicts");

    let got = wiki_traverse_graph(
        &conn,
        "tier_fact",
        "a",
        1,
        TraverseDirection::Outbound,
        &["supports"],
    )
    .unwrap();
    assert_eq!(got.edges.len(), 1);
    assert_eq!(got.edges[0].edge_type, "supports");
}

#[test]
fn wiki_traverse_graph_excludes_deleted_endpoint() {
    let conn = open_graph_db();
    insert_node(&conn, "a", "tier_fact", "A", false);
    insert_node(&conn, "b", "tier_fact", "B", true);
    insert_edge(&conn, "e1", "tier_fact", "a", "b", "relates");

    let got = wiki_traverse_graph(
        &conn,
        "tier_fact",
        "a",
        1,
        TraverseDirection::Outbound,
        &[],
    )
    .unwrap();
    let ids: HashSet<_> = got.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, HashSet::from(["a"]));
    assert!(got.edges.is_empty());
}

#[test]
fn wiki_traverse_graph_unknown_source_returns_empty() {
    let conn = open_graph_db();
    let got = wiki_traverse_graph(
        &conn,
        "tier_fact",
        "missing",
        2,
        TraverseDirection::Both,
        &[],
    )
    .unwrap();
    assert!(got.nodes.is_empty() && got.edges.is_empty());
}

#[test]
fn wiki_traverse_graph_truncates_at_max_nodes() {
    let conn = open_graph_db();
    insert_node(&conn, "hub", "tier_fact", "Hub", false);
    for i in 0..=MAX_TRAVERSAL_NODES {
        let nid = format!("n{i}");
        insert_node(&conn, &nid, "tier_fact", &nid, false);
        insert_edge(&conn, &format!("e{i}"), "tier_fact", "hub", &nid, "relates");
    }
    let got = wiki_traverse_graph(
        &conn,
        "tier_fact",
        "hub",
        1,
        TraverseDirection::Outbound,
        &[],
    )
    .unwrap();
    assert!(got.truncated);
    assert!(got.nodes.len() <= MAX_TRAVERSAL_NODES);
    let node_ids: HashSet<_> = got.nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in &got.edges {
        assert!(node_ids.contains(edge.source_id.as_str()));
        assert!(node_ids.contains(edge.target_id.as_str()));
    }
}

#[test]
fn wiki_traverse_graph_excludes_cross_tier_endpoints() {
    let conn = open_graph_db();
    insert_node(&conn, "a", "tier_fact", "A", false);
    insert_node(&conn, "b", "tier_wisdom", "B", false);
    insert_edge(&conn, "e1", "tier_fact", "a", "b", "relates");

    let got = wiki_traverse_graph(
        &conn,
        "tier_fact",
        "a",
        1,
        TraverseDirection::Outbound,
        &[],
    )
    .unwrap();
    let ids: HashSet<_> = got.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, HashSet::from(["a"]));
    assert!(got.edges.is_empty());
}
