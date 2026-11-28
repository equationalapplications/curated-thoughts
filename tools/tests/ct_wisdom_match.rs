//! `ct wisdom match` contract tests (issue #265). Stub embeddings
//! (`CURATED_EMBED_STUB=constant8`) give every text the same direction, so
//! a fact seeded with blob [1,0,…] scores cosine 1 and clears the
//! `stub:constant8` floor (0.5); the gate logic itself is unit-tested in
//! src-tauri/src/wisdom_match.rs.

mod common;

use common::{
    run_ct, seed_entity, seed_fact, with_scoped_brain, with_seeded_brain,
};

fn brain_db() -> rusqlite::Connection {
    let dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    rusqlite::Connection::open(std::path::Path::new(&dir).join("brain.db")).unwrap()
}

fn json_of(out: &std::process::Output) -> serde_json::Value {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is JSON")
}

/// The gate-decision log line lives on STDERR so stdout stays pure JSON; the
/// tripwire cron greps the stable token and splits the tabs.
fn gate_decision_lines(out: &std::process::Output) -> Vec<Vec<String>> {
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr
        .lines()
        .filter(|l| l.starts_with("wisdom_gate_decision\t"))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// Exactly one decision line per `ct wisdom match` call, tab-separated field
/// order pinned by the spec's merge blocker 3 (F5):
/// `wisdom_gate_decision<TAB>ts<TAB>scheme<TAB>open|closed<TAB>n_results`.
/// The tripwire cron parses this — field order and tab count are contract.
#[test]
fn gate_decision_log_line_shape_on_a_hit() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let out = run_ct(&["wisdom", "match", "--json", "--", "how do I deploy"]);
        let v = json_of(&out);
        let lines = gate_decision_lines(&out);
        assert_eq!(
            lines.len(),
            1,
            "exactly one decision line per call; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let f = &lines[0];
        assert_eq!(f.len(), 5, "token + 4 tab-separated fields: {f:?}");
        assert_eq!(f[0], "wisdom_gate_decision");
        // ts: ISO-8601 UTC, `Z` suffix.
        let ts = &f[1];
        assert!(ts.ends_with('Z'), "ts must be UTC (Z suffix): {ts}");
        assert!(
            ts.parse::<chrono::DateTime<chrono::Utc>>().is_ok(),
            "ts must parse as ISO-8601 UTC: {ts}"
        );
        // scheme: the active read scheme string (seeded brain = `raw`).
        assert_eq!(f[2], "raw");
        // hit → open, n_results matches the JSON contract.
        assert_eq!(f[3], "open");
        assert_eq!(f[4], "1");
        assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    });
}

/// Miss DB: no entries → the same line shape but `closed` with `n_results=0`.
#[test]
fn gate_decision_log_line_closed_on_a_miss() {
    with_seeded_brain(|| {
        let out = run_ct(&["wisdom", "match", "--json", "--", "anything"]);
        let v = json_of(&out);
        assert_eq!(v["entries"], serde_json::json!([]));
        let lines = gate_decision_lines(&out);
        assert_eq!(lines.len(), 1);
        let f = &lines[0];
        assert_eq!(f.len(), 5, "token + 4 tab-separated fields: {f:?}");
        assert_eq!(f[0], "wisdom_gate_decision");
        assert_eq!(f[2], "raw");
        assert_eq!(f[3], "closed");
        assert_eq!(f[4], "0");
    });
}

#[test]
fn match_returns_contract_shape() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let v = json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--",
            "how do I deploy",
        ]));
        assert_eq!(v["schema"], 1);
        assert_eq!(v["gate"], "semantic-v1:stub:constant8");
        let e = &v["entries"][0];
        assert_eq!(e["id"], "fact_a");
        assert_eq!(e["title"], "Title fact_a");
        assert_eq!(e["text"], "Body fact_a");
        assert!(e["score"].as_f64().unwrap() > 0.99);
        assert_eq!(e["supersedes"], serde_json::json!([]));
        assert_eq!(e["provenance"], "librarian_inferred");
        assert_eq!(v["corrections"], serde_json::json!([]));
    });
}

