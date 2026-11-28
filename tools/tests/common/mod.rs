//! Shared test fixtures for tools integration tests: seed a temp brain vault
//! via the real retrieval API (Task 2 pattern from shared_queries.rs).
#![allow(dead_code)]

pub mod ct_age;

use std::fs;
use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::embedder::embed_one;
use tauri_app_lib::retrieval::{
    self, AppDb, insert_chunk, insert_embedding, mark_document_indexed, upsert_document,
};

pub const AST_CHUNK_TEXT: &str = "fn my_fn() { todo!() }";
pub const DOC_PATH: &str = "/vault/code.rs";

/// Seed a temp brain vault with one ast-strategy chunk defining `my_fn`
/// (with embedding) and one prose chunk (also with an embedding).
pub fn seed_vault(brain_path: &std::path::Path) -> i64 {
    fs::write(brain_path.join("config.json"), b"{}\n").unwrap();
    let paths = retrieval::resolve_brain_paths();
    let db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .expect("writable brain db open");
    let doc_id = upsert_document(&db.0, DOC_PATH, "h_fixture").unwrap();

    let ast_chunk = Chunk {
        text: AST_CHUNK_TEXT.into(),
        start_line: 1,
        end_line: 1,
        symbol_name: Some("my_fn".into()),
        defined_symbol: Some("my_fn".into()),
        strategy: ChunkStrategyTag::AstSymbolRust,
    };
    let chunk_id = insert_chunk(&db.0, doc_id, &ast_chunk, 0, "ent_fixture", "chash1").unwrap();

    let prose_chunk = Chunk {
        text: "plain prose notes about my_fn usage".into(),
        start_line: 2,
        end_line: 2,
        symbol_name: None,
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let prose_id = insert_chunk(&db.0, doc_id, &prose_chunk, 1, "ent_fixture", "chash2").unwrap();

    let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();
    for (rowid, text) in [
        (chunk_id, ast_chunk.text.clone()),
        (prose_id, prose_chunk.text.clone()),
    ] {
        let v = embed_one(&profile, text).unwrap();
        insert_embedding(&db.0, rowid, &v).unwrap();
    }
    mark_document_indexed(&db.0, doc_id, "seed-h").unwrap();
    chunk_id
}

/// Open the (existing) brain.db schema at `dir` by going through the real
/// AppDb migration path, so seeded rows always match production DDL.
pub fn init_brain_db(brain_dir: &std::path::Path) {
    fs::write(brain_dir.join("config.json"), b"{}\n").unwrap();
    let paths = retrieval::resolve_brain_paths();
    let _db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .expect("writable brain db open");
}

/// Insert a pending `new_entity` proposal with `item_count` items directly via
/// SQL matching the real DDL (`src-tauri/src/db/okf_ddl.rs`). Requires
/// `init_brain_db` to have run first.
pub fn insert_pending_proposal(
    brain_dir: &std::path::Path,
    id: &str,
    item_count: usize,
    created_at: i64,
) {
    let conn = rusqlite::Connection::open(brain_dir.join("brain.db")).expect("open brain.db rw");
    conn.execute(
        "INSERT INTO curated_proposals (
            id, kind, entity_id, proposed_name, proposed_type, reasoning, model, status, created_at
         ) VALUES (?1, 'new_entity', NULL, ?2, NULL, NULL, 'fixture-model', 'pending', ?3)",
        rusqlite::params![id, format!("Entity {id}"), created_at],
    )
    .unwrap();
    for i in 0..item_count {
        conn.execute(
            "INSERT INTO curated_proposal_items (
                id, proposal_id, item_type, target_id, payload, evidence, status
             ) VALUES (?1, ?2, 'fact_add', NULL, ?3, '[]', 'pending')",
            rusqlite::params![
                format!("{id}-item-{i}"),
                id,
                format!(r#"{{"body":"fact {i}","tags":[],"confidence":"inferred"}}"#)
            ],
        )
        .unwrap();
    }
}

/// Run `f` with a freshly seeded temp brain as CURATED_BRAIN_DIR and stub
/// embeddings enabled. `run_ct` must be used inside `f`.
pub fn with_seeded_brain<F: FnOnce()>(f: F) {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();
    let brain_path_str = brain_path.to_str().unwrap().to_string();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        move || {
            seed_vault(&brain_path);
            f();
        },
    );
}

/// Invoke the compiled `ct` binary against the ambient (temp) brain.
pub fn run_ct(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .args(args)
        .output()
        .expect("spawn ct")
}

