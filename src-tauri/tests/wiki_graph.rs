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
    blob: Option<&[u8]>,
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
    insert_entry(&conn, "f1", "tier_fact", "Fact node", Some(&fact), None);
    insert_entry(
        &conn,
        "w1",
        "tier_wisdom",
        "Wisdom node",
        Some(&wisdom),
        None,
    );

    let hits = wiki_search(&conn, &q, Some(&["tier_fact", "tier_wisdom"]), 10).unwrap();
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
    insert_entry(&conn, "bad", "tier_fact", "Bad trailing", Some(&bad), None);
    let hits = wiki_search(&conn, &q, Some(&["tier_fact"]), 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn wiki_search_skips_dimension_mismatch_without_error() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let bad = f32_vec_to_blob(&[1.0, 0.0, 0.0]); // 3-dim blob, query is 2-dim
    insert_entry(&conn, "bad", "tier_fact", "Bad dim", Some(&bad), None);
    let hits = wiki_search(&conn, &q, Some(&["tier_fact"]), 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn wiki_search_skips_null_embedding_blob_without_error() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(&conn, "null-blob", "tier_fact", "No embedding", None, None);
    insert_entry(
        &conn,
        "good",
        "tier_fact",
        "Has embedding",
        Some(&blob),
        None,
    );
    let hits = wiki_search(&conn, &q, Some(&["tier_fact"]), 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "good");
}

#[test]
fn wiki_search_respects_entity_ids_in_list() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(&conn, "w1", "tier_wisdom", "Wisdom", Some(&blob), None);
    let hits = wiki_search(&conn, &q, Some(&["tier_fact"]), 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn wiki_search_none_entity_ids_searches_all_live_entries() {
    // The #133 shape: every row carries an `ent_*` id, no `tier_*` row exists.
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(
        &conn,
        "fact_1",
        "ent_448a",
        "Ingest watchdog",
        Some(&blob),
        None,
    );
    insert_entry(
        &conn,
        "fact_2",
        "ent_409c",
        "Drain stall",
        Some(&blob),
        None,
    );

    let hits = wiki_search(&conn, &q, None, 10).unwrap();

    assert_eq!(
        hits.len(),
        2,
        "both ent_* entries must be searchable by default"
    );
    let ids: HashSet<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(ids.contains("fact_1") && ids.contains("fact_2"));
}

#[test]
fn wiki_search_explicit_empty_entity_ids_returns_empty() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(
        &conn,
        "fact_1",
        "ent_448a",
        "Ingest watchdog",
        Some(&blob),
        None,
    );

    let hits = wiki_search(&conn, &q, Some(&[]), 10).unwrap();

    assert!(
        hits.is_empty(),
        "an explicit empty filter still matches nothing"
    );
}

#[test]
fn wiki_search_none_still_applies_tier_weight_bonus() {
    // Mixed brain: equal cosine similarity, so only tier_weight can separate them.
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(&conn, "f1", "tier_fact", "Fact", Some(&blob), None);
    insert_entry(
        &conn,
        "e1",
        "ent_448a",
        "Entity-namespaced",
        Some(&blob),
        None,
    );

    let hits = wiki_search(&conn, &q, None, 10).unwrap();

    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].id, "f1",
        "tier_fact keeps its 1.5x bonus under the broadened filter"
    );
    assert!((hits[0].score - 1.5).abs() < 1e-5);
    assert!((hits[1].score - 1.0).abs() < 1e-5);
}

#[test]
fn wiki_search_none_excludes_deleted_and_unembedded_rows() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(&conn, "live", "ent_448a", "Live", Some(&blob), None);
    insert_entry(
        &conn,
        "gone",
        "ent_448a",
        "Soft-deleted",
        Some(&blob),
        Some(1),
    );
    insert_entry(&conn, "raw", "ent_448a", "No embedding", None, None);

    let hits = wiki_search(&conn, &q, None, 10).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "live");
}

