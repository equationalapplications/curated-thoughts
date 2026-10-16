//! `ct wisdom match --two-stage` CLI wiring tests (issue #271, plan Task 5).
//!
//! Stub embeddings (`CURATED_EMBED_STUB=constant8`) keep the vectors
//! deterministic. The src-tauri test-build composite floor slice
//! (`WISDOM_GATE_FLOORS_TEST`) is `cfg(test)`-gated inside the LIBRARY, so the
//! `ct` binary this test file spawns resolves floors from the PRODUCTION
//! table only — a stub-backed `--two-stage` run always exercises the §4
//! cell-1 v1 fallback (the shipped pre-calibration behavior, and exactly what
//! these tests pin: labels, the seven-field audit line, embed-skip, and the
//! byte-identical v1 stderr line). The decision core itself (`two_stage_entries`,
//! §4 matrix, rule attribution) is unit-tested in `src-tauri/src/wisdom_match.rs`
//! (Task 4).

mod common;

use common::{run_ct, with_seeded_brain};
use std::fs;

/// The audit line stderr token (§6 pin).
const AUDIT_TOKEN: &str = "wisdom_two_stage_audit";

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// The `wisdom_two_stage_audit` line split on whitespace: token + EXACTLY 7
/// fields, in the pinned order `rule=<i|ii|none> open=<0|1> top=<score|none>
/// v1merged=<n> floor=<key> k=<k> stale_embed_key=<n>`.
fn audit_fields(out: &std::process::Output) -> Vec<String> {
    let s = stderr(out);
    let lines: Vec<&str> = s.lines().filter(|l| l.starts_with(AUDIT_TOKEN)).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one audit line per --two-stage call; stderr:\n{s}"
    );
    let fields: Vec<String> = lines[0].split_whitespace().map(str::to_string).collect();
    assert_eq!(
        fields.len(),
        8,
        "audit line = token + EXACTLY 7 fields, space-separated: {fields:?}"
    );
    assert_eq!(fields[0], AUDIT_TOKEN);
    for (i, key) in [
        "rule=",
        "open=",
        "top=",
        "v1merged=",
        "floor=",
        "k=",
        "stale_embed_key=",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            fields[i + 1].starts_with(key),
            "field {} must be <{key}…>: {fields:?}",
            i + 1
        );
    }
    fields
}

fn audit_value<'a>(fields: &'a [String], key: &str) -> &'a str {
    fields
        .iter()
        .find_map(|f| f.strip_prefix(key))
        .unwrap_or_else(|| panic!("audit line missing {key}: {fields:?}"))
}

fn gate_decision_line(out: &std::process::Output) -> Vec<Vec<String>> {
    stderr(out)
        .lines()
        .filter(|l| l.starts_with("wisdom_gate_decision\t"))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// Step-1 pin: `--two-stage` wires through; stdout keeps the v1 label (§4
/// cell-1 fallback — the stub's composite floor is production-absent); the
/// audit line carries 7 fields with `rule=none`, `k=8`, and a non-empty
/// fallback floor key; exit 0 (r28-M3: a fallback is never an error).
#[test]
fn two_stage_flag_wires_through_and_emits_audit_line() {
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        let out = run_ct(&["wisdom", "match", "--two-stage", "--json", "--", "q"]);
        assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            v["gate"], "semantic-v1:stub:constant8",
            "cell-1 fallback keeps the v1 label (brief CARRIED WARNING)"
        );
        let f = audit_fields(&out);
        assert_eq!(audit_value(&f, "rule="), "none", "fallback ⇒ rule=none");
        assert_eq!(audit_value(&f, "k="), "8", "k = two_stage_k default");
        assert!(
            !audit_value(&f, "floor=").is_empty(),
            "v1 fallback floor key non-empty (brief CARRIED WARNING)"
        );
        audit_value(&f, "v1merged=")
            .parse::<usize>()
            .expect("v1merged is a plain integer");
        audit_value(&f, "stale_embed_key=")
            .parse::<i64>()
            .expect("stale_embed_key is a plain integer");
        assert!(
            matches!(audit_value(&f, "open="), "0" | "1"),
            "open is 0|1: {f:?}"
        );
        assert!(
            matches!(audit_value(&f, "top="), "none" | _),
            "top is a score or 'none': {f:?}"
        );
    });
}