/// Run `ct` with ADDITIONAL env vars layered over this test process's env
/// (which `with_seeded_brain` has already pointed at the temp brain).
pub fn run_ct_with_env<const N: usize>(args: &[&str], vars: [(&str, &str); N]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ct"));
    cmd.args(args);
    for (k, v) in vars {
        // An empty value REMOVES the var (stub opt-out must not leak a
        // resolved empty string into gate_model_key, which filters empties —
        // explicit is safer than relying on that filter).
        if v.is_empty() {
            cmd.env_remove(k);
        } else {
            cmd.env(k, v);
        }
    }
    cmd.output().expect("spawn ct")
}

/// The seeded brain's raw SQLite handle (fixture brain DB path via the
/// `CURATED_BRAIN_DIR` that `with_seeded_brain` set).
pub fn brain_db() -> rusqlite::Connection {
    let dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    rusqlite::Connection::open(std::path::Path::new(&dir).join("brain.db")).unwrap()
}

/// Seed one wisdom fact matching ct_wisdom_match.rs's pattern: a blob
/// `[1,0,…]` (stub query vectors share the direction ⇒ cosine ≈ 1) scoring
/// above the `stub:constant8` floor (0.5).
pub fn seed_fact(id: &str) {
    let mut v = [0f32; 8];
    v[0] = 1.0;
    let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    brain_db()
        .execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent_t', ?2, ?3, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?4, NULL)",
            rusqlite::params![id, format!("Title {id}"), format!("Body {id}"), blob],
        )
        .unwrap();
}

pub fn seeded_fact_with_entity(id: &str, entity_id: &str, area: Option<&str>) {
    let mut v = [0f32; 8];
    v[0] = 1.0;
    let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    brain_db()
        .execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, area, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, ?3, ?4, ?5, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?6, NULL)",
            rusqlite::params![
                id,
                entity_id,
                area,
                format!("Title {id}"),
                format!("Body {id}"),
                blob
            ],
        )
        .unwrap();
}

pub fn seed_entity(id: &str) {
    brain_db()
        .execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?1, 'repo', '', 100, 100)",
            rusqlite::params![id],
        )
        .unwrap();
}

/// The scoped fixture: a real checkout dir with `.ct-areas.yaml`
/// (`auth: ["src/auth/**"]`), live `hub-1` + `other-1` entities, facts in
/// the auth area / whole-repo / foreign repo — and the brain's config.json
/// rewritten with the `wisdom` block pointing at the areas file.
/// `org` chooses the configured `org_entity_id` (live `org-1`, a dead id,
/// or absent → no org band configured).
pub fn with_scoped_brain<F: FnOnce(&std::path::Path)>(org: Option<&str>, f: F) {
    with_seeded_brain(|| {
        let brain_dir = std::env::var("CURATED_BRAIN_DIR").unwrap();
        // tempdir(): the fixture checkout drops with the closure (m1 — no
        // `ct-scoped-fixture-*` leakage under the system temp dir).
        let checkout_tmp = tempfile::tempdir().unwrap();
        let checkout = checkout_tmp.path();
        std::fs::create_dir_all(checkout.join("src/auth")).unwrap();
        std::fs::create_dir_all(checkout.join("docs")).unwrap();
        std::fs::write(
            checkout.join(".ct-areas.yaml"),
            "areas:\n  auth:\n    - \"src/auth/**\"\n",
        )
        .unwrap();
        seed_entity("hub-1");
        seed_entity("other-1");
        if org == Some("org-1") {
            seed_entity("org-1");
        }
        seeded_fact_with_entity("fact_area", "hub-1", Some("auth"));
        seeded_fact_with_entity("fact_repo", "hub-1", None);
        seeded_fact_with_entity("fact_foreign", "other-1", None);
        let areas_file = checkout.join(".ct-areas.yaml");
        // json!-built config (m2): entity-id keys are literals, paths are
        // VALUES — no display()-splicing quoting/backslash hazards.
        let mut areas_paths = serde_json::Map::new();
        areas_paths.insert(
            "hub-1".to_string(),
            serde_json::Value::String(areas_file.display().to_string()),
        );
        let mut wisdom = serde_json::Map::new();
        wisdom.insert(
            "areas_paths".to_string(),
            serde_json::Value::Object(areas_paths),
        );
        if let Some(id) = org {
            wisdom.insert(
                "org_entity_id".to_string(),
                serde_json::Value::String(id.to_string()),
            );
        }
        let config =
            serde_json::json!({ "wisdom": serde_json::Value::Object(wisdom) }).to_string();
        std::fs::write(
            std::path::Path::new(&brain_dir).join("config.json"),
            config,
        )
        .unwrap();
        f(checkout);
    });
}
