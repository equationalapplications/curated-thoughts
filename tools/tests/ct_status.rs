use std::process::Command;
use tempfile::TempDir;

/// Seed a minimal brain.db with the tables `ct status` reads.
fn seed_db(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE documents (id INTEGER PRIMARY KEY, path TEXT);
        CREATE TABLE chunks (id INTEGER PRIMARY KEY, doc_id INTEGER, chunk_text TEXT, position INTEGER);
        CREATE TABLE llm_wiki_entries (id INTEGER PRIMARY KEY, deleted_at INTEGER NULL);
        CREATE TABLE curated_proposals (id INTEGER PRIMARY KEY, status TEXT);
        CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
        INSERT INTO schema_version (version) VALUES (10);
        INSERT INTO documents (path) VALUES ('a.md'), ('b.md');
        INSERT INTO chunks (doc_id) VALUES (1), (2), (1);
        INSERT INTO llm_wiki_entries (deleted_at) VALUES (NULL), (7);
        INSERT INTO curated_proposals (status) VALUES ('pending'), ('approved');
        "#,
    )
    .unwrap();
}

#[test]
fn status_json_exits_zero_and_prints_expected_keys() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap().to_string();
    seed_db(tmp.path());

    let out = Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", &dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(["status", "--json"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "ct status --json failed: {} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for key in [
        "docs",
        "chunks",
        "wiki_entries",
        "proposals_pending",
        "db_path",
        "schema_version",
        "last_ingest_run",
    ] {
        assert!(v.get(key).is_some(), "missing key {key} in {v}");
    }
    assert_eq!(v["docs"], 2);
    assert_eq!(v["chunks"], 3);
    assert_eq!(v["wiki_entries"], 1); // deleted_at IS NULL only
    assert_eq!(v["proposals_pending"], 1);
    assert_eq!(v["schema_version"], 10);
}

#[test]
fn unknown_subcommand_exits_one() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap().to_string();

    let out = Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", &dir)
        .arg("bogus-subcommand")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(1));
}
