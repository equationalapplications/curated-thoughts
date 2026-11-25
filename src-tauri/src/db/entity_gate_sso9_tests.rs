//! sso9 MERGE-refresh planner tests (plan Task 4e). Child module of
//! `entity_gate` (declared there via `#[path]`), so it sees the private
//! planner items.

use super::*;
use serde_json::{json, Value};

const SSO_8_0_2: &str = include_str!("testdata/schema-software-org-8.0.2.manifest.json");

fn refreshed(json_in: &str) -> Value {
    match plan_sso9_manifest_refresh(json_in).unwrap() {
        RefreshPlan::Refresh { new_json } => serde_json::from_str(&new_json).unwrap(),
        other => panic!("expected Refresh, got {other:?}"),
    }
}

fn node_keys(v: &Value) -> Vec<String> {
    v["node_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| refresh_entry_key(e).unwrap())
        .collect()
}

#[test]
fn vendored_manifest_parses_and_declares_21_node_types() {
    let v: Value = serde_json::from_str(SSO_9X_VENDORED_MANIFEST_JSON).unwrap();
    let m = crate::wiki_graph::parse_manifest_value(&v);
    assert_eq!(m.node_types.len(), 21);
}

#[test]
fn sso_8_0_2_refreshes_to_the_9x_shape() {
    let out = refreshed(SSO_8_0_2);
    let keys = node_keys(&out);
    assert!(!keys.contains(&"handoff".to_string()));
    assert!(!keys.contains(&"session_recap".to_string()));
    for s in SSO9_ADDED_NODE_TYPES {
        assert!(keys.contains(&s.to_string()), "missing {s}");
    }
    assert_eq!(keys.len(), 21);
    let edges = out["edge_types"].as_array().unwrap();
    assert!(edges.iter().all(|e| e["type"] != "handoffFor"));
    assert_eq!(edges.len(), 42);
    // Impl-review (owx4): E2 requires the EXACT four new shapes, not just a
    // count — assert every (type, source_type, target_type) tuple.
    let triples: Vec<(String, String, String)> = edges
        .iter()
        .map(|e| refresh_edge_triple(e).expect("every edge entry carries a triple"))
        .collect();
    for want in [
        ("about", "statement", "area"),
        ("about", "statement", "software_application"),
        ("about", "statement", "organization"),
        ("partOf", "area", "software_application"),
    ] {
        // Triples are membership keys (NodeVocabulary::key is trim+lowercase):
        // normalize the wanted literals the same way.
        let want = (
            NodeVocabulary::key(want.0),
            NodeVocabulary::key(want.1),
            NodeVocabulary::key(want.2),
        );
        assert!(triples.contains(&want), "E2 shape missing: {want:?}");
    }
    // Surviving 8.x entries keep their relative order (new ones appended).
    let old: Value = serde_json::from_str(SSO_8_0_2).unwrap();
    let surviving: Vec<String> = old["node_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| refresh_entry_key(e).unwrap())
        .filter(|k| k != "handoff" && k != "session_recap")
        .collect();
    assert_eq!(&keys[..surviving.len()], surviving.as_slice());
    // New nodes are copied verbatim (description + parent_type) from vendored.
    let vend: Value = serde_json::from_str(SSO_9X_VENDORED_MANIFEST_JSON).unwrap();
    let find = |v: &Value, s: &str| -> Value {
        v["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| refresh_entry_key(e).as_deref() == Some(s))
            .unwrap()
            .clone()
    };
    for s in SSO9_ADDED_NODE_TYPES {
        assert_eq!(find(&out, s), find(&vend, s));
    }
}

#[test]
fn refresh_initializes_an_absent_edge_types_key() {
    // Impl-review (owx8): an eligible manifest may predate `edge_types`
    // entirely (`parse_manifest_value` reads it as an empty list) — the
    // refresh must still add the four E2-required shapes, `partOf` included,
    // instead of skipping the edge merge wholesale.
    let mut v: Value = serde_json::from_str(SSO_8_0_2).unwrap();
    v.as_object_mut().unwrap().remove("edge_types");
    let out = refreshed(&v.to_string());
    let edges = out["edge_types"].as_array().expect("edge_types initialized");
    let triples: Vec<(String, String, String)> = edges
        .iter()
        .map(|e| refresh_edge_triple(e).unwrap())
        .collect();
    for want in [
        ("about", "statement", "area"),
        ("about", "statement", "software_application"),
        ("about", "statement", "organization"),
        ("partOf", "area", "software_application"),
    ] {
        let want = (
            NodeVocabulary::key(want.0),
            NodeVocabulary::key(want.1),
            NodeVocabulary::key(want.2),
        );
        assert!(triples.contains(&want), "missing {want:?}");
    }
    assert!(!triples.iter().any(|t| t.0 == "handoffFor"));
}

#[test]
fn refreshed_manifest_is_ineligible_so_a_second_pass_is_untouched() {
    let first = match plan_sso9_manifest_refresh(SSO_8_0_2).unwrap() {
        RefreshPlan::Refresh { new_json } => new_json,
        o => panic!("{o:?}"),
    };
    assert!(matches!(
        plan_sso9_manifest_refresh(&first).unwrap(),
        RefreshPlan::Untouched(_)
    ));
}

#[test]
fn customised_manifest_keeps_its_extras_and_other_keys() {
    let mut v: Value = serde_json::from_str(SSO_8_0_2).unwrap();
    v["node_types"]
        .as_array_mut()
        .unwrap()
        .insert(2, json!({"type": "gadget", "description": "custom"}));
    v["edge_types"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "fooBar", "source_type": "gadget", "target_type": "person"}));
    v["fallback_node_type"] = json!("project");
    v["custom_key"] = json!({"x": 1});
    // A pre-declared new type must not be duplicated.
    v["node_types"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "Decision", "description": "mine"}));
    let out = refreshed(&v.to_string());
    let keys = node_keys(&out);
    assert!(keys.contains(&"gadget".to_string()));
    assert_eq!(keys.iter().filter(|k| *k == "decision").count(), 1);
    assert_eq!(out["fallback_node_type"], "project");
    assert_eq!(out["custom_key"], json!({"x": 1}));
    assert!(out["edge_types"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "fooBar"));
}

