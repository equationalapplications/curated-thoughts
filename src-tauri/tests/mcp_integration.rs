//! Spawns **`curated-thoughts`** over stdio with `--mcp` and exercises MCP tool calls (**`rmcp`** client).
//! Build the binary first: `cargo build --features mcp-server --manifest-path src-tauri/Cargo.toml`
//! Then run: `CURATED_MCP_INTEGRATION_TESTS=1 cargo test --manifest-path src-tauri/Cargo.toml --test mcp_integration`
//!
//! Skipped unless `CURATED_MCP_INTEGRATION_TESTS=1` is set (binary not available in default CI).

use std::path::{Path, PathBuf};

use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::embedder::{embed_one, EmbedProfile};
use tauri_app_lib::retrieval::{
    self, insert_chunk, insert_embedding, mark_document_indexed, upsert_document, AppDb,
};
use tauri_app_lib::search::SearchResult;
use tauri_app_lib::wiki_graph::{
    f32_vec_to_blob, WikiOntologyResult, WikiSearchHit, WikiTraverseResult,
};

fn mcp_exe() -> PathBuf {
    // After the unified-binary refactor, the main binary runs as an MCP server
    // when invoked with --mcp. Build it with --features mcp-server.
    // Cargo replaces hyphens with underscores in CARGO_BIN_EXE_* env var names.
    if let Some(p) = std::env::var_os("CARGO_BIN_EXE_curated_thoughts") {
        return PathBuf::from(p);
    }
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(profile)
        .join(if cfg!(windows) {
            "curated-thoughts.exe"
        } else {
            "curated-thoughts"
        })
}

fn first_text_hit(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .concat()
}

async fn spawn_mcp(
    brain_root: impl AsRef<Path>,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let brain = brain_root.as_ref().to_path_buf();
    let exe = mcp_exe();
    let transport = TokioChildProcess::new(tokio::process::Command::new(&exe).configure(|cmd| {
        cmd.arg("--mcp")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .env(
                "CURATED_BRAIN_DIR",
                brain.as_os_str().to_str().expect("UTF-8 brain path"),
            )
            .env("CURATED_EMBED_STUB", "constant8");
    }))?;
    let client = ().serve(transport).await?;
    Ok(client)
}

