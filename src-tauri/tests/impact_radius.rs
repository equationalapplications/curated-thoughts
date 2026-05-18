use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::connection::open_in_memory;
use tauri_app_lib::db::queries::{
    insert_chunk, insert_relationship, mark_document_indexed, upsert_document,
};
use tauri_app_lib::graph;

fn make_def_chunk(name: &str) -> Chunk {
    Chunk {
        text: format!("fn {}() {{}}", name),
        start_line: 1,
        end_line: 3,
        symbol_name: Some(name.to_string()),
        defined_symbol: Some(name.to_lowercase()),
        strategy: ChunkStrategyTag::AstSymbolRust,
    }
}

#[test]
fn impact_radius_caps_at_five_hops() {
    let conn = open_in_memory().expect("open in-memory database");
    let doc_id = upsert_document(&conn, "/vault/documents/deep_chain.rs", "h").expect("upsert doc");
    mark_document_indexed(&conn, doc_id).expect("mark document indexed");

    let mut chunks = Vec::new();
    for idx in 0..7 {
        let chunk_id = insert_chunk(
            &conn,
            doc_id,
            &make_def_chunk(&format!("f{}", idx)),
            idx,
            "tier_fact",
        )
        .expect("insert chunk");
        chunks.push(chunk_id);
    }

    for idx in 0..6 {
        insert_relationship(
            &conn,
            chunks[idx],
            chunks[idx + 1],
            "CALLS",
            &format!("f{}", idx + 1),
            "tier_fact",
        )
        .expect("insert relationship");
    }

    let neighbors = graph::get_both(&conn, chunks[0], "tier_fact", 5).expect("impact radius query");
    assert!(
        neighbors.iter().all(|n| n.depth <= 5),
        "no neighbors beyond 5 hops"
    );
    assert!(
        neighbors.iter().any(|n| n.chunk_id == chunks[5]),
        "depth-5 neighbor should be included"
    );
    assert!(
        !neighbors.iter().any(|n| n.chunk_id == chunks[6]),
        "depth-6 neighbor should be excluded"
    );
}

#[test]
fn impact_radius_traverses_recursive_call_graphs_in_both_directions() {
    let conn = open_in_memory().expect("open in-memory database");
    let doc_id = upsert_document(&conn, "/vault/documents/recursive.rs", "h").expect("upsert doc");
    mark_document_indexed(&conn, doc_id).expect("mark document indexed");

    let a =
        insert_chunk(&conn, doc_id, &make_def_chunk("a"), 0, "tier_fact").expect("insert chunk a");
    let b =
        insert_chunk(&conn, doc_id, &make_def_chunk("b"), 1, "tier_fact").expect("insert chunk b");
    let c =
        insert_chunk(&conn, doc_id, &make_def_chunk("c"), 2, "tier_fact").expect("insert chunk c");
    let d =
        insert_chunk(&conn, doc_id, &make_def_chunk("d"), 3, "tier_fact").expect("insert chunk d");
    let e =
        insert_chunk(&conn, doc_id, &make_def_chunk("e"), 4, "tier_fact").expect("insert chunk e");

    insert_relationship(&conn, a, b, "CALLS", "b", "tier_fact").expect("insert relationship a->b");
    insert_relationship(&conn, b, c, "CALLS", "c", "tier_fact").expect("insert relationship b->c");
    insert_relationship(&conn, c, d, "CALLS", "d", "tier_fact").expect("insert relationship c->d");
    insert_relationship(&conn, e, a, "CALLS", "a", "tier_fact").expect("insert relationship e->a");

    let neighbors = graph::get_both(&conn, a, "tier_fact", 5).expect("impact radius query");
    let depth_by_chunk: std::collections::HashMap<i64, i64> = neighbors
        .into_iter()
        .map(|n| (n.chunk_id, n.depth))
        .collect();

    assert_eq!(
        depth_by_chunk.get(&b),
        Some(&1),
        "B should be a direct callee of A"
    );
    assert_eq!(
        depth_by_chunk.get(&c),
        Some(&2),
        "C should be reachable via B at depth 2"
    );
    assert_eq!(
        depth_by_chunk.get(&d),
        Some(&3),
        "D should be reachable via C at depth 3"
    );
    assert_eq!(
        depth_by_chunk.get(&e),
        Some(&1),
        "E should be recognized as a direct caller of A"
    );
}
