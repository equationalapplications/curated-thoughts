//! The #186 acceptance gate. Seeds a scratch brain.db with CT-shaped rows,
//! runs the REAL installed engine's setup(), and asserts it rewrites nothing.
//!
//! Marked #[ignore] because it shells out to node, which local `cargo test`
//! runs cannot assume. Run explicitly:
//!   cargo test -p curated-thoughts --test engine_source_ref_gate -- --ignored
//!
//! CI runs it on every push (review round 5, finding 6): the
//! "Engine source_ref acceptance gate" step in ci.yml executes this test with
//! `--ignored` against the pnpm-installed core-llm-wiki, so a future engine
//! bump whose selector re-matches CT tokens fails the build instead of
//! shipping green while the hand-transcribed `engine_would_rewrite` still
//! says false.
//!
//! On main pre-fix this doubles as the real-repro proof that the shipped
//! engine mangles JSON refs (seed a JSON source_ref instead of a token; see
//! scripts/engine-setup-probe.mjs, which prints every changed row).

use tauri_app_lib::db::connection::open_app_db;

#[test]
#[ignore = "requires node >= 22.5 (built-in sqlite) + installed core-llm-wiki"]
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

    // The gate is only meaningful against the pinned engine: assert the
    // version the probe reports is exactly 7.1.0 before checking the
    // source_refs, so drift between the installed and pinned versions fails
    // loudly instead of silently passing against the wrong engine. The probe
    // degrades to 'unknown' when pnpm cannot read the installed version —
    // install workspace deps first in that case.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("probe output was not JSON ({e}): {stdout}"));
    assert_eq!(
        report["engineVersion"].as_str(),
        Some("7.1.0"),
        "engine-in-the-loop gate must run against core-llm-wiki 7.1.0, got {:?}. \
         A value of \"unknown\" means pnpm could not read the installed package \
         version — run `pnpm install` before this gate",
        report["engineVersion"]
    );
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
