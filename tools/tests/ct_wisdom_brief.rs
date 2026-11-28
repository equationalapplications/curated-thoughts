//! `ct wisdom brief` contract tests (spec 2026-11-28-wisdom-brief). The brief
//! has no query and no embedding; fixtures reuse the scoped-brain helpers.

mod common;

use common::{
    brain_db, run_ct, seed_entity, seeded_fact_with_entity, with_scoped_brain,
    with_seeded_brain,
};
use std::collections::BTreeSet;

fn json_of(out: &std::process::Output) -> serde_json::Value {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("brief stdout is JSON")
}

fn keys(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn ids(v: &serde_json::Value) -> Vec<String> {
    v["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

fn brief(checkout: &std::path::Path) -> std::process::Output {
    run_ct(&["wisdom", "brief", "--json", "--repo-path", checkout.to_str().unwrap()])
}

#[test]
fn help_exits_zero() {
    let out = run_ct(&["wisdom", "brief", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("--repo-path"));
}

#[test]
fn max_chars_out_of_range_exits_1() {
    with_scoped_brain(None, |c| {
        for bad in ["0", "599", "8001"] {
            let out = run_ct(&[
                "wisdom", "brief", "--json", "--max-chars", bad, "--repo-path",
                c.to_str().unwrap(),
            ]);
            assert_eq!(out.status.code(), Some(1), "max-chars {bad}");
        }
    });
}

#[test]
fn max_chars_600_accepted_599_rejected() {
    with_scoped_brain(None, |c| {
        let p = c.to_str().unwrap();
        let ok = run_ct(&["wisdom", "brief", "--json", "--max-chars", "600", "--repo-path", p]);
        assert_eq!(ok.status.code(), Some(0));
        let bad = run_ct(&["wisdom", "brief", "--json", "--max-chars", "599", "--repo-path", p]);
        assert_eq!(bad.status.code(), Some(1));
        let top = run_ct(&["wisdom", "brief", "--json", "--max-chars", "8000", "--repo-path", p]);
        assert_eq!(top.status.code(), Some(0));
    });
}

#[test]
fn brief_json_shape_pinned() {
    with_scoped_brain(Some("org-1"), |c| {
        let v = json_of(&brief(c));
        let top: BTreeSet<String> = ["schema", "scope", "scope_reason", "budget", "statements", "index", "omitted"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(keys(&v), top);
        assert_eq!(v["schema"], 1);
        let st = &v["statements"][0];
        let got = keys(st);
        let want: BTreeSet<String> = ["id", "entity_id", "title", "text", "source_ref", "confidence", "source_type", "band"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(got, want);
        assert_eq!(keys(&v["budget"]), set(&["max_chars", "used_chars"]));
        assert_eq!(keys(&v["index"]), set(&["text", "areas", "counts"]));
        assert_eq!(keys(&v["omitted"]), set(&["statements", "oversize", "by_tier"]));
    });
}

#[test]
fn repo_and_org_statements_listed() {
    with_scoped_brain(Some("org-1"), |c| {
        seeded_fact_with_entity("fact_org", "org-1", None);
        let v = json_of(&brief(c));
        let mut got = ids(&v);
        got.sort();
        // fact_area is area-scoped (index only); fact_foreign is another hub.
        assert_eq!(got, vec!["fact_org", "fact_repo"]);
        assert_eq!(v["scope"], "repo+org");
        assert!(v["scope_reason"].is_null());
        assert_eq!(v["index"]["areas"][0]["name"], "auth");
    });
}

#[test]
fn no_prefix_returns_org_only_with_reason() {
    with_scoped_brain(Some("org-1"), |_c| {
        seeded_fact_with_entity("fact_org", "org-1", None);
        let elsewhere = tempfile::tempdir().unwrap();
        let v = json_of(&brief(elsewhere.path()));
        assert_eq!(ids(&v), vec!["fact_org"]);
        assert_eq!(v["scope"], "org");
        assert_eq!(v["scope_reason"], "no_prefix");
        assert_eq!(v["statements"][0]["band"], "org");
    });
}

#[test]
fn no_config_returns_empty_exit_0() {
    with_seeded_brain(|| {
        seed_entity("hub-1");
        seeded_fact_with_entity("f", "hub-1", None);
        let v = json_of(&run_ct(&["wisdom", "brief", "--json"]));
        assert!(v["statements"].as_array().unwrap().is_empty());
        assert_eq!(v["index"]["text"], "");
        assert_eq!(v["scope"], "none");
        assert_eq!(v["scope_reason"], "unconfigured");
    });
}

#[test]
fn unreadable_brain_exits_1() {
    let empty = tempfile::tempdir().unwrap();
    temp_env::with_vars(
        [("CURATED_BRAIN_DIR", Some(empty.path().to_str().unwrap()))],
        || {
            let out = run_ct(&["wisdom", "brief", "--json"]);
            assert_eq!(out.status.code(), Some(1));
            assert!(!out.stderr.is_empty());
        },
    );
}

#[test]
fn two_runs_byte_identical() {
    with_scoped_brain(Some("org-1"), |c| {
        seeded_fact_with_entity("fact_org", "org-1", None);
        let a = brief(c);
        let b = brief(c);
        assert_eq!(a.stdout, b.stdout);
        assert!(!a.stdout.is_empty());
    });
}

#[test]
fn relative_repo_path_resolves_against_cwd() {
    with_scoped_brain(None, |c| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_ct"))
            .args(["wisdom", "brief", "--json", "--repo-path", "."])
            .current_dir(c)
            .output()
            .unwrap();
        let v = json_of(&out);
        assert_eq!(ids(&v), vec!["fact_repo"]);
        assert_eq!(v["scope"], "repo");
    });
}

#[test]
fn output_never_contains_the_repo_path() {
    with_scoped_brain(Some("org-1"), |c| {
        let out = brief(c);
        let needle = c.to_str().unwrap();
        assert!(!String::from_utf8_lossy(&out.stdout).contains(needle));
        assert!(!String::from_utf8_lossy(&out.stderr).contains(needle));
        let human = run_ct(&["wisdom", "brief", "--repo-path", needle]);
        assert_eq!(human.status.code(), Some(0));
        assert!(!String::from_utf8_lossy(&human.stdout).contains(needle));
    });
}

#[test]
fn superseded_fact_absent_end_to_end() {
    with_scoped_brain(None, |c| {
        seeded_fact_with_entity("fact_old", "hub-1", None);
        brain_db()
            .execute(
                "UPDATE llm_wiki_entries SET superseded_by='fact_repo', superseded_at=1 WHERE id='fact_old'",
                [],
            )
            .unwrap();
        let v = json_of(&brief(c));
        assert_eq!(ids(&v), vec!["fact_repo"]);
    });
}

#[test]
fn stderr_diagnostic_has_no_path() {
    // Org configured but not a live entity: the diagnostic names the org id.
    with_scoped_brain(Some("org-dead"), |c| {
        let out = brief(c);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(stderr.contains("wisdom_brief_diagnostic\t"), "{stderr}");
        assert!(stderr.contains("org-dead"));
        assert!(!stderr.contains(c.to_str().unwrap()));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["scope_reason"], "org_unresolved");
    });
}

#[test]
fn load_lenient_diagnostics_never_contain_repo_path() {
    with_scoped_brain(Some("org-1"), |c| {
        let dir = std::env::var("CURATED_BRAIN_DIR").unwrap();
        std::fs::write(std::path::Path::new(&dir).join("config.json"), "{ not json").unwrap();
        let out = brief(c);
        let needle = c.to_str().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&out.stderr).contains("wisdom_brief_diagnostic\t"));
        assert!(!String::from_utf8_lossy(&out.stderr).contains(needle));
        assert!(!String::from_utf8_lossy(&out.stdout).contains(needle));
    });
}

// ---- delivery log (Task 8) ------------------------------------------------

fn log_rows() -> Vec<(String, String, Option<String>, String)> {
    let c = brain_db();
    let mut s = c
        .prepare("SELECT client, tool, entity_id, summary FROM curated_agent_log ORDER BY id")
        .unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn brief_writes_one_delivery_row_with_ids() {
    with_scoped_brain(Some("org-1"), |c| {
        let v = json_of(&brief(c));
        let rows = log_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "ct-cli");
        assert_eq!(rows[0].1, "wisdom_brief");
        assert_eq!(rows[0].2.as_deref(), Some("hub-1"));
        let s: serde_json::Value = serde_json::from_str(&rows[0].3).unwrap();
        assert_eq!(s["surface"], "brief");
        assert_eq!(s["ids"], serde_json::json!(ids(&v)));
        assert!(!rows[0].3.contains(c.to_str().unwrap()));
    });
}

#[test]
fn brief_empty_writes_no_row() {
    with_seeded_brain(|| {
        json_of(&run_ct(&["wisdom", "brief", "--json"]));
        assert!(log_rows().is_empty());
    });
}

#[test]
fn brief_output_identical_with_log_on_and_off() {
    with_scoped_brain(Some("org-1"), |c| {
        let p = c.to_str().unwrap();
        let args = ["wisdom", "brief", "--json", "--repo-path", p];
        let on = run_ct(&args);
        let off = common::run_ct_with_env(&args, [("CURATED_DELIVERY_LOG", "0")]);
        assert_eq!(on.stdout, off.stdout);
        assert_eq!(on.status.code(), off.status.code());
        assert_eq!(log_rows().len(), 1, "only the log-on call wrote a row");
    });
}

#[test]
fn brief_two_runs_identical_after_log_rows_exist() {
    with_scoped_brain(Some("org-1"), |c| {
        let a = brief(c);
        let b = brief(c);
        let d = brief(c);
        assert_eq!(a.stdout, b.stdout);
        assert_eq!(b.stdout, d.stdout);
        assert_eq!(log_rows().len(), 3);
    });
}

#[test]
fn brief_fail_open_when_log_table_dropped() {
    with_scoped_brain(Some("org-1"), |c| {
        let before = brief(c);
        brain_db().execute_batch("DROP TABLE curated_agent_log").unwrap();
        let after = brief(c);
        assert_eq!(after.status.code(), Some(0));
        assert_eq!(before.stdout, after.stdout);
        let err = String::from_utf8_lossy(&after.stderr);
        assert!(err.contains("wisdom_delivery_log_skipped\tno_table"), "{err}");
    });
}
