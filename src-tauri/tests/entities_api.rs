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

/// Task R3 end-to-end: `add_entity_fact_cmd` now threads `EmbedProfileState`
/// into `add_fact_with_profile`, so a profile-aware write should land with a
/// non-NULL `embedding_blob` rather than deferring to the runtime sweep.
/// Uses `CURATED_EMBED_STUB=constant8` to make the embedder deterministic
/// without contacting Ollama.
#[test]
fn add_entity_fact_cmd_writes_entry_embedding_inline() {
    temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
        let app = TestApp::new();
        let created: serde_json::Value = app.invoke(
            "create_entity_cmd",
            json!({
                "input": {
                    "name": "Fact Target",
                    "entity_type": null,
                    "summary": null
                }
            }),
        );
        let entity_id = created["id"].as_str().expect("entity id").to_string();

        let fact: serde_json::Value = app.invoke(
            "add_entity_fact_cmd",
            json!({ "entityId": entity_id, "body": "A fact written with a profile." }),
        );
        let fact_id = fact["id"].as_str().expect("fact id").to_string();

        let conn = app.open_db();
        let blob_len: Option<i64> = conn
            .query_row(
                "SELECT length(embedding_blob) FROM llm_wiki_entries WHERE id = ?1",
                [&fact_id],
                |r| r.get(0),
            )
            .expect("query embedding_blob length");
        assert_eq!(
            blob_len,
            Some(32),
            "embedding_blob should be set inline (8 floats × 4 bytes)"
        );
    });
}