#[test]
fn zero_matches_is_success() {
    with_seeded_brain(|| {
        let v = json_of(&run_ct(&["wisdom", "match", "--json", "--", "anything"]));
        assert_eq!(v["entries"], serde_json::json!([]));
    });
}

#[test]
fn exclude_and_correction() {
    with_seeded_brain(|| {
        seed_fact("fact_old");
        seed_fact("fact_new");
        brain_db()
            .execute(
                "UPDATE llm_wiki_entries SET superseded_by = 'fact_new', valid_to = 1
                 WHERE id = 'fact_old'",
                [],
            )
            .unwrap();
        let v = json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "0",
            "--exclude=fact_old",
            "--",
            "q",
        ]));
        assert_eq!(v["entries"], serde_json::json!([]));
        assert_eq!(v["corrections"][0]["id"], "fact_new");
        assert_eq!(
            v["corrections"][0]["supersedes"],
            serde_json::json!(["fact_old"])
        );
        assert!(v["corrections"][0]["score"].is_null());
        // excluding the head too: nothing to correct, nothing to match
        let v = json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--exclude=fact_old",
            "--exclude=fact_new",
            "--",
            "q",
        ]));
        assert_eq!(v["entries"], serde_json::json!([]));
        assert_eq!(v["corrections"], serde_json::json!([]));
    });
}

#[test]
fn text_requires_double_dash_and_may_start_with_dash() {
    with_seeded_brain(|| {
        let out = run_ct(&["wisdom", "match", "--json", "deploy"]);
        assert_eq!(out.status.code(), Some(1));
        json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--",
            "-weird --text",
        ]));
    });
}

#[test]
fn bad_or_too_many_excludes_exit_1() {
    with_seeded_brain(|| {
        let out = run_ct(&["wisdom", "match", "--json", "--exclude=bad id", "--", "q"]);
        assert_eq!(out.status.code(), Some(1));
        let many: Vec<String> = (0..1025).map(|i| format!("--exclude=f{i}")).collect();
        let mut args: Vec<&str> = vec!["wisdom", "match", "--json"];
        args.extend(many.iter().map(String::as_str));
        args.extend(["--", "q"]);
        assert_eq!(run_ct(&args).status.code(), Some(1));
    });
}

#[test]
fn whitespace_text_is_empty_success() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let v = json_of(&run_ct(&["wisdom", "match", "--json", "--", "   "]));
        assert_eq!(v["entries"], serde_json::json!([]));
    });
}

