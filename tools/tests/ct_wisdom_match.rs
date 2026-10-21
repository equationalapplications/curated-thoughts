//! `ct wisdom match` contract tests (issue #265). Stub embeddings
//! (`CURATED_EMBED_STUB=constant8`) give every text the same direction, so
//! a fact seeded with blob [1,0,…] scores cosine 1 and clears the
//! `stub:constant8` floor (0.5); the gate logic itself is unit-tested in
//! src-tauri/src/wisdom_match.rs.

mod common;

use common::{run_ct, with_seeded_brain};
use rusqlite::params;

fn brain_db() -> rusqlite::Connection {
    let dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    rusqlite::Connection::open(std::path::Path::new(&dir).join("brain.db")).unwrap()
}

fn seed_fact(id: &str) {
    let mut v = [0f32; 8];
    v[0] = 1.0;
    let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    brain_db()
        .execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent_t', ?2, ?3, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?4, NULL)",
            params![id, format!("Title {id}"), format!("Body {id}"), blob],
        )
        .unwrap();
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