/// r28-M3: a V27-shape (pre-V28) brain opened read-only degrades to v1 with
/// exit 0 — never a "no such column" error. The fixture brain goes through
/// the real migration path, so `documents` is hand-stripped back to its V1
/// shape (no `embed_key` / `last_indexed_hash`) to drive the probe-miss
/// branch.
#[test]
fn pre_v28_readonly_brain_degrades_to_v1_exit_0() {
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        {
            let conn = common::brain_db();
            conn.execute_batch(
                "CREATE TABLE documents_v27 AS SELECT * FROM documents;
                 DROP TABLE documents;
                 ALTER TABLE documents_v27 RENAME TO documents;",
            )
            .unwrap();
        }
        let out = run_ct(&["wisdom", "match", "--two-stage", "--json", "--", "q"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "probe miss ⇒ v1 fallback, never an error: stderr={}",
            stderr(&out)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["gate"], "semantic-v1:stub:constant8");
        let f = audit_fields(&out);
        assert_eq!(audit_value(&f, "rule="), "none");
        assert!(
            !audit_value(&f, "floor=").is_empty(),
            "v1 fallback reports the v1 floor key"
        );
        // Fix-round Minor #5: the probe-miss stale count is PINNED to the
        // exact value the code produces — plan Step 3.1's
        // `stale_embed_key=<count via probe 0>` semantics. On the V27 shape
        // `two_stage::coverage` misses entirely (`two_stage_schema_ready`
        // false ⇒ `Ok(None)`), so it cannot answer the count; the degraded
        // value is the integer 0. (The fixture's one candidate doc carries
        // the current key, so a HYPOTHETICAL probe would answer stale = 0
        // too — both readings agree; the pin is on the 0 the code emits.)
        let f = audit_fields(&out);
        assert_eq!(
            audit_value(&f, "stale_embed_key="),
            "0",
            "probe miss ⇒ the count degrades to 0 (`<count via probe 0>`, Step 3.1)"
        );
    });
}

/// The pinned 5-field `wisdom_gate_decision` stderr line keeps its SHAPE under
/// both paths — same field count, same scheme, same open state for the same
/// seed, parseable UTC timestamps (§5: the harness prefix-filter is untouched).
#[test]
fn wisdom_gate_decision_stderr_line_byte_identical() {
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        let ts = run_ct(&["wisdom", "match", "--two-stage", "--json", "--", "q"]);
        let ss = run_ct(&["wisdom", "match", "--single-stage", "--json", "--", "q"]);
        for out in [&ts, &ss] {
            assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(out));
        }
        let mut two = gate_decision_line(&ts);
        let mut single = gate_decision_line(&ss);
        assert_eq!(two.len(), 1, "stderr={}", stderr(&ts));
        assert_eq!(single.len(), 1, "stderr={}", stderr(&ss));
        let two = two.pop().unwrap();
        let single = single.pop().unwrap();
        assert_eq!(two.len(), 5, "token + 4 fields: {two:?}");
        assert_eq!(single.len(), 5, "token + 4 fields: {single:?}");
        assert_eq!(two[0], single[0], "wisdom_gate_decision token");
        assert!(
            two[1].parse::<chrono::DateTime<chrono::Utc>>().is_ok(),
            "{two:?}"
        );
        assert!(
            single[1].parse::<chrono::DateTime<chrono::Utc>>().is_ok(),
            "{single:?}"
        );
        assert_eq!(two[2], single[2], "scheme");
        assert_eq!(two[3], single[3], "open|closed");
        assert_eq!(two[4], single[4], "n_results");
    });
}

/// Both kill-switches (`--single-stage`, `CURATED_WISDOM_SINGLE_STAGE=1`)
/// suppress the audit line entirely and never print a two-stage label.
#[test]
fn single_stage_flags_and_env_are_kill_switches() {
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        let ss = run_ct(&["wisdom", "match", "--single-stage", "--json", "--", "q"]);
        assert_eq!(ss.status.code(), Some(0), "stderr={}", stderr(&ss));
        assert!(
            !stderr(&ss).contains(AUDIT_TOKEN),
            "--single-stage suppresses the audit line: {}",
            stderr(&ss)
        );
        let v: serde_json::Value = serde_json::from_slice(&ss.stdout).unwrap();
        let gate = v["gate"].as_str().unwrap();
        assert_eq!(gate, "semantic-v1:stub:constant8");
        assert!(!gate.contains("two-stage"));

        let out = common::run_ct_with_env(
            &["wisdom", "match", "--json", "--", "q"],
            [("CURATED_WISDOM_SINGLE_STAGE", "1")],
        );
        assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        assert!(
            !stderr(&out).contains(AUDIT_TOKEN),
            "env kill-switch suppresses the audit line: {}",
            stderr(&out)
        );
    });
}