#[test]
fn help_exits_zero() {
    let out = run_ct(&["wisdom", "match", "--help"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn unresolvable_brain_exits_1() {
    let empty = tempfile::tempdir().unwrap();
    temp_env::with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(empty.path().to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            let out = run_ct(&["wisdom", "match", "--json", "--", "q"]);
            assert_eq!(out.status.code(), Some(1));
            assert!(!out.stderr.is_empty());
        },
    );
}

// ===================== Issue #282: activity query mode =====================
// NOTE (spec §Testing, rev-12 M1): the `ct` binary compiles the lib WITHOUT
// `cfg(test)`, so it can never see the stub activity composite row — every
// CLI activity call here is a §4 FALLBACK. The active-path behavior is
// pinned by the src-tauri `mod tests` (attribution, label, aperture,
// degradation); these tests pin the CLI surface: the clap group, the
// capability probe, the never-worse fallback parity, and the fallback's
// label/exit discipline.

fn target_of(out: &std::process::Output) -> String {
    // Task-5 ride-along (Task-4 verdict D6, test-only): the gate-decision log
    // line carries a wall-clock ts, so byte-comparing two separate `ct`
    // invocations flaked when they straddled a second boundary. Normalize the
    // ts field for the comparison only; field order and count stay
    // contract-pinned by `gate_decision_log_line_*`.
    let normalize = |s: String| {
        s.lines()
            .map(|l| match l.strip_prefix("wisdom_gate_decision\t") {
                Some(rest) => match rest.split_once('\t') {
                    Some((_ts, tail)) => format!("wisdom_gate_decision\t<TS>\t{tail}"),
                    None => l.to_string(),
                },
                None => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "{}{}",
        normalize(String::from_utf8_lossy(&out.stdout).into_owned()),
        normalize(String::from_utf8_lossy(&out.stderr).into_owned())
    )
}

/// rev-2 M-1 / rev-13: the `activity_target` group owns exactly-one — every
/// mixing combination is a clap usage error (non-zero exit).
#[test]
fn activity_target_group_rejects_every_mix() {
    with_seeded_brain(|| {
        for args in [
            vec![
                "wisdom",
                "match",
                "--json",
                "--path=a.rs",
                "--command=pnpm test",
            ],
            vec!["wisdom", "match", "--json", "--path=a.rs", "--pattern=*.ts"],
            vec![
                "wisdom",
                "match",
                "--json",
                "--command=pnpm test",
                "--pattern=*.ts",
            ],
            vec![
                "wisdom",
                "match",
                "--json",
                "--path=a.rs",
                "--",
                "a message",
            ],
            vec![
                "wisdom",
                "match",
                "--json",
                "--command=pnpm test",
                "--",
                "a message",
            ],
            vec![
                "wisdom",
                "match",
                "--json",
                "--pattern=*.ts",
                "--",
                "a message",
            ],
        ] {
            let out = run_ct(&args);
            assert_ne!(
                out.status.code(),
                Some(0),
                "mixed targets must be a usage error: {args:?}"
            );
        }
    });
}

/// rev-9 m4: the zero-target case (no target at all) is ALSO a clap
/// group-required error — the group satisfies the arg-required contract.
#[test]
fn zero_target_is_a_group_usage_error() {
    with_seeded_brain(|| {
        let out = run_ct(&["wisdom", "match", "--json"]);
        assert_ne!(out.status.code(), Some(0));
    });
}

/// Fable D1: `--help` advertises all three activity flags — the
/// integrations' capability probe.
#[test]
fn help_advertises_the_three_activity_flags() {
    let out = run_ct(&["wisdom", "match", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--path", "--command", "--pattern"] {
        assert!(help.contains(flag), "--help must advertise {flag}");
    }
}

/// §4 never-worse fallback (rev-3 M6, no-flag arm): `--path=X` is
/// byte-identical to `ct wisdom match -- X` (stdout AND stderr), exit 0.
#[test]
fn activity_no_flag_fallback_is_byte_identical_to_the_message_call() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let reference = run_ct(&["wisdom", "match", "--json", "--", "how do I deploy"]);
        let activity = run_ct(&["wisdom", "match", "--json", "--path=how do I deploy"]);
        assert_eq!(activity.status.code(), Some(0));
        assert_eq!(
            target_of(&activity),
            target_of(&reference),
            "no-flag fallback must be byte-identical to the message call"
        );
    });
}

/// §4 (rev-3 M6, --two-stage arm): `--two-stage --path=X` is byte-identical
/// to `ct wisdom match --two-stage -- X` — including the audit line the
/// message two-stage call prints (r29-m4). The target passes as ONE argv
/// element after `--` (rev-6 m4).
#[test]
fn activity_two_stage_fallback_is_byte_identical_to_the_message_two_stage_call() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let reference = run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--two-stage",
            "--",
            "how do I deploy",
        ]);
        let activity = run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--two-stage",
            "--path=how do I deploy",
        ]);
        assert_eq!(activity.status.code(), Some(0));
        assert_eq!(
            target_of(&activity),
            target_of(&reference),
            "--two-stage fallback must be byte-identical to the message two-stage call \
             (including the audit line)"
        );
    });
}