#[test]
fn bare_string_node_types_refresh() {
    let mut names: Vec<Value> = EA_SEED_TYPES_SSO_8X.iter().map(|s| json!(s)).collect();
    names.push(json!("custom"));
    let v = json!({"node_types": names, "edge_types": ["handoffFor", "knows"]});
    let out = refreshed(&v.to_string());
    let keys = node_keys(&out);
    assert!(!keys.contains(&"handoff".to_string()));
    assert!(keys.contains(&"lesson".to_string()));
    assert!(keys.contains(&"custom".to_string()));
    let edges = out["edge_types"].as_array().unwrap();
    assert!(!edges.iter().any(|e| e == "handoffFor"));
    assert!(edges.iter().any(|e| e == "knows"));
}

#[test]
fn foreign_manifest_is_untouched() {
    let v = json!({"node_types": [{"type": "widget"}], "fallback_node_type": "widget"});
    assert!(matches!(
        plan_sso9_manifest_refresh(&v.to_string()).unwrap(),
        RefreshPlan::Untouched(_)
    ));
}

#[test]
fn malformed_json_is_malformed() {
    assert!(matches!(
        plan_sso9_manifest_refresh("{not json").unwrap(),
        RefreshPlan::Malformed(_)
    ));
}

/// Spec §5 "Manifest-refresh test": an 8.x-era stored manifest was NOT an EA
/// subset under the 20-slug constants, passes after the refresh (+ the
/// document/process ensure is separate), and declares `decision`/`lesson`.
#[test]
fn refreshed_8x_manifest_declares_decision_lesson() {
    let old: Value = serde_json::from_str(SSO_8_0_2).unwrap();
    let old_vocab = NodeVocabulary::from_manifest(&crate::wiki_graph::parse_manifest_value(&old));
    assert!(!EA_SEED_TYPES.iter().all(|s| old_vocab.contains(s)));
    let out = refreshed(SSO_8_0_2);
    let vocab = NodeVocabulary::from_manifest(&crate::wiki_graph::parse_manifest_value(&out));
    assert!(vocab.contains("decision"));
    assert!(vocab.contains("lesson"));
    assert!(EA_SEED_TYPES.iter().all(|s| vocab.contains(s)));
}

#[test]
fn ensure_after_refresh_adds_document_process_and_fallback() {
    let out = refreshed(SSO_8_0_2).to_string();
    match plan_manifest_ensure(&out, "tier_fact", true).unwrap() {
        EnsurePlan::Edit {
            new_json,
            extended,
            fallback_set,
        } => {
            assert!(extended && fallback_set);
            let v: Value = serde_json::from_str(&new_json).unwrap();
            let keys = node_keys(&v);
            assert!(keys.contains(&"document".to_string()));
            assert!(keys.contains(&"process".to_string()));
            assert!(v["fallback_node_type"].is_string());
        }
        other => panic!("{other:?}"),
    }
}