#[test]
fn wiki_search_excludes_soft_deleted_rows() {
    let conn = open_entries_db();
    let q = vec![1.0_f32, 0.0];
    let blob = f32_vec_to_blob(&[1.0, 0.0]);
    insert_entry(&conn, "gone", "tier_fact", "Deleted", Some(&blob), Some(1));
    let hits = wiki_search(&conn, &q, Some(&["tier_fact"]), 10).unwrap();
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
            Some(&blob),
            None,
        );
    }
    let hits = wiki_search(&conn, &q, Some(&["tier_fact"]), 25).unwrap();
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

    let got =
        wiki_traverse_graph(&conn, "tier_fact", "a", 2, TraverseDirection::Outbound, &[]).unwrap();

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

    let got =
        wiki_traverse_graph(&conn, "tier_fact", "b", 1, TraverseDirection::Inbound, &[]).unwrap();
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

    let got =
        wiki_traverse_graph(&conn, "tier_fact", "a", 1, TraverseDirection::Outbound, &[]).unwrap();
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

    let got =
        wiki_traverse_graph(&conn, "tier_fact", "a", 1, TraverseDirection::Outbound, &[]).unwrap();
    let ids: HashSet<_> = got.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, HashSet::from(["a"]));
    assert!(got.edges.is_empty());
}

fn open_graph_db_with_entities() -> Connection {
    let conn = open_graph_db();
    // curated_entities has no entity_id column: scoping lives on the edge row.
    conn.execute_batch(
        "CREATE TABLE curated_entities (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            entity_type TEXT NOT NULL DEFAULT 'concept',
            summary TEXT NOT NULL DEFAULT '',
            summary_embedding BLOB,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            deleted_at INTEGER
        );",
    )
    .unwrap();
    conn
}

fn insert_curated_entity(conn: &Connection, id: &str, name: &str, deleted: bool) {
    conn.execute(
        "INSERT INTO curated_entities (id, name, created_at, updated_at, deleted_at)
         VALUES (?1, ?2, 0, 0, ?3)",
        rusqlite::params![id, name, deleted.then_some(1_i64)],
    )
    .unwrap();
}

#[test]
fn wiki_traverse_graph_resolves_seed_from_curated_entities() {
    // Seed exists only in curated_entities. Before heterogeneous resolution the
    // seed lookup failed and the whole result was empty; now the node resolves,
    // with name mapped onto title and entity_id set to the walked partition.
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "ce_1", "Ingest Watchdog", false);

    let result =
        wiki_traverse_graph(&conn, "ent_448a", "ce_1", 2, TraverseDirection::Both, &[]).unwrap();

    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].id, "ce_1");
    assert_eq!(
        result.nodes[0].title, "Ingest Watchdog",
        "name maps onto title"
    );
    assert_eq!(
        result.nodes[0].entity_id, "ent_448a",
        "entity_id is the walked partition"
    );
    assert!(!result.truncated);
}

#[test]
fn wiki_traverse_graph_ignores_soft_deleted_curated_entity_seed() {
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "ce_dead", "Forgotten", true);

    let result = wiki_traverse_graph(
        &conn,
        "ent_448a",
        "ce_dead",
        2,
        TraverseDirection::Both,
        &[],
    )
    .unwrap();

    assert!(
        result.nodes.is_empty(),
        "a soft-deleted seed resolves to nothing"
    );
    assert!(result.edges.is_empty());
}

#[test]
fn wiki_traverse_graph_prefers_the_entry_table_when_an_id_exists_in_both() {
    // Entry space wins: existing behavior must not change for entry-anchored data.
    let conn = open_graph_db_with_entities();
    insert_node(&conn, "dup", "tier_fact", "Entry Title", false);
    insert_curated_entity(&conn, "dup", "Entity Name", false);

    let result =
        wiki_traverse_graph(&conn, "tier_fact", "dup", 2, TraverseDirection::Both, &[]).unwrap();

    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].title, "Entry Title");
}

#[test]
fn wiki_traverse_graph_walks_entity_anchored_edges() {
    // The #134 shape: every edge endpoint lives in curated_entities.
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "ce_a", "Alpha", false);
    insert_curated_entity(&conn, "ce_b", "Beta", false);
    insert_curated_entity(&conn, "ce_c", "Gamma", false);
    insert_edge(&conn, "edge_ab", "ent_448a", "ce_a", "ce_b", "related_to");
    insert_edge(&conn, "edge_bc", "ent_448a", "ce_b", "ce_c", "related_to");

    let result =
        wiki_traverse_graph(&conn, "ent_448a", "ce_a", 2, TraverseDirection::Both, &[]).unwrap();

    let ids: HashSet<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        HashSet::from(["ce_a", "ce_b", "ce_c"]),
        "two hops reached"
    );
    assert_eq!(result.edges.len(), 2);
    assert!(!result.truncated);
}