/// §4 (rev-6 M2 / rev-11 m3): an empty target AND `--max 0` delegate to the
/// message call with NO activity precheck — byte-identical, exit 0.
#[test]
fn empty_target_and_max_zero_delegate_to_the_message_call() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let empty_ref = run_ct(&["wisdom", "match", "--json", "--", ""]);
        let empty_act = run_ct(&["wisdom", "match", "--json", "--path="]);
        assert_eq!(empty_act.status.code(), Some(0));
        assert_eq!(target_of(&empty_act), target_of(&empty_ref));

        let max0_ref = run_ct(&["wisdom", "match", "--json", "--max", "0", "--", "deploy"]);
        let max0_act = run_ct(&["wisdom", "match", "--json", "--max", "0", "--path=deploy"]);
        assert_eq!(max0_act.status.code(), Some(0));
        assert_eq!(target_of(&max0_act), target_of(&max0_ref));
    });
}

/// rev-13 m2: the CLI can only ever see the FALLBACK, whose `gate` carries
/// NO `:activity` (the active-path label is a src-tauri lib assertion).
#[test]
fn fallback_gate_label_has_no_activity_suffix() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let out = run_ct(&["wisdom", "match", "--json", "--path=how do I deploy"]);
        let v = json_of(&out);
        let gate = v["gate"].as_str().expect("gate is a string");
        assert!(
            !gate.contains(":activity"),
            "fallback label must not claim the activity floor: {gate}"
        );
    });
}

// ── Active Verification Prompting from the CLI (issue #344 OQ1, plan Task 6)
// ──────────────────────────────────────────────────────────────────────────

/// A vault-shaped brain: `<vault>/.brain` as CURATED_BRAIN_DIR with a real
/// `vault_path` in config.json, real AppDb schema, stub embeddings — the
/// minimum for the drop file to land at `<vault>/.brain/unverified-match.jsonl`.
fn with_vault_brain<F: FnOnce(&std::path::Path)>(enabled: Option<bool>, f: F) {
    use tempfile::tempdir;
    use temp_env::with_vars;

    let vault = tempdir().unwrap();
    let brain = vault.path().join(".brain");
    std::fs::create_dir_all(&brain).unwrap();
    let confirmations = match enabled {
        Some(true) => r#","confirmations":{"enabled":true}"#,
        Some(false) => r#","confirmations":{"enabled":false}"#,
        None => "",
    };
    let config = format!(
        r#"{{"vault_path":"{}"{}}}"#,
        vault.path().display(),
        confirmations
    );
    std::fs::write(brain.join("config.json"), config).unwrap();
    let brain_str = brain.to_str().unwrap().to_string();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
            ("CURATED_BRAIN_DB", None::<&str>),
        ],
        move || {
            // Real DDL via the production migration path (common.rs pattern).
            let paths = tauri_app_lib::retrieval::resolve_brain_paths();
            let _db = tauri_app_lib::retrieval::AppDb::open_with_config(
                &paths.db_path,
                &paths.config_path,
            )
            .expect("writable brain db open");
            f(vault.path());
        },
    );
}

fn drop_file(vault: &std::path::Path) -> std::path::PathBuf {
    vault.join(".brain").join("unverified-match.jsonl")
}

/// An activity run that DELIVERS an agent-tier fact gains an entry in
/// `unverified-match.jsonl` when the confirmation surface is on (plan Task 6
/// Step 3). The fixture brain is uncalibrated, so the run takes the
/// never-worse fallback — the OQ1 ruling wires the prompt there too.
#[test]
fn activity_delivery_emits_unverified_match_when_enabled() {
    with_vault_brain(Some(true), |vault| {
        seed_fact("fact_a");
        let out = run_ct(&["wisdom", "match", "--json", "--path=how do I deploy"]);
        let v = json_of(&out);
        assert_eq!(
            v["entries"].as_array().unwrap().len(),
            1,
            "the fixture fact must deliver: {v}"
        );
        let text = std::fs::read_to_string(drop_file(vault))
            .expect("the drop file must be created");
        assert!(
            text.contains("fact_a"),
            "delivered agent-tier fact must be offered for confirmation: {text}"
        );
    });
}