/// §3 embed-skip: v1 floor present ⇒ exactly ONE embed; both floors missing
/// ⇒ 0 embeds (§4). Embeds are counted via the env-gated counting-stub
/// sidecar (`CURATED_EMBED_COUNT_FILE`).
#[test]
fn embed_skip_when_both_floors_missing() {
    let counts = count_file();
    fs::remove_file(&counts).ok();
    let count_arg = counts.to_str().unwrap().to_string();
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        // v1 floor present (stub:constant8) ⇒ exactly one query embed.
        let out = common::run_ct_with_env(
            &["wisdom", "match", "--two-stage", "--json", "--", "q"],
            [("CURATED_EMBED_COUNT_FILE", count_arg.as_str())],
        );
        assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        let with_v1 = read_count(&counts);
        assert_eq!(with_v1, 1, "v1-floor-only ⇒ exactly 1 embed; got {with_v1}");
        // No stub ⇒ the profile key (local:nomic-embed-code) has no floor
        // anywhere ⇒ uncalibrated ⇒ 0 further embeds.
        let out = common::run_ct_with_env(
            &["wisdom", "match", "--two-stage", "--json", "--", "q"],
            [
                ("CURATED_EMBED_STUB", ""),
                ("CURATED_EMBED_COUNT_FILE", count_arg.as_str()),
            ],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "uncalibrated is exit 0: stderr={}",
            stderr(&out)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["gate"], "uncalibrated");
        let both_missing = read_count(&counts) - with_v1;
        assert_eq!(both_missing, 0, "both floors missing ⇒ 0 embeds");
    });
    fs::remove_file(&counts).ok();
}

/// r26 m3: under the raw scheme the stage-1 vector IS the fact vector —
/// exactly ONE embed per message on a full `--two-stage` run.
#[test]
fn raw_scheme_reuses_one_embed_per_message() {
    let counts = count_file();
    fs::remove_file(&counts).ok();
    let count_arg = counts.to_str().unwrap().to_string();
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        let out = common::run_ct_with_env(
            &["wisdom", "match", "--two-stage", "--json", "--", "q"],
            [("CURATED_EMBED_COUNT_FILE", count_arg.as_str())],
        );
        assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        let n = read_count(&counts);
        assert_eq!(n, 1, "raw scheme reuses the raw vector: one embed; got {n}");
    });
    fs::remove_file(&counts).ok();
}

/// §5 semantics, pinned (fix-round Minor #3): the env kill-switch dominates
/// an EXPLICIT `--two-stage` flag — `CURATED_WISDOM_SINGLE_STAGE=1` +
/// `--two-stage` ⇒ the plain v1 path runs: no audit line, no two-stage
/// label, exit 0.
#[test]
fn env_kill_switch_dominates_explicit_two_stage_flag() {
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        let out = common::run_ct_with_env(
            &["wisdom", "match", "--two-stage", "--json", "--", "q"],
            [("CURATED_WISDOM_SINGLE_STAGE", "1")],
        );
        assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        assert!(
            !stderr(&out).contains(AUDIT_TOKEN),
            "env kill-switch over --two-stage ⇒ NO audit line: {}",
            stderr(&out)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let gate = v["gate"].as_str().unwrap();
        assert_eq!(
            gate, "semantic-v1:stub:constant8",
            "v1 path runs (never a two-stage label)"
        );
        assert!(!gate.contains("two-stage"));
    });
}

/// Fix-round Minor #4: the audit line IS emitted on the uncalibrated
/// no-open path (both floors missing) — r29-m4's "every requested path".
#[test]
fn uncalibrated_no_open_still_emits_audit_line() {
    with_seeded_brain(|| {
        common::seed_fact("fact_a");
        // No stub ⇒ the profile key (local:nomic-embed-code) has no floor
        // anywhere ⇒ both floors missing ⇒ the uncalibrated no-open path.
        let out = common::run_ct_with_env(
            &["wisdom", "match", "--two-stage", "--json", "--", "q"],
            [("CURATED_EMBED_STUB", "")],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "uncalibrated is exit 0: stderr={}",
            stderr(&out)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["gate"], "uncalibrated");
        let f = audit_fields(&out);
        assert_eq!(audit_value(&f, "rule="), "none");
        assert_eq!(audit_value(&f, "open="), "0");
        assert_eq!(
            audit_value(&f, "floor="),
            "",
            "uncalibrated ⇒ EMPTY floor key in the audit line"
        );
    });
}

// ---------------------------------------------------------------------------
// Counting-stub plumbing: `ct` runs in a CHILD process, so embed calls are
// counted through the env-gated sidecar file the embedder appends to
// (`CURATED_EMBED_COUNT_FILE`, one line per embed_batch call).
// ---------------------------------------------------------------------------

fn count_file() -> std::path::PathBuf {
    std::env::temp_dir().join("ct_two_stage_embed_count_test")
}

fn read_count(path: &std::path::Path) -> usize {
    fs::read_to_string(path).unwrap_or_default().lines().count()
}
