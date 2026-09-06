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
            "",
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

    let a = insert_chunk(&conn, doc_id, &make_def_chunk("a"), 0, "tier_fact", "")
        .expect("insert chunk a");
    let b = insert_chunk(&conn, doc_id, &make_def_chunk("b"), 1, "tier_fact", "")
        .expect("insert chunk b");
    let c = insert_chunk(&conn, doc_id, &make_def_chunk("c"), 2, "tier_fact", "")
        .expect("insert chunk c");
    let d = insert_chunk(&conn, doc_id, &make_def_chunk("d"), 3, "tier_fact", "")
        .expect("insert chunk d");
    let e = insert_chunk(&conn, doc_id, &make_def_chunk("e"), 4, "tier_fact", "")
        .expect("insert chunk e");

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

#[test]
fn get_chunk_ids_returns_evidence_anchors_for_token_rows() {
    let conn = open_in_memory().unwrap();
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status)
         VALUES ('notes.md','h','user_doc','indexed')",
        [],
    )
    .unwrap();
    let doc_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy,
             entity_id, content_hash)
         VALUES (?1,'c',0,1,1,'prose','ent','h1')",
        [doc_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
             source_type, source_ref, created_at, updated_at, access_count)
         VALUES ('fact_g','ent','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
        [tauri_app_lib::db::commit::librarian_source_ref_token(
            "fact_g",
        )],
    )
    .unwrap();
    tauri_app_lib::db::commit::insert_librarian_evidence(
        &conn,
        "fact_g",
        "prop_g",
        r#"{"evidence":[{"chunk_id":1,"content_hash":"h1"}],"proposal_id":"prop_g"}"#,
        false,
        1,
    )
    .unwrap();

    let ids = tauri_app_lib::chunk_ids_for_entry(&conn, "fact_g", "ent", None);
    assert!(
        !ids.is_empty(),
        "token rows must resolve their evidence anchors"
    );
}

#[test]
fn get_chunk_ids_returns_empty_for_token_row_without_evidence() {
    let conn = open_in_memory().unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
             source_type, source_ref, created_at, updated_at, access_count)
         VALUES ('fact_ne','ent','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
        [tauri_app_lib::db::commit::librarian_source_ref_token(
            "fact_ne",
        )],
    )
    .unwrap();
    // Empty, not a wrong-namespace fallback. The caller must render nothing.
    assert!(tauri_app_lib::chunk_ids_for_entry(&conn, "fact_ne", "ent", None).is_empty());
}
