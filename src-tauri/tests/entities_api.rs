mod helpers;

use helpers::TestApp;
use serde_json::json;

#[test]
fn entity_crud_via_tauri_commands() {
    let app = TestApp::new();

    let created: serde_json::Value = app.invoke(
        "create_entity_cmd",
        json!({
            "input": {
                "name": "Integration Entity",
                "entity_type": "project",
                "summary": "Initial summary."
            }
        }),
    );
    let entity_id = created["id"].as_str().expect("entity id");
    assert!(entity_id.starts_with("ent_"));
    assert_eq!(created["name"], "Integration Entity");

    let listed: Vec<serde_json::Value> = app.invoke(
        "list_entities_cmd",
        json!({ "sort": "name_asc", "filter": {} }),
    );
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], entity_id);
    assert_eq!(listed[0]["fact_count"], 0);

    app.invoke::<()>(
        "update_entity_summary_cmd",
        json!({ "entityId": entity_id, "summary": "Updated summary." }),
    );

    let detail: serde_json::Value = app.invoke("get_entity_cmd", json!({ "entityId": entity_id }));
    assert_eq!(detail["summary"], "Updated summary.");
    assert!(detail["facts"].as_array().unwrap().is_empty());

    app.invoke::<()>("archive_entity_cmd", json!({ "entityId": entity_id }));

    let after_archive: Vec<serde_json::Value> =
        app.invoke("list_entities_cmd", json!({ "sort": null, "filter": {} }));
    assert!(after_archive.is_empty());

    let with_archived: Vec<serde_json::Value> = app.invoke(
        "list_entities_cmd",
        json!({
            "sort": null,
            "filter": { "include_archived": true }
        }),
    );
    assert_eq!(with_archived.len(), 1);
}

/// Task 9 retired the app's direct `user_stated` INSERT (spec §3.7.6 + §9,
/// PR #294 verdict E3): `add_entity_fact_cmd` is now an async capture command
/// (stated/ file → attestation → synchronous librarian kick) and the R3
/// inline-write contract is gone with it. Its replacement coverage lives in
/// the lib suite: `capture_statement_writes_attested_file_and_kicks`,
/// `capture_statement_dedupes_duplicate_slugs`, and the real-pipeline E2E
/// (all in `src/db/wisdom.rs`). An integration test of the capture command
/// cannot run the production kick deterministically (no completer seam at
/// the command layer), so the command-level contract pinned here is only
/// what CI-exercised glue still needs: the EDIT twin's write-time embedding.
///
/// `update_entity_fact_cmd` is the edit twin and is UNCHANGED by Task 9: it
/// must embed at write time. It previously NULLed `embedding_blob` on every
/// edit and relied on the null-embedding sweep, which nothing on this path
/// triggers — so an edited fact dropped out of semantic retrieval until an
/// app restart or an unrelated write happened to fire a sweep. The seed fact
/// is inserted DIRECTLY (Task 9 fixture shape, mirroring `seed_fact` in
/// `src/db/wisdom.rs`) because the old seed route — `add_entity_fact_cmd` —
/// no longer writes `llm_wiki_entries`.
#[test]
fn update_entity_fact_cmd_writes_entry_embedding_inline() {
    temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
        let app = TestApp::new();
        let created: serde_json::Value = app.invoke(
            "create_entity_cmd",
            json!({
                "input": { "name": "Edit Target", "entity_type": null, "summary": null }
            }),
        );
        let entity_id = created["id"].as_str().expect("entity id").to_string();

        // Direct seed: one live fact row keyed to the entity (same column
        // shape the retired `add_wisdom_in_tx` INSERT used; `embed_scheme`
        // default 'raw' keeps the stub embedder deterministic).
        let fact_id = "fact_seed_edit_inline".to_string();
        {
            let conn = app.open_db();
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    source_hash, source_ref, created_at, updated_at, last_accessed_at,
                    access_count, deleted_at, embedding_blob, embed_scheme, embedding
                 ) VALUES (?1, ?2, ?3, ?4, '[]', 'confirmed', 'user_stated', NULL, NULL, ?5, ?5, NULL, 0, NULL, NULL, 'raw', NULL)",
                rusqlite::params![
                    fact_id,
                    entity_id,
                    "The body before the edit.",
                    "The body before the edit.",
                    1_700_000_000_i64,
                ],
            )
            .expect("seed fact row");
        }

        app.invoke::<()>(
            "update_entity_fact_cmd",
            json!({
                "entityId": entity_id,
                "factId": fact_id,
                "body": "The body after the edit."
            }),
        );

        let conn = app.open_db();
        let (body, blob_len): (String, Option<i64>) = conn
            .query_row(
                "SELECT body, length(embedding_blob) FROM llm_wiki_entries WHERE id = ?1",
                [&fact_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("query edited fact");
        assert_eq!(body, "The body after the edit.");
        assert_eq!(
            blob_len,
            Some(32),
            "the edit should land with a fresh embedding (8 floats x 4 bytes), \
             not NULL waiting on a sweep that this path never triggers"
        );
    });
}
