//! Leniency policy tests: per-field drops, hard errors, missing blocks.

use std::fs;
use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::BrainPaths;
use tempfile::TempDir;

fn temp_paths(json: &str) -> (TempDir, BrainPaths) {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let brain_dir = temp.path().to_path_buf();
    if !json.is_empty() {
        fs::write(&config_path, json).unwrap();
    }
    let paths = BrainPaths {
        brain_dir,
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };
    (temp, paths)
}

#[test]
fn leniency_drop_unknown_embed_variant() {
    let json = r#"{"vault_path":"~/v","embed_profile":"unknown_variant","generation":{},"embedding":{},"privacy":{}}"#;
    let (_temp, paths) = temp_paths(json);

    let report = BrainConfig::load_lenient(&paths).unwrap();
    assert_eq!(report.config.embed_profile, None);
    assert!(report
        .diagnostics
        .iter()
        .any(|d| d.contains("embed_profile")));
}

#[test]
fn leniency_hard_fail_on_malformed_json() {
    let (_temp, paths) = temp_paths("{ invalid }");

    // Malformed top-level JSON is propagated as a typed ConfigError.
    let result = BrainConfig::load_lenient(&paths);
    assert!(result.is_err(), "malformed JSON must be fatal");
}

#[test]
fn leniency_hard_fail_on_unparseable_vault_path() {
    // vault_path present but not a string — propagated as a typed
    // ConfigError (the previous contract returned Ok with a diagnostic,
    // forcing callers to string-match; the typed contract is unambiguous).
    let json = r#"{"vault_path":123,"generation":{},"embedding":{},"privacy":{}}"#;
    let (_temp, paths) = temp_paths(json);

    let result = BrainConfig::load_lenient(&paths);
    assert!(result.is_err(), "non-string vault_path must be fatal");
}

#[test]
fn leniency_missing_blocks_marked() {
    let json = r#"{"vault_path":"~/v"}"#;
    let (_temp, paths) = temp_paths(json);

    let report = BrainConfig::load_lenient(&paths).unwrap();
    assert!(report.generation_missing);
    assert!(report.embedding_missing);
    assert!(report.privacy_missing);
    assert!(!report.vault_path_missing);
}

#[test]
fn leniency_missing_vault_path_marked() {
    let json = r#"{"generation":{},"embedding":{},"privacy":{}}"#;
    let (_temp, paths) = temp_paths(json);

    let report = BrainConfig::load_lenient(&paths).unwrap();
    assert!(report.vault_path_missing);
}

// ---------------------------------------------------------------------------
// Load-boundary validation of TrustedLink::link (issue #140).
// The predicate `is_vault_relative_link` is shared with the approval write
// path (PR #144); here it is applied to every ledger entry as it is read
// back from config.json, so a hand-edited ledger cannot smuggle an absolute,
// rooted, or `..`-traversal link past the walker.
// ---------------------------------------------------------------------------

/// Serialize `entries` as a config.json body with the `trusted_links`
/// array set to the given slice. Accepts any mix of entries — callers
/// below use it with one well-formed entry plus one (or more) bad `link`
/// values to prove lenient drop-the-bad-keep-the-rest semantics.
fn trusted_links_json(entries: &[serde_json::Value]) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "vault_path".to_string(),
        serde_json::Value::String("~/v".to_string()),
    );
    obj.insert("generation".to_string(), serde_json::json!({}));
    obj.insert("embedding".to_string(), serde_json::json!({}));
    obj.insert("privacy".to_string(), serde_json::json!({}));
    obj.insert(
        "trusted_links".to_string(),
        serde_json::Value::Array(entries.to_vec()),
    );
    serde_json::Value::Object(obj).to_string()
}

fn wellformed_entry(link: &str) -> serde_json::Value {
    serde_json::json!({
        "link": link,
        "target": "/tmp/somewhere",
        "approved_at": 1_700_000_000i64,
    })
}

#[test]
fn load_lenient_rejects_absolute_trusted_link() {
    let json = trusted_links_json(&[
        wellformed_entry("documents/specs"),
        wellformed_entry("/etc/outside-link"),
    ]);
    let (_temp, paths) = temp_paths(&json);

    let report = BrainConfig::load_lenient(&paths).unwrap();
    let links: Vec<&str> = report
        .config
        .trusted_links
        .iter()
        .map(|e| e.link.as_str())
        .collect();
    assert_eq!(links, vec!["documents/specs"]);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.contains("not vault-relative") && d.contains("/etc/outside-link")),
        "expected a 'not vault-relative' diagnostic echoing the link, got: {:?}",
        report.diagnostics
    );
}

#[test]
fn load_lenient_rejects_parentdir_trusted_link() {
    let json = trusted_links_json(&[
        wellformed_entry("../outside-link"),
        wellformed_entry("documents/../secrets"),
        wellformed_entry("documents/specs"),
    ]);
    let (_temp, paths) = temp_paths(&json);

    let report = BrainConfig::load_lenient(&paths).unwrap();
    let links: Vec<&str> = report
        .config
        .trusted_links
        .iter()
        .map(|e| e.link.as_str())
        .collect();
    assert_eq!(links, vec!["documents/specs"]);
    let rejections = report
        .diagnostics
        .iter()
        .filter(|d| d.contains("not vault-relative"))
        .count();
    assert_eq!(rejections, 2, "both ParentDir links must be rejected");
}

#[test]
fn load_lenient_rejects_empty_trusted_link() {
    // The empty string now FAILS `is_vault_relative_link`: issue #143
    // tightened the predicate to refuse empty and whitespace-only input at
    // both boundaries, so a hand-edited ledger entry with `link: ""` is
    // dropped with a diagnostic at load instead of being pinned through
    // (the previous behavior here asserted `""` was accepted; the flip is
    // the fix — see the predicate tests in tests/trusted_links.rs).
    let json = trusted_links_json(&[wellformed_entry("documents/specs"), wellformed_entry("")]);
    let (_temp, paths) = temp_paths(&json);

    let report = BrainConfig::load_lenient(&paths).unwrap();
    let links: Vec<&str> = report
        .config
        .trusted_links
        .iter()
        .map(|e| e.link.as_str())
        .collect();
    assert_eq!(
        links,
        vec!["documents/specs"],
        "the empty link must be dropped at the load boundary"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.contains("not vault-relative")),
        "expected a 'not vault-relative' diagnostic for the empty link, got: {:?}",
        report.diagnostics
    );
}

#[test]
fn load_lenient_accepts_wellformed_trusted_links() {
    let json = trusted_links_json(&[wellformed_entry("documents/specs")]);
    let (_temp, paths) = temp_paths(&json);

    let report = BrainConfig::load_lenient(&paths).unwrap();
    let links: Vec<&str> = report
        .config
        .trusted_links
        .iter()
        .map(|e| e.link.as_str())
        .collect();
    assert_eq!(links, vec!["documents/specs"]);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.contains("not vault-relative")),
        "well-formed links must not be rejected, got: {:?}",
        report.diagnostics
    );
}
