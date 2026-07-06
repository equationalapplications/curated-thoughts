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

    let detail: serde_json::Value =
        app.invoke("get_entity_cmd", json!({ "entityId": entity_id }));
    assert_eq!(detail["summary"], "Updated summary.");
    assert!(detail["facts"].as_array().unwrap().is_empty());

    app.invoke::<()>("archive_entity_cmd", json!({ "entityId": entity_id }));

    let after_archive: Vec<serde_json::Value> = app.invoke(
        "list_entities_cmd",
        json!({ "sort": null, "filter": {} }),
    );
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