fn seed_fixture(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    let doc_id = upsert_document(conn, "/fixtures/mcp_semantic.md", "h_mcp")?;
    let chunk = Chunk {
        text: "mcp fixture chunk gamma".into(),
        start_line: 1,
        end_line: 2,
        symbol_name: Some("mcp_sym".into()),
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let cid = insert_chunk(conn, doc_id, &chunk, 0, "tier_fact")?;
    let profile = EmbedProfile::default();
    let v = embed_one(&profile, "q".into())?;
    insert_embedding(conn, cid, &v)?;
    mark_document_indexed(conn, doc_id)?;

    let doc_b = upsert_document(conn, "/fixtures/mcp_other.md", "h_b")?;
    let chunk_b = Chunk {
        text: "sidebar doc beta".into(),
        start_line: 1,
        end_line: 1,
        symbol_name: None,
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let cid_b = insert_chunk(conn, doc_b, &chunk_b, 0, "tier_fact")?;
    let v_b = embed_one(&profile, chunk_b.text.clone())?;
    insert_embedding(conn, cid_b, &v_b)?;
    mark_document_indexed(conn, doc_b)?;
    Ok(())
}

fn seed_wiki_fixture(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    // AppDb::open (via seed_fixture's caller) already runs V7 migrations; only insert rows.
    let blob = f32_vec_to_blob(&[1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            created_at, updated_at, embedding_blob
         ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'librarian_inferred', 1, 1, ?5)",
        ("seed-a", "tier_fact", "MCP seed A", "seed body A", blob.clone()),
    )?;
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            created_at, updated_at, embedding_blob
         ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'librarian_inferred', 1, 1, ?5)",
        ("seed-b", "tier_fact", "MCP seed B", "seed body B", blob),
    )?;
    conn.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type)
         VALUES ('edge-ab', 'tier_fact', 'seed-a', 'seed-b', 'relates')",
        [],
    )?;
    conn.execute(
        "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json)
         VALUES ('tier_fact', 'active', '{\"node_types\":[\"Fact\"],\"edge_types\":[\"relates\"]}')",
        [],
    )?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_lists_search_tools_and_semantic_returns_json_hits() {
    if std::env::var("CURATED_MCP_INTEGRATION_TESTS").is_err() {
        eprintln!("Skipping MCP integration test — set CURATED_MCP_INTEGRATION_TESTS=1 to run");
        return;
    }
    let root = tempdir().expect("tempdir");
    let brain = root.path();
    std::fs::write(brain.join("config.json"), b"{}\n").unwrap();

    with_vars(
        [
            ("CURATED_EMBED_STUB", Some("constant8")),
            ("CURATED_BRAIN_DIR", brain.to_str()),
        ],
        || {
            let paths = retrieval::resolve_brain_paths();
            let db_path = paths.db_path.clone();
            let db = AppDb::open(&db_path).unwrap();
            seed_fixture(&db.0).unwrap();
        },
    );

    assert!(
        mcp_exe().exists(),
        "MCP binary missing: {:?}\nbuild with:\n  cargo build --features mcp-server --manifest-path src-tauri/Cargo.toml",
        mcp_exe()
    );

    let client = spawn_mcp(brain).await.expect("mcp handshake");

    let tools = client.list_all_tools().await.expect("list_all_tools");
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.iter().any(|n| *n == "vault_semantic_search"));
    assert!(names.iter().any(|n| *n == "vault_related_chunks"));

    let args = serde_json::json!({
        "query": "q",
        "limit": 5,
    })
    .as_object()
    .unwrap()
    .clone();
    let res = client
        .peer()
        .call_tool(CallToolRequestParams::new("vault_semantic_search").with_arguments(args))
        .await
        .expect("call_tool semantic");
    let text = first_text_hit(&res);
    let parsed: Vec<SearchResult> = serde_json::from_str(&text)
        .expect("tool returns JSON SearchResult array (Tauri/MCP contract)");
    assert!(
        parsed
            .iter()
            .any(|row| row.symbol_name.as_deref() == Some("mcp_sym")),
        "semantic JSON missing mcp_sym: {text:?}"
    );
    assert!(
        parsed.iter().any(|row| !row.strategy.is_empty()),
        "semantic JSON should include chunk strategy: {text:?}"
    );

    let rel_args = serde_json::json!({
        "doc_path": "/fixtures/mcp_semantic.md",
        "limit": 10,
    })
    .as_object()
    .unwrap()
    .clone();
    let rel = client
        .peer()
        .call_tool(CallToolRequestParams::new("vault_related_chunks").with_arguments(rel_args))
        .await
        .expect("call_tool related");
    let rel_text = first_text_hit(&rel);
    let rel_parsed: Vec<SearchResult> = serde_json::from_str(&rel_text)
        .expect("related returns SearchResult array (Tauri/MCP contract)");
    assert!(
        rel_parsed
            .iter()
            .any(|row| row.doc_path == "/fixtures/mcp_other.md"),
        "related should rank chunks from the other fixtures doc; got {rel_text:?}"
    );

    client.cancel().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_lists_wiki_tools_and_returns_json() {
    if std::env::var("CURATED_MCP_INTEGRATION_TESTS").is_err() {
        eprintln!("Skipping MCP integration test — set CURATED_MCP_INTEGRATION_TESTS=1 to run");
        return;
    }
    let root = tempdir().expect("tempdir");
    let brain = root.path();
    std::fs::write(brain.join("config.json"), b"{}\n").unwrap();

    with_vars(
        [
            ("CURATED_EMBED_STUB", Some("constant8")),
            ("CURATED_BRAIN_DIR", brain.to_str()),
        ],
        || {
            let paths = retrieval::resolve_brain_paths();
            let db = AppDb::open(&paths.db_path).unwrap();
            seed_fixture(&db.0).unwrap();
            seed_wiki_fixture(&db.0).unwrap();
        },
    );

    assert!(
        mcp_exe().exists(),
        "MCP binary missing: {:?}\nbuild with:\n  cargo build --features mcp-server --manifest-path src-tauri/Cargo.toml",
        mcp_exe()
    );

    let client = spawn_mcp(brain).await.expect("mcp handshake");
    let tools = client.list_all_tools().await.expect("list_all_tools");
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    for tool in ["wiki_search", "wiki_get_ontology", "wiki_traverse_graph"] {
        assert!(names.iter().any(|n| *n == tool), "missing tool {tool}");
    }

    let search_args = serde_json::json!({ "query": "seed", "limit": 5 })
        .as_object()
        .unwrap()
        .clone();
    let search_res = client
        .peer()
        .call_tool(CallToolRequestParams::new("wiki_search").with_arguments(search_args))
        .await
        .expect("wiki_search");
    let search_hits: Vec<WikiSearchHit> =
        serde_json::from_str(&first_text_hit(&search_res)).expect("wiki_search JSON");
    assert!(search_hits.iter().any(|h| h.id == "seed-a"));

    let onto_args = serde_json::json!({ "entityId": "tier_fact" })
        .as_object()
        .unwrap()
        .clone();
    let onto_res = client
        .peer()
        .call_tool(CallToolRequestParams::new("wiki_get_ontology").with_arguments(onto_args))
        .await
        .expect("wiki_get_ontology");
    let onto: WikiOntologyResult =
        serde_json::from_str(&first_text_hit(&onto_res)).expect("ontology JSON");
    assert_eq!(onto.mode, "active");

    let traverse_args = serde_json::json!({
        "entityId": "tier_fact",
        "sourceId": "seed-a",
        "maxDepth": 1,
    })
    .as_object()
    .unwrap()
    .clone();
    let traverse_res = client
        .peer()
        .call_tool(CallToolRequestParams::new("wiki_traverse_graph").with_arguments(traverse_args))
        .await
        .expect("wiki_traverse_graph");
    let graph: WikiTraverseResult =
        serde_json::from_str(&first_text_hit(&traverse_res)).expect("traverse JSON");
    assert!(graph.nodes.iter().any(|n| n.id == "seed-b"));

    client.cancel().await.expect("shutdown");
}