/// The confirmations.enabled switch (Task 5) gates the CLI surface too:
/// flag off → the delivery still happens, but NOTHING is written to disk.
#[test]
fn activity_delivery_emits_nothing_when_disabled() {
    with_vault_brain(Some(false), |vault| {
        seed_fact("fact_a");
        let out = run_ct(&["wisdom", "match", "--json", "--path=how do I deploy"]);
        let v = json_of(&out);
        assert_eq!(v["entries"].as_array().unwrap().len(), 1);
        assert!(
            !drop_file(vault).exists(),
            "flag off must leave no drop file"
        );
    });
}

// ── SCOPED CLI wiring (spec 2026-11-14-areas §9.3 remainder, plan Task 5)
// ──────────────────────────────────────────────────────────────────────────
// Every assertion here runs against a SPAWNED `ct` binary: diagnostics are
// DATA in the lib; the CLI prints them on stderr, and stderr assertions need
// a real process (tier-3 r3 MAJOR-3).

/// Scoped-match stderr diagnostics use the `print_gate_decision` house form:
/// ONE tab-separated line per diagnostic, `wisdom_scoped_diagnostic` token,
/// reason as the single field (the pinned `wisdom_gate_decision` line's
/// token + four fields shape is tripwire-cron contract and must NOT grow).
fn scoped_diagnostic_lines(out: &std::process::Output) -> Vec<String> {
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr
        .lines()
        .filter(|l| l.starts_with("wisdom_scoped_diagnostic\t"))
        .map(|l| l.split('\t').skip(1).collect::<Vec<_>>().join("\t"))
        .collect()
}

fn entry_ids(v: &serde_json::Value) -> Vec<String> {
    v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect()
}

/// The §9.3 SCOPED hit: a `--path=` under the configured checkout matching a
/// glob returns ONLY the matching-area + whole-repo facts of the bound group
/// — the foreign repo's fact is band-excluded, the area-band item carries
/// `band:"area"` + the area name, the whole-repo item `band:"repo"`.
#[test]
fn scoped_query_returns_only_matching_area_and_whole_repo_facts() {
    with_scoped_brain(Some("org-1"), |checkout| {
        let p = checkout.join("src/auth/login.rs");
        let out = run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "10",
            &format!("--path={}", p.display()),
        ]);
        let v = json_of(&out);
        let mut ids = entry_ids(&v);
        ids.sort();
        assert_eq!(
            ids,
            vec!["fact_area".to_string(), "fact_repo".to_string()],
            "scoped band: {v}"
        );
        let entries = v["entries"].as_array().unwrap();
        let area_item = entries.iter().find(|e| e["id"] == "fact_area").unwrap();
        assert_eq!(area_item["band"], "area");
        assert_eq!(area_item["area"], "auth");
        let repo_item = entries.iter().find(|e| e["id"] == "fact_repo").unwrap();
        assert_eq!(repo_item["band"], "repo");
        // Healthy resolution: NO diagnostic lines on stderr.
        assert!(
            scoped_diagnostic_lines(&out).is_empty(),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
}

/// No `--path=` → UNSCOPED bytes: even with the wisdom block configured, the
/// message call returns the foreign repo's facts too (no band predicate).
#[test]
fn no_path_stays_byte_identical_unscoped_with_configured_block() {
    with_scoped_brain(Some("org-1"), |checkout| {
        let _ = checkout;
        let v = json_of(&run_ct(&["wisdom", "match", "--json", "--max", "10", "--", "how do I deploy"]));
        let mut ids = entry_ids(&v);
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "fact_area".to_string(),
                "fact_foreign".to_string(),
                "fact_repo".to_string()
            ],
            "no --path must never band-filter: {v}"
        );
    });
}