#[test]
fn wiki_traverse_graph_excludes_soft_deleted_entity_endpoint() {
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "ce_a", "Alpha", false);
    insert_curated_entity(&conn, "ce_dead", "Gone", true);
    insert_edge(
        &conn,
        "edge_ad",
        "ent_448a",
        "ce_a",
        "ce_dead",
        "related_to",
    );

    let result =
        wiki_traverse_graph(&conn, "ent_448a", "ce_a", 2, TraverseDirection::Both, &[]).unwrap();

    assert_eq!(result.nodes.len(), 1, "only the seed survives");
    assert!(
        result.edges.is_empty(),
        "an edge to a dead endpoint is not walkable"
    );
}

#[test]
fn wiki_traverse_graph_entity_space_respects_edge_partition() {
    // An edge in another entity partition must not be walked.
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "ce_a", "Alpha", false);
    insert_curated_entity(&conn, "ce_other", "Other partition", false);
    insert_edge(
        &conn,
        "edge_x",
        "ent_OTHER",
        "ce_a",
        "ce_other",
        "related_to",
    );

    let result =
        wiki_traverse_graph(&conn, "ent_448a", "ce_a", 2, TraverseDirection::Both, &[]).unwrap();

    assert_eq!(result.nodes.len(), 1);
    assert!(result.edges.is_empty());
}

#[test]
fn wiki_traverse_graph_entity_space_filters_edge_types() {
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "ce_a", "Alpha", false);
    insert_curated_entity(&conn, "ce_b", "Beta", false);
    insert_curated_entity(&conn, "ce_c", "Gamma", false);
    insert_edge(&conn, "edge_ab", "ent_448a", "ce_a", "ce_b", "supports");
    insert_edge(&conn, "edge_ac", "ent_448a", "ce_a", "ce_c", "contradicts");

    let result = wiki_traverse_graph(
        &conn,
        "ent_448a",
        "ce_a",
        2,
        TraverseDirection::Both,
        &["supports"],
    )
    .unwrap();

    assert_eq!(result.edges.len(), 1);
    assert_eq!(result.edges[0].edge_type, "supports");
}

#[test]
fn wiki_traverse_graph_entity_space_direction_inbound_only() {
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "ce_a", "Alpha", false);
    insert_curated_entity(&conn, "ce_b", "Beta", false);
    insert_edge(&conn, "edge_ba", "ent_448a", "ce_b", "ce_a", "related_to");
    insert_edge(&conn, "edge_ac", "ent_448a", "ce_a", "ce_b", "related_to");

    let result = wiki_traverse_graph(
        &conn,
        "ent_448a",
        "ce_a",
        1,
        TraverseDirection::Inbound,
        &[],
    )
    .unwrap();

    assert_eq!(result.edges.len(), 1);
    assert_eq!(result.edges[0].source_id, "ce_b");
    assert_eq!(result.edges[0].target_id, "ce_a");
}

#[test]
fn wiki_traverse_graph_entity_space_truncates_at_max_nodes() {
    let conn = open_graph_db_with_entities();
    insert_curated_entity(&conn, "hub", "Hub", false);
    for i in 0..(MAX_TRAVERSAL_NODES + 10) {
        let id = format!("ce_{i}");
        insert_curated_entity(&conn, &id, &format!("Spoke {i}"), false);
        insert_edge(
            &conn,
            &format!("edge_{i}"),
            "ent_448a",
            "hub",
            &id,
            "related_to",
        );
    }

    let result =
        wiki_traverse_graph(&conn, "ent_448a", "hub", 2, TraverseDirection::Both, &[]).unwrap();

    assert!(result.nodes.len() <= MAX_TRAVERSAL_NODES);
    assert!(
        result.truncated,
        "an oversized entity graph must report truncation"
    );
}
