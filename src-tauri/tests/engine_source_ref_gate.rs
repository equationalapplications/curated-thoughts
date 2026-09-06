//! The #186 acceptance gate. Seeds a scratch brain.db with CT-shaped rows,
//! runs the REAL installed engine's setup(), and asserts it rewrites nothing.
//!
//! Marked #[ignore] because it shells out to node. Run explicitly:
//!   cargo test -p curated-thoughts --test engine_source_ref_gate -- --ignored
//!
//! On main pre-fix this doubles as the real-repro proof that the shipped
//! engine mangles JSON refs (seed a JSON source_ref instead of a token; see
//! scripts/engine-setup-probe.mjs, which prints every changed row).

use tauri_app_lib::db::connection::open_app_db;

#[test]
#[ignore = "requires node + installed core-llm-wiki"]
fn installed_engine_setup_does_not_rewrite_ct_source_refs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("brain.db");
    let conn = open_app_db(&db_path, None).unwrap();

    let token = tauri_app_lib::db::commit::librarian_source_ref_token("fact_gate");
    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
             source_type, source_ref, created_at, updated_at, access_count)
         VALUES ('fact_gate','ent','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
        [&token],
    )
    .unwrap();
    drop(conn);

    let out = std::process::Command::new("node")
        .arg("scripts/engine-setup-probe.mjs")
        .arg(&db_path)
        .current_dir(env!("CARGO_MANIFEST_DIR").to_string() + "/..")
        .output()
        .expect("node must be available");
    assert!(
        out.status.success(),
        "probe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Engine version is recorded in the assertion output so drift between the
    // installed and pinned versions is visible in every run, never silent.
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("engine-in-the-loop gate ran against: {stdout}");

    let conn = open_app_db(&db_path, None).unwrap();
    let after: String = conn
        .query_row(
            "SELECT source_ref FROM llm_wiki_entries WHERE id='fact_gate'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        after, token,
        "the engine's setup() must not touch CT token rows"
    );
}