/// §9.8 UNSCOPED fence (the NEW pin): `--path=` OUTSIDE every configured
/// checkout → the query stays BYTE-IDENTICAL UNSCOPED — foreign facts
/// deliver, no scoped diagnostic, exit 0.
#[test]
fn path_outside_every_checkout_stays_unscoped() {
    with_scoped_brain(Some("org-1"), |_checkout| {
        let outside = std::env::temp_dir().join("ct-not-a-checkout/src/main.rs");
        let out = run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "10",
            &format!("--path={}", outside.display()),
        ]);
        let v = json_of(&out);
        let mut ids = entry_ids(&v);
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "fact_area".to_string(),
                "fact_foreign".to_string(),
                "fact_repo".to_string()
            ],
            "§9.8: no prefix → byte-identical UNSCOPED: {v}"
        );
        assert!(scoped_diagnostic_lines(&out).is_empty());
    });
}

/// §4.4 no-pattern case (tier-3 r2 n2): `--path=` INSIDE the configured
/// checkout matching NO glob → area `None`, the area arm is OMITTED — only
/// the whole-repo band matches (repo+org bands only). The auth-area fact
/// does NOT cross-match; the foreign repo stays excluded.
#[test]
fn path_with_no_matching_glob_omits_the_area_arm() {
    with_scoped_brain(Some("org-1"), |checkout| {
        let p = checkout.join("docs/readme.md");
        let v = json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "10",
            &format!("--path={}", p.display()),
        ]));
        assert_eq!(
            entry_ids(&v),
            vec!["fact_repo".to_string()],
            "area None → area facts excluded, whole-repo only: {v}"
        );
        assert_eq!(v["entries"][0]["band"], "repo");
        assert!(
            v["entries"][0].get("area").is_none(),
            "§10.2: area field absent when no area resolved"
        );
    });
}

/// Tier-3 r3 MAJOR-3 (Unresolved print path): a same-survivor collision
/// between two configured keys → the resolver's reason is PRINTED LOUDLY on
/// stderr in the house form, and the query proceeds BYTE-IDENTICAL UNSCOPED
/// (all facts deliver — §3.3 "loudly" is the printed reason, not a
/// different query).
#[test]
fn collision_unresolved_reason_is_printed_loudly_and_query_stays_unscoped() {
    with_scoped_brain(None, |checkout| {
        // §3.3 same-survivor collision: a SECOND key in a DIFFERENT
        // directory (two keys on one file would trip Task 2's load-time
        // duplicate-directory drop instead) whose entity is MERGED into
        // hub-1 — both configured keys chain-resolve to survivor hub-1.
        let brain_dir = std::env::var("CURATED_BRAIN_DIR").unwrap();
        seed_entity("hub-2");
        brain_db()
            .execute(
                "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES ('hub-2', 'hub-1', '100')",
                [],
            )
            .unwrap();
        // tempdir(): the collision fixture checkout drops with the closure
        // (m1 — no `ct-scoped-fixture-*` leakage under the system temp dir).
        let checkout2_tmp = tempfile::tempdir().unwrap();
        let checkout2 = checkout2_tmp.path();
        let af2 = checkout2.join(".ct-areas.yaml");
        std::fs::write(&af2, "areas:\n  deploy:\n    - \"infra/**\"\n").unwrap();
        let af = checkout.join(".ct-areas.yaml");
        // json!-built config (m2): no display()-splicing quoting hazards.
        let config = serde_json::json!({
            "wisdom": {
                "areas_paths": {
                    "hub-1": af.display().to_string(),
                    "hub-2": af2.display().to_string()
                }
            }
        })
        .to_string();
        std::fs::write(
            std::path::Path::new(&brain_dir).join("config.json"),
            config,
        )
        .unwrap();
        let p = checkout.join("src/auth/login.rs");
        let out = run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "10",
            &format!("--path={}", p.display()),
        ]);
        let v = json_of(&out);
        let lines = scoped_diagnostic_lines(&out);
        assert_eq!(
            lines.len(),
            1,
            "exactly one loud diagnostic; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            lines[0].contains("same-survivor collision") && lines[0].contains("hub-1") && lines[0].contains("hub-2"),
            "reason must name the collision: {lines:?}"
        );
        // Query bytes unchanged: the call proceeds UNSCOPED.
        let mut ids = entry_ids(&v);
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "fact_area".to_string(),
                "fact_foreign".to_string(),
                "fact_repo".to_string()
            ],
            "Unresolved → byte-identical UNSCOPED call: {v}"
        );
    });
}

/// Fix-round-1 MAJOR-1 (controller ruling): the load-side duplicate-directory
/// drop fails the WHOLE wisdom block closed at config-parse time — the
/// resolver never sees it, so the CLI must print the load report's
/// diagnostics ITSELF. TWO configured keys on the SAME areas file trip the
/// drop (config/mod.rs duplicate-directory guard); the spawned `ct` call
/// with `--path=` inside the checkout must print EXACTLY ONE diagnostic
/// line naming the drop, and the query must stay BYTE-IDENTICAL UNSCOPED
/// (all facts deliver).
#[test]
fn load_time_duplicate_directory_drop_is_printed_loudly_and_query_stays_unscoped() {
    with_scoped_brain(None, |checkout| {
        // Rewrite the brain's config.json with BOTH keys on the SAME areas
        // file (json!-built: no display()-splicing quoting hazards).
        let brain_dir = std::env::var("CURATED_BRAIN_DIR").unwrap();
        let af = checkout.join(".ct-areas.yaml");
        let config = serde_json::json!({
            "wisdom": {
                "areas_paths": {
                    "hub-1": af.display().to_string(),
                    "hub-2": af.display().to_string()
                }
            }
        })
        .to_string();
        std::fs::write(
            std::path::Path::new(&brain_dir).join("config.json"),
            config,
        )
        .unwrap();
        let p = checkout.join("src/auth/login.rs");
        let out = run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "10",
            &format!("--path={}", p.display()),
        ]);
        let v = json_of(&out);
        let lines = scoped_diagnostic_lines(&out);
        assert_eq!(
            lines.len(),
            1,
            "exactly ONE load-side diagnostic; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            lines[0].contains("duplicate areas-file directory"),
            "reason must name the duplicate-directory drop: {lines:?}"
        );
        // Query bytes unchanged: the call proceeds UNSCOPED.
        let mut ids = entry_ids(&v);
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "fact_area".to_string(),
                "fact_foreign".to_string(),
                "fact_repo".to_string()
            ],
            "fail-closed drop → byte-identical UNSCOPED call: {v}"
        );
    });
}

/// Org-omission diagnostic (tier-3 r3 MAJOR-3): `org_entity_id` configured
/// but dead → the org arm is omitted AND its diagnostic line is printed;
/// the SCOPED results still deliver.
#[test]
fn org_omission_diagnostic_is_printed_and_scoped_results_deliver() {
    with_scoped_brain(Some("org-dead"), |checkout| {
        let p = checkout.join("src/auth/login.rs");
        let out = run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "10",
            &format!("--path={}", p.display()),
        ]);
        let v = json_of(&out);
        let lines = scoped_diagnostic_lines(&out);
        assert_eq!(
            lines.len(),
            1,
            "org-omission must print one diagnostic; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            lines[0].contains("org hub org-dead unresolvable; org band omitted"),
            "{lines:?}"
        );
        let mut ids = entry_ids(&v);
        ids.sort();
        assert_eq!(ids, vec!["fact_area".to_string(), "fact_repo".to_string()]);
    });
}
