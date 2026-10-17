//! Offline harness tests for `calibrate_two_stage` (issue #271 plan Task 14).
//!
//! The paired live calibration driver SHIPS here — it does NOT run (no probe
//! JSONL exists yet; the live run is §9 step 3, manual). These tests cover the
//! offline logic only, per the plan's Step-1 list:
//!
//! 1. letter verdict math — `two_stage_passes = ts_hit2 >= ss_hit2 && ts_fp <= ss_fp`,
//!    exact boundary: equal hit@2 + equal FP PASSES (§7, Kurt's pinned letter);
//! 2. artifact serialization round-trip;
//! 3. held-out split stratification is deterministic given a seed (r24 m7/E5);
//! 4. refusal when `CURATED_EMBED_STUB` set (`calibrate_wisdom_gate` rule);
//! 5. refusal when coverage < 100% — the two-stage arm must not run on a
//!    skewed set; the run aborts with instructions to run Task 13
//!    (`ct reindex verify-scratch`) then Task 12
//!    (`ct reindex repair-embed-key --stale`) (§9 step 3a, r25-M1);
//! 6. SQL literal pins — the driver's rebuilt eligibility predicate against
//!    the core fragment, pinned verbatim (task-14-fix I-1);
//! 7. OFFLINE driver-vs-core scoring parity over a fixture DB — one test
//!    tripping on eligibility, floor-drop, tier-weighting, sort, and
//!    cut-to-max drift simultaneously (task-14-fix I-1);
//! 8. grid picker — pinned tie-break on synthetic grids (higher hit@2 →
//!    lower FP → rule (i), GLM r1 minor-5) + campaign-homogeneity and
//!    letter-consistency refusals + the patch-snippet report (plan §Task 15);
//! 9. flip — refusals on a missing/stale/unversioned artifact, an unknown
//!    schema, a failing or inconsistent letter (printing the letter
//!    numbers), and the flip package on a passing letter (plan §Task 15);
//! 10. after a SIMULATED flip, every non-stub floor key (the REAL
//!     `WISDOM_GATE_FLOORS` rows + the composite key) has a freeze dir —
//!     the mirror of the bench's `every_non_stub_floor_key_has_a_freeze_dir`
//!     (plan §Task 15 Step 1, test 3).
//!
//! The driver's pure logic lives in the bin source, included here as a module
//! (integration tests cannot import a bin target directly).

// `fn main` and the live-only helpers are dead in a test build.
#[allow(dead_code)]
#[path = "../src/bin/calibrate_two_stage.rs"]
mod calibrate_two_stage;

use calibrate_two_stage::{
    arm_metrics, check_coverage, check_embed_stub, composite_floor_key, eligibility_where,
    flip_plan, freeze_relative_dir, grid_cells, letter_verdict, load_artifact, patch_snippet,
    pick_report, pick_winner, score_entries, stratified_splits, temporal_shape, write_flip_package,
    ArmMetrics, BaselineArm, CalibrationArtifact, CandidateCell, Coverage, EmbedStats, GridInfo,
    HeldOutInfo, HeldOutSplit, InjectionStats, MergeAttribution, ProbeSetInfo, ProbeVerdict,
    TemporalShape, TopKAblation, V1MergeStats,
};
use rusqlite::Connection;
use tauri_app_lib::embed_scheme::Scheme;
use tauri_app_lib::wiki_graph::f32_vec_to_blob;
use tauri_app_lib::wisdom_match::wisdom_match_with_floor;

// ── 1. Letter verdict math (§7, exact boundary) ─────────────────────────────

#[test]
fn letter_verdict_passes_on_exact_tie() {
    // Equal hit@2 + equal FP PASSES — the boundary case the plan names.
    assert!(letter_verdict(0.500, 3, 0.500, 3));
    assert!(letter_verdict(0.0, 0, 0.0, 0));
}

#[test]
fn letter_verdict_passes_only_when_not_worse_on_both_axes() {
    // Strictly better on both axes passes.
    assert!(letter_verdict(0.525, 2, 0.500, 3));
    // Better hit@2 but more FP fails.
    assert!(!letter_verdict(0.550, 4, 0.500, 3));
    // Equal hit@2 but more FP fails.
    assert!(!letter_verdict(0.500, 4, 0.500, 3));
    // Fewer FP but lower hit@2 fails.
    assert!(!letter_verdict(0.475, 3, 0.500, 3));
    // Equal FP but lower hit@2 fails.
    assert!(!letter_verdict(0.475, 3, 0.500, 3));
}

// ── 2. Artifact serialization round-trip ────────────────────────────────────

/// M-3 fixture: a `#[cfg(test)]`-only provenance value — the test crate
/// cannot inject a compile-time `CT_BUILD_SHA`; the LIVE binary reads
/// `option_env!("CT_BUILD_SHA")` (fallback "unversioned") at the
/// artifact-construction site (`code_sha()`).
const FIXTURE_CODE_SHA: &str = "test-code-sha-fixture";

/// The Task-14 fixture artifact, asserted on verbatim by the round-trip test
/// and cloned with cell/metric overrides by the Task-15 picker/flip tests
/// (`grid_cell`).
fn fixture_artifact() -> CalibrationArtifact {
    CalibrationArtifact {
        schema: 1,
        code_sha: FIXTURE_CODE_SHA.to_string(),
        generated_utc: "2026-10-17T12:00:00Z".to_string(),
        probes: ProbeSetInfo {
            path: "probes.jsonl".to_string(),
            sha256: "deadbeef".to_string(),
            count: 150,
            relevant: 40,
            fp: 110,
        },
        baseline: BaselineArm {
            key: "external:qwen/qwen3-embedding-4b:instr1".to_string(),
            floor: 0.64,
            scheme: "instr1".to_string(),
        },
        candidate: CandidateCell {
            rule: "chunk-hop".to_string(),
            floor: 0.5,
            k: 8,
        },
        grid: GridInfo {
            cells: 1,
            rule: "chunk-hop".to_string(),
            floor: 0.5,
            k: 8,
        },
        coverage: Coverage {
            candidate_docs: 47,
            covered: 47,
            fraction: 1.0,
        },
        single_stage: ArmMetrics {
            hit2: 0.5,
            fp: 2,
            opens: 30,
            entries_per_open: 1.5,
        },
        two_stage: ArmMetrics {
            hit2: 0.5,
            fp: 2,
            opens: 28,
            entries_per_open: 2.0,
        },
        two_stage_passes: letter_verdict(0.5, 2, 0.5, 2),
        letter: "two_stage_passes = ts_hit2 >= ss_hit2 && ts_fp <= ss_fp".to_string(),
        heldout: HeldOutInfo {
            splits: 5,
            half_size: 75,
            seed: 271_828,
            per_split: vec![HeldOutSplit {
                relevant: 20,
                fp: 55,
                single_stage_hit2: 0.55,
                two_stage_hit2: 0.5,
            }],
        },
        embeds: EmbedStats {
            per_message: 2,
            scheme: "instr1".to_string(),
            latency_ms_p50: 120.0,
            latency_ms_p95: 340.0,
        },
        injection: InjectionStats {
            rule: "chunk-hop".to_string(),
            opens: 28,
            entries_per_open_median: 3.0,
        },
        merge_attribution: MergeAttribution {
            opens_via_merge_only: 1,
            hits_via_merge_only: 0,
            v1merged_total: 3,
        },
        v1_merge: V1MergeStats {
            latency_ms_p95: 12.0,
            volume_total: 3,
            measured: "merge-segment".to_string(),
        },
        ablation: Some(TopKAblation {
            k: 8,
            top1: ArmMetrics {
                hit2: 0.5,
                fp: 2,
                opens: 28,
                entries_per_open: 2.0,
            },
            topk: ArmMetrics {
                hit2: 0.525,
                fp: 3,
                opens: 31,
                entries_per_open: 2.25,
            },
        }),
        stale_embed_key_total: 0,
    }
}

#[test]
fn artifact_round_trips_through_json() {
    let artifact = fixture_artifact();
    let json = serde_json::to_string(&artifact).expect("serialize artifact");
    let back: CalibrationArtifact = serde_json::from_str(&json).expect("deserialize artifact");
    assert_eq!(back, artifact);
    // The pinned letter verdict is a literal JSON boolean in the artifact.
    assert!(json.contains("\"two_stage_passes\":true"));
    // M-3: code provenance round-trips and is carried in the JSON.
    assert_eq!(back.code_sha, FIXTURE_CODE_SHA);
    assert!(
        json.contains(&format!("\"code_sha\":\"{FIXTURE_CODE_SHA}\"")),
        "artifact JSON must carry code provenance (task-14-fix M-3)"
    );
}

// ── 3. Held-out split stratification, deterministic given a seed ────────────

fn fixture_labels() -> Vec<bool> {
    // The 150-probe real-traffic set: 40 relevant + 110 FP probes (§7/r24 m7).
    let mut labels = vec![true; 40];
    labels.extend(vec![false; 110]);
    labels
}

#[test]
fn stratified_splits_are_deterministic_and_balanced() {
    let labels = fixture_labels();
    let a = stratified_splits(&labels, 5, 271_828);
    let b = stratified_splits(&labels, 5, 271_828);
    assert_eq!(a.len(), 5);
    // Same seed ⇒ byte-identical splits.
    for (sa, sb) in a.iter().zip(b.iter()) {
        assert_eq!(sa.held_out, sb.held_out);
        assert_eq!(sa.rest, sb.rest);
    }
    for split in &a {
        // 75/75 halves.
        assert_eq!(split.held_out.len(), 75);
        assert_eq!(split.rest.len(), 75);
        // Stratified: each half carries exactly 20 relevant + 55 FP.
        let held_rel = split.held_out.iter().filter(|&&i| labels[i]).count();
        let held_fp = split.held_out.iter().filter(|&&i| !labels[i]).count();
        assert_eq!(held_rel, 20, "held-out half must keep the 40/110 ratio");
        assert_eq!(held_fp, 55);
        let rest_rel = split.rest.iter().filter(|&&i| labels[i]).count();
        let rest_fp = split.rest.iter().filter(|&&i| !labels[i]).count();
        assert_eq!(rest_rel, 20);
        assert_eq!(rest_fp, 55);
        // Halves are a partition (no overlap, joint coverage).
        let mut seen = split.held_out.clone();
        seen.extend(&split.rest);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 150);
    }
}

#[test]
fn stratified_splits_vary_with_the_seed() {
    let labels = fixture_labels();
    let a = stratified_splits(&labels, 5, 1);
    let b = stratified_splits(&labels, 5, 2);
    assert!(
        a[0].held_out != b[0].held_out,
        "different seeds must draw different splits (informational only, r24 E5)"
    );
}

// ── Arm metrics from probe labels + open/entries facts (r25 m1) ─────────────

#[test]
fn arm_metrics_derive_hit2_and_fp_from_labels_and_opens() {
    // labels: [relevant, relevant, fp, fp]
    let relevant = vec![true, true, false, false];
    let verdicts = vec![
        ProbeVerdict {
            hit: true,
            open: true,
            entries: 2,
        },
        ProbeVerdict {
            hit: false,
            open: true,
            entries: 1,
        },
        ProbeVerdict {
            hit: false,
            open: true,
            entries: 1,
        }, // FP: open on an FP probe
        ProbeVerdict {
            hit: false,
            open: false,
            entries: 0,
        },
    ];
    let m = arm_metrics(&verdicts, &relevant);
    assert!((m.hit2 - 0.5).abs() < 1e-9, "1 hit over 2 relevant probes");
    assert_eq!(m.fp, 1);
    assert_eq!(m.opens, 3);
    assert!((m.entries_per_open - 4.0 / 3.0).abs() < 1e-9);
}

#[test]
fn arm_metrics_handle_zero_relevant_and_zero_opens() {
    let m = arm_metrics(&[], &[]);
    assert_eq!(m.hit2, 0.0);
    assert_eq!(m.fp, 0);
    assert_eq!(m.opens, 0);
    assert_eq!(m.entries_per_open, 0.0);
}

// ── 4. Refusal when CURATED_EMBED_STUB is set ────────────────────────────────

#[test]
fn refuses_embed_stub() {
    use std::ffi::OsStr;
    check_embed_stub(None).expect("no stub set ⇒ allowed");
    let err = check_embed_stub(Some(OsStr::new("constant8")))
        .expect_err("stub set ⇒ refuse, real embeddings only");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("CURATED_EMBED_STUB"),
        "refusal must name the env var, got: {msg}"
    );
}

// ── 5. Refusal when coverage < 100% (skewed set ⇒ abort) ────────────────────

#[test]
fn refuses_coverage_below_100_percent_with_repair_instructions() {
    // 46/47 covered — the skewed-set abort.
    let err = check_coverage(Some((47, 46))).expect_err("coverage < 100% must abort");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("ct reindex verify-scratch"),
        "abort must instruct running Task 13 (verify-scratch) first, got: {msg}"
    );
    assert!(
        msg.contains("ct reindex repair-embed-key"),
        "abort must instruct Task 12 (repair-embed-key --stale) when still short, got: {msg}"
    );

    // Probe miss (pre-V28 brain) cannot record 100% ⇒ refuse too.
    let err = check_coverage(None).expect_err("unavailable coverage must abort");
    assert!(format!("{err:#}").contains("ct reindex verify-scratch"));
}

#[test]
fn accepts_full_coverage_and_records_the_fraction() {
    let cov = check_coverage(Some((47, 47))).expect("100% coverage ⇒ run");
    assert_eq!(cov.candidate_docs, 47);
    assert_eq!(cov.covered, 47);
    assert!((cov.fraction - 1.0).abs() < 1e-9);
}

// ── 6. SQL literal pins (task-14-fix I-1) ────────────────────────────────────

/// The core's private `wisdom_match::entry_eligibility_where` output for the
/// same (shape, scheme) inputs, pinned VERBATIM — it was verified byte-equal
/// to the driver's rebuild at task-14 review, and the fn is `pub(crate)`
/// (not importable from the tools crate), so the pin is a literal.
fn core_fragment_literal(has_superseded_by: bool, has_valid_to: bool, scheme: Scheme) -> String {
    let mut sql = format!(
        "e.deleted_at IS NULL \
         AND e.title IS NOT NULL AND e.body IS NOT NULL \
         AND e.embedding_blob IS NOT NULL AND embed_scheme = '{}'",
        scheme.as_str()
    );
    if has_superseded_by {
        sql.push_str(" AND (e.superseded_by IS NULL OR e.superseded_at > :now)");
    }
    if has_valid_to {
        sql.push_str(" AND (e.valid_to IS NULL OR e.valid_to > :now)");
    }
    sql
}

#[test]
fn eligibility_where_pins_the_core_fragment_by_literal() {
    // Every (temporal shape × scheme) combination the core can produce.
    for scheme in [Scheme::Raw, Scheme::Instr1] {
        for sup in [false, true] {
            for val in [false, true] {
                let shape = TemporalShape {
                    has_superseded_by: sup,
                    has_valid_to: val,
                };
                assert_eq!(
                    eligibility_where(&shape, scheme),
                    core_fragment_literal(sup, val, scheme),
                    "driver eligibility_where drifted from the core fragment \
                     (sup={sup} valid_to={val}, scheme {})",
                    scheme.as_str()
                );
            }
        }
    }
    // The live production shape, spelled out for audit: one generated string
    // pinned character-for-character (incl. the scheme filter interpolation).
    let live = TemporalShape {
        has_superseded_by: true,
        has_valid_to: true,
    };
    assert_eq!(
        eligibility_where(&live, Scheme::Raw),
        "e.deleted_at IS NULL \
         AND e.title IS NOT NULL AND e.body IS NOT NULL \
         AND e.embedding_blob IS NOT NULL AND embed_scheme = 'raw' \
         AND (e.superseded_by IS NULL OR e.superseded_at > :now) \
         AND (e.valid_to IS NULL OR e.valid_to > :now)"
    );
}

// ── 7. Driver-vs-core scoring parity, OFFLINE (task-14-fix I-1) ──────────────

const NOW_MS: i64 = 1_700_000_000_000;
const FLOOR: f32 = 0.40;

fn parity_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory fixture db");
    conn.execute_batch(
        "CREATE TABLE llm_wiki_entries (
             id TEXT PRIMARY KEY,
             entity_id TEXT NOT NULL,
             title TEXT,
             body TEXT,
             source_type TEXT,
             embedding_blob BLOB,
             deleted_at INTEGER,
             superseded_by TEXT,
             superseded_at INTEGER,
             valid_to INTEGER,
             embed_scheme TEXT
         );",
    )
    .expect("fixture schema");
    conn
}

#[allow(clippy::too_many_arguments)]
fn seed_entry(
    conn: &Connection,
    id: &str,
    entity_id: &str,
    vec: &[f32],
    deleted_at: Option<i64>,
    superseded_by: Option<&str>,
    superseded_at: Option<i64>,
    valid_to: Option<i64>,
) {
    conn.execute(
        "INSERT INTO llm_wiki_entries
             (id, entity_id, title, body, source_type, embedding_blob,
              deleted_at, superseded_by, superseded_at, valid_to, embed_scheme)
         VALUES (?1, ?2, 'T', 'B', 'note', ?3, ?4, ?5, ?6, ?7, 'raw')",
        rusqlite::params![
            id,
            entity_id,
            f32_vec_to_blob(vec),
            deleted_at,
            superseded_by,
            superseded_at,
            valid_to
        ],
    )
    .expect("seed fixture row");
}

/// Fixture rows + query vector. Query is `[1, 0, 0, 0]` so every seeded
/// entry's RAW cosine equals its first component. All seeded ids are handed
/// to BOTH sides — eligibility, not the id list, must filter the ineligible
/// rows. OFFLINE: fixture DB only, no live brain, no embeds.
fn parity_fixture() -> (Connection, Vec<f32>, Vec<String>) {
    let conn = parity_conn();
    let qv = vec![1.0, 0.0, 0.0, 0.0];
    // Eligible, above floor. Query direction is [1,0,0,0], so an entry's
    // RAW cosine is the first component of its UNIT vector. Weighted order
    // DIFFERS from raw order: c-working has raw 0.70 > d-plain 0.65, but
    // its `tier_working::` weight 0.6 drops it to 0.42 — below d-plain.
    // The order assertion below trips if either side stops tier-weighting.
    seed_entry(
        &conn,
        "a-tier-fact",
        "tier_fact",
        &[0.5, 0.5, 0.0, 0.0],
        None,
        None,
        None,
        None,
    ); // raw 0.7071 × 1.5 = 1.0607
    seed_entry(
        &conn,
        "b-wisdom",
        "tier_wisdom",
        &[0.8, 0.6, 0.0, 0.0],
        None,
        None,
        None,
        None,
    ); // raw 0.80
    seed_entry(
        &conn,
        "c-working",
        "tier_working::w1",
        &[0.7, 0.7141428, 0.0, 0.0],
        None,
        None,
        None,
        None,
    ); // raw 0.70 × 0.6 = 0.42
    seed_entry(
        &conn,
        "d-plain",
        "plain",
        &[0.65, 0.7599342, 0.0, 0.0],
        None,
        None,
        None,
        None,
    ); // raw 0.65
       // Supersession IN THE FUTURE — still eligible (`superseded_at > :now`).
    seed_entry(
        &conn,
        "i-superseded-future",
        "plain",
        &[0.55, 0.8351647, 0.0, 0.0],
        None,
        Some("x"),
        Some(NOW_MS + 1_000),
        None,
    ); // raw 0.55
       // Raw below floor (plain weight) — dropped by the `raw < floor` arm.
    seed_entry(
        &conn,
        "e-below-floor",
        "plain",
        &[0.3, 0.9539392, 0.0, 0.0],
        None,
        None,
        None,
        None,
    ); // raw 0.30 < 0.40
       // Weighted ABOVE floor but raw BELOW — dropped iff the floor compares
       // RAW cosine (the pinned mechanic on both sides); 0.35 × 1.5 = 0.525.
    seed_entry(
        &conn,
        "f-weighted-above-floor",
        "tier_fact",
        &[0.35, 0.9367497, 0.0, 0.0],
        None,
        None,
        None,
        None,
    ); // raw 0.35 < 0.40
       // Ineligible — the three temporal arms: deleted / superseded-now /
       // expired. Highest raw cosines in the fixture: a drift in eligibility
       // puts them at the TOP of one side's output.
    seed_entry(
        &conn,
        "g-deleted",
        "plain",
        &[0.99, 0.1410674, 0.0, 0.0],
        Some(1),
        None,
        None,
        None,
    ); // raw 0.99
    seed_entry(
        &conn,
        "h-superseded-now",
        "plain",
        &[0.98, 0.1989975, 0.0, 0.0],
        None,
        Some("x"),
        Some(NOW_MS),
        None,
    ); // raw 0.98
    seed_entry(
        &conn,
        "j-expired",
        "plain",
        &[0.97, 0.2431049, 0.0, 0.0],
        None,
        None,
        None,
        Some(NOW_MS),
    ); // raw 0.97
       // Dimension skew — skipped by both sides, never fails the call.
    seed_entry(
        &conn,
        "k-dim-skew",
        "plain",
        &[1.0, 0.0],
        None,
        None,
        None,
        None,
    );
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM llm_wiki_entries")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    (conn, qv, ids)
}

/// ONE test tripping on EVERY drift axis at once (task-14-fix I-1):
/// eligibility (g/h/j), the RAW floor drop (e, and f iff the floor moves to
/// the weighted score), tier weighting (c vs d), weighted-cosine sort, and
/// cut-to-max — driver `score_entries` vs the core's pub entry point
/// `wisdom_match_with_floor` (floor `Some` ⇒ `gated_entries` with an empty
/// skip set: ids + scores + order).
#[test]
fn score_entries_parity_with_core_gated_entries() {
    let (conn, qv, ids) = parity_fixture();
    let shape = temporal_shape(&conn).expect("fixture has all temporal columns");
    assert!(
        shape.has_superseded_by && shape.has_valid_to,
        "fixture must exercise both temporal arms"
    );

    let driver = score_entries(
        &conn,
        &qv,
        Some(FLOOR),
        10,
        &ids,
        &shape,
        Scheme::Raw,
        NOW_MS,
    )
    .expect("driver score_entries");
    let core = wisdom_match_with_floor(
        &conn,
        &qv,
        "parity-gate-key",
        Scheme::Raw,
        Some(FLOOR),
        10,
        &[],
        NOW_MS,
    )
    .expect("core wisdom_match_with_floor");
    // exclude is empty and no fixture row is superseded ⇒ no corrections.
    assert!(core.corrections.is_empty());

    // Expected order: weighted-cosine sort (a 1.0607, b 0.8, d 0.65,
    // i 0.55, c 0.42) — NOT the raw order (b 0.8, a 0.7071, c 0.70,
    // d 0.65, i 0.55): weighting drift shows up as an a/c-d swap.
    let expected = [
        "a-tier-fact",
        "b-wisdom",
        "d-plain",
        "i-superseded-future",
        "c-working",
    ];
    let driver_ids: Vec<&str> = driver.iter().map(|(_, e)| e.id.as_str()).collect();
    let core_ids: Vec<&str> = core.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        core_ids, expected,
        "core order (tier-weighted, floor-dropped)"
    );
    assert_eq!(driver_ids, expected, "driver order must equal the core's");

    for ((weighted, d), c) in driver.iter().zip(core.entries.iter()) {
        assert_eq!(d.id, c.id);
        let dr = d.score.expect("driver entry score");
        let cr = c.score.expect("core entry score");
        assert!((dr - cr).abs() < 1e-6, "raw cosine drift on {}", d.id);
        assert!(dr >= FLOOR, "raw below floor leaked on {}", d.id);
        assert!(
            (weighted - dr * tier_weight_of(&d.id)).abs() < 1e-6,
            "tier weighting drift on {}",
            d.id
        );
    }
    // The top row's weight ratio pins the tier_fact 1.5 amplification.
    let top = &driver[0];
    assert!(
        (top.0 / top.1.score.unwrap() - 1.5).abs() < 1e-6,
        "a-tier-fact must carry the 1.5 tier_fact weight"
    );

    // Cut-to-max: same inputs, max 2 — both sides truncate identically.
    let driver_top2 = score_entries(
        &conn,
        &qv,
        Some(FLOOR),
        2,
        &ids,
        &shape,
        Scheme::Raw,
        NOW_MS,
    )
    .expect("driver score_entries (max 2)");
    let core_top2 = wisdom_match_with_floor(
        &conn,
        &qv,
        "parity-gate-key",
        Scheme::Raw,
        Some(FLOOR),
        2,
        &[],
        NOW_MS,
    )
    .expect("core wisdom_match_with_floor (max 2)");
    let d2: Vec<&str> = driver_top2.iter().map(|(_, e)| e.id.as_str()).collect();
    let c2: Vec<&str> = core_top2.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        driver_top2.len(),
        2,
        "cut-to-max must truncate the driver side"
    );
    assert_eq!(c2, d2, "cut-to-max drift");
    assert_eq!(d2, ["a-tier-fact", "b-wisdom"]);
}

/// Fixture entity tiers by id (mirrors `tauri_app_lib::wiki_graph::tier_weight`:
/// tier_fact 1.5, tier_wisdom 1.0, `tier_working::` 0.6, else 1.0).
fn tier_weight_of(id: &str) -> f32 {
    match id {
        "a-tier-fact" | "f-weighted-above-floor" => 1.5,
        "c-working" => 0.6,
        _ => 1.0,
    }
}

// ── 8. Grid picker: pinned tie-break on synthetic grids (plan §Task 15) ──────

/// One synthetic grid cell: the fixture artifact with the candidate cell +
/// two-stage arm metrics overridden. `passes` MUST equal the letter verdict
/// over the overridden numbers for the campaign-consistency checks to hold
/// (the picker refuses inconsistent artifacts by design).
fn grid_cell(
    rule: &str,
    floor: f32,
    k: usize,
    ts_hit2: f64,
    ts_fp: usize,
    passes: bool,
) -> CalibrationArtifact {
    let mut a = fixture_artifact();
    a.candidate = CandidateCell {
        rule: rule.to_string(),
        floor,
        k,
    };
    a.grid = GridInfo {
        cells: 1,
        rule: rule.to_string(),
        floor,
        k,
    };
    a.two_stage.hit2 = ts_hit2;
    a.two_stage.fp = ts_fp;
    a.two_stage_passes = passes;
    a
}

#[test]
fn composite_floor_key_matches_spec_shape() {
    // Spec §3: `<floor_key_for(model, scheme)>:two-stage:<rule>`. The
    // artifact's `baseline.key` IS the v1 schemed floor key (Opus r1 M3).
    let a = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    assert_eq!(
        composite_floor_key(&a),
        "external:qwen/qwen3-embedding-4b:instr1:two-stage:chunk-hop"
    );
}

#[test]
fn picker_prefers_higher_hit2_among_passing_cells() {
    let lo = grid_cell("restricted-cosine", 0.5, 8, 0.500, 2, true);
    let hi = grid_cell("chunk-hop", 0.5, 8, 0.525, 2, true);
    let w = pick_winner(&[lo, hi]).expect("a passing grid has a winner");
    assert_eq!(w.candidate.rule, "chunk-hop");
    assert!((w.two_stage.hit2 - 0.525).abs() < 1e-9);
}

#[test]
fn picker_breaks_hit2_ties_by_lower_fp() {
    // Both cells pass (fp ≤ ss_fp = 2), equal hit@2 ⇒ lower FP wins.
    let more_fp = grid_cell("restricted-cosine", 0.5, 8, 0.525, 2, true);
    let less_fp = grid_cell("chunk-hop", 0.5, 8, 0.525, 1, true);
    let w = pick_winner(&[more_fp, less_fp]).expect("a passing grid has a winner");
    assert_eq!(w.candidate.rule, "chunk-hop", "equal hit@2 ⇒ lower FP wins");
    assert_eq!(w.two_stage.fp, 1);
}

#[test]
fn picker_breaks_metric_ties_toward_rule_i() {
    // GLM r1 minor-5: equal hit@2 AND FP across both rules ⇒ rule (i), the
    // simpler path.
    let i = grid_cell("restricted-cosine", 0.5, 8, 0.525, 2, true);
    let ii = grid_cell("chunk-hop", 0.5, 8, 0.525, 2, true);
    let w = pick_winner(&[ii, i]).expect("a passing grid has a winner");
    assert_eq!(w.candidate.rule, "restricted-cosine");
}

#[test]
fn picker_prefers_stricter_floor_on_full_ties() {
    // Documented extension beyond the three pinned tie-break keys: equal
    // hit@2, FP, AND rule ⇒ the stricter (higher) floor — the measured
    // letter is identical, and a higher floor admits strictly fewer marginal
    // opens. Makes the pinned order TOTAL (deterministic picks).
    let loose = grid_cell("restricted-cosine", 0.50, 8, 0.525, 2, true);
    let strict = grid_cell("restricted-cosine", 0.55, 8, 0.525, 2, true);
    let w = pick_winner(&[loose, strict]).expect("a passing grid has a winner");
    assert!(
        (w.candidate.floor - 0.55).abs() < 1e-6,
        "full tie ⇒ stricter floor, got {}",
        w.candidate.floor
    );
}

#[test]
fn picker_refuses_when_no_cell_passes_the_letter() {
    let a = grid_cell("restricted-cosine", 0.5, 8, 0.475, 2, false);
    let b = grid_cell("chunk-hop", 0.5, 8, 0.500, 4, false);
    let err = pick_winner(&[a, b]).expect_err("no passing cell ⇒ refuse to pick");
    let msg = format!("{err:#}");
    assert!(msg.contains("no grid cell passes"), "got: {msg}");
    // The refusal prints the best cell's letter numbers (§7 auditability).
    assert!(msg.contains("ts_hit2"), "got: {msg}");
    assert!(msg.contains("ss_hit2"), "got: {msg}");
}

#[test]
fn picker_refuses_mixed_campaigns() {
    let mut other_run = grid_cell("chunk-hop", 0.5, 8, 0.525, 2, true);
    other_run.probes.sha256 = "a-different-paired-run".to_string();
    let err = pick_winner(&[grid_cell("chunk-hop", 0.5, 8, 0.500, 2, true), other_run])
        .expect_err("mixed campaigns ⇒ refuse");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("same paired run"),
        "must refuse to compare cells from different paired runs, got: {msg}"
    );
}

#[test]
fn picker_refuses_letter_inconsistent_artifact() {
    // Recorded verdict true, but the artifact's own numbers say FAIL — a
    // tampered or buggy artifact must never feed the picker.
    let a = grid_cell("chunk-hop", 0.5, 8, 0.475, 3, true);
    let err = pick_winner(&[a]).expect_err("inconsistent letter ⇒ refuse");
    let msg = format!("{err:#}");
    assert!(msg.contains("contradict"), "got: {msg}");
}

#[test]
fn pick_report_emits_composite_key_and_patch_snippet() {
    let winner = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    let other = grid_cell("restricted-cosine", 0.5, 8, 0.500, 2, true);
    let rep = pick_report(&[winner, other]).expect("report");
    assert_eq!(
        rep.winner.gate_key,
        "external:qwen/qwen3-embedding-4b:instr1:two-stage:chunk-hop"
    );
    assert_eq!(rep.winner.rule, "chunk-hop");
    assert!((rep.winner.floor - 0.55).abs() < 1e-6);
    assert_eq!(rep.winner.k, 12);
    assert_eq!(rep.cells_considered, 2);
    assert_eq!(rep.passing_cells, 2);
    // Ready-to-apply patch snippet: names BOTH tables and carries the exact
    // composite row + k row.
    assert!(rep.patch_snippet.contains("WISDOM_GATE_FLOORS"));
    assert!(rep.patch_snippet.contains("TWO_STAGE_K"));
    assert!(rep.patch_snippet.contains("two-stage:chunk-hop"));
    assert!(
        rep.patch_snippet.contains("0.55"),
        "got: {}",
        rep.patch_snippet
    );
    assert!(
        rep.patch_snippet.contains("12"),
        "got: {}",
        rep.patch_snippet
    );
}

#[test]
fn pick_report_rule_i_winner_omits_the_k_row() {
    // Rule (i) scores the whole rule-reachable set — stage-1 breadth does
    // not apply, so a rule-(i) flip needs NO `TWO_STAGE_K` row (k is inert).
    let i = grid_cell("restricted-cosine", 0.55, 8, 0.525, 2, true);
    let rep = pick_report(&[i]).expect("report");
    assert!(rep.patch_snippet.contains("WISDOM_GATE_FLOORS"));
    assert!(
        !rep.patch_snippet.contains("TWO_STAGE_K"),
        "rule (i) must not emit a k row, got: {}",
        rep.patch_snippet
    );
}

#[test]
fn grid_cells_reads_artifacts_and_skips_unparseable_files() {
    // The grid dir may legitimately hold the previous pick's report or the
    // flip package: non-artifact JSON is skipped with a stderr note, not a
    // refusal. Zero artifacts IS a refusal (separate test).
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, cell) in [
        (
            "cell-a.json",
            grid_cell("chunk-hop", 0.5, 8, 0.525, 2, true),
        ),
        (
            "cell-b.json",
            grid_cell("restricted-cosine", 0.5, 8, 0.500, 2, true),
        ),
    ] {
        std::fs::write(dir.path().join(name), serde_json::to_string(&cell).unwrap())
            .expect("write cell");
    }
    std::fs::write(dir.path().join("garbage.json"), "{not json").expect("write garbage");
    std::fs::write(dir.path().join("notes.txt"), "not json at all").expect("write txt");
    let cells = grid_cells(dir.path()).expect("grid");
    assert_eq!(cells.len(), 2, "unparseable/non-artifact files are skipped");
}

// ── 8b. task-15-fix: emitted k row binds at the resolver; smaller-k tie-break;
//        grid-wide build provenance ──────────────────────────────────────────

/// I-1 (task-15-fix): the production k resolver looks rows up by the
/// UNSCHEMED profile key (`two_stage_k(gate_model_key(..))`,
/// wisdom_match.rs:666 via tools/src/queries.rs:839). The emitted
/// `TWO_STAGE_K` row must be keyed by THAT key for BOTH schemes — under
/// `raw` the two coincide, but an `instr1` baseline.key carries the
/// `:instr1` suffix the resolver's exact find would miss (the pasted row
/// would be inert and production would silently run k=8). Resolved exactly
/// as an operator's paste would bind: `TWO_STAGE_K` + the pasted row, the
/// resolver's exact-find-then-default lookup (`two_stage_k_in` — production
/// `TWO_STAGE_K` ships unflipped, so the pasted row can only live in a test
/// table; `two_stage_k` delegates to the same lookup).
#[test]
fn emitted_k_row_binds_at_the_resolver_for_both_schemes() {
    let profile = tauri_app_lib::embedder::EmbedProfile::External {
        profile: tauri_app_lib::embedder::ExternalEmbedProfile {
            base_url: String::new(),
            model: "qwen/qwen3-embedding-4b".to_string(),
            api_key: None,
        },
    };
    // The key the runtime resolver actually looks up (queries.rs:839).
    let profile_key = tauri_app_lib::wisdom_match::gate_model_key(&profile, None);
    assert_eq!(profile_key, "external:qwen/qwen3-embedding-4b");

    for scheme in ["instr1", "raw"] {
        let mut winner = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
        let read_scheme = if scheme == "instr1" {
            Scheme::Instr1
        } else {
            Scheme::Raw
        };
        winner.baseline.scheme = scheme.to_string();
        winner.baseline.key = tauri_app_lib::embed_scheme::floor_key_for(&profile_key, read_scheme);
        let snippet = patch_snippet(&winner);
        // The pasted row: the FIRST ("key", k) row AFTER the TWO_STAGE_K
        // marker (the composite floor row precedes it).
        let after = snippet.split("TWO_STAGE_K").nth(1).expect("k row section");
        let k_key = after
            .split("(\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("k row key");
        let pasted: Vec<(&str, usize)> = tauri_app_lib::wisdom_match::TWO_STAGE_K
            .iter()
            .copied()
            .chain(std::iter::once((k_key, winner.candidate.k)))
            .collect();
        assert_eq!(
            tauri_app_lib::wisdom_match::two_stage_k_in(&pasted, &profile_key),
            winner.candidate.k,
            "scheme {scheme}: the emitted k row must bind at the resolver \
             (emitted key was {k_key:?})"
        );
    }
}

/// M-1 (task-15-fix): full tie on (hit@2, FP, rule, floor), cells differing
/// only in k ⇒ the smaller k — pinned under BOTH input orders (the pick must
/// be permutation-stable, not sort-accidental).
#[test]
fn picker_prefers_smaller_k_on_full_metric_tie() {
    let small_a = grid_cell("restricted-cosine", 0.55, 8, 0.525, 2, true);
    let large_a = grid_cell("restricted-cosine", 0.55, 12, 0.525, 2, true);
    // Reversed order: without the k tie-break the stable sort would keep the
    // larger-k cell first.
    let w_rev = pick_winner(&[large_a, small_a]).expect("a passing grid has a winner");
    assert_eq!(w_rev.candidate.k, 8, "full tie ⇒ smaller k");
    let small_b = grid_cell("restricted-cosine", 0.55, 8, 0.525, 2, true);
    let large_b = grid_cell("restricted-cosine", 0.55, 12, 0.525, 2, true);
    let w_fwd = pick_winner(&[small_b, large_b]).expect("a passing grid has a winner");
    assert_eq!(w_fwd.candidate.k, 8, "tie-break must be order-stable");
}

/// M-3 (task-15-fix): a grid holding artifacts from TWO builds of the driver
/// must be refused — `code_sha` is the build provenance compared across
/// cells, alongside the baseline arm and probe-set checks.
#[test]
fn picker_refuses_grid_mixing_builds() {
    let mut other_build = grid_cell("chunk-hop", 0.5, 8, 0.525, 2, true);
    other_build.code_sha = "a-different-build".to_string();
    let err = pick_winner(&[grid_cell("chunk-hop", 0.5, 8, 0.500, 2, true), other_build])
        .expect_err("a grid across builds must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("code_sha"),
        "the refusal must name the build provenance, got: {msg}"
    );
    assert!(msg.contains("a-different-build"), "got: {msg}");
}

#[test]
fn grid_cells_refuses_empty_grid_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = grid_cells(dir.path()).expect_err("empty dir ⇒ refuse");
    assert!(
        format!("{err:#}").contains("no calibration artifacts"),
        "got: {err:#}"
    );
}

// ── 9. Flip: letter-gated refusals + flip package (plan §Task 15) ────────────

#[test]
fn flip_refuses_missing_artifact() {
    let p = std::env::temp_dir().join("ct-t15-no-such-artifact.json");
    let _ = std::fs::remove_file(&p);
    let err = load_artifact(&p).expect_err("missing file ⇒ refuse");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("no such") || msg.contains("read"),
        "got: {msg}"
    );
    assert!(msg.contains("ct-t15-no-such-artifact.json"), "got: {msg}");
}

#[test]
fn flip_refuses_stale_artifact() {
    // The artifact was produced by a DIFFERENT build than the one asked to
    // flip — the flip must refuse (code_sha is the freshness proof, M-3).
    let a = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    let err = flip_plan(&a, "a-different-build").expect_err("stale ⇒ refuse");
    let msg = format!("{err:#}");
    assert!(msg.contains("STALE"), "got: {msg}");
    assert!(msg.contains("test-code-sha-fixture"), "got: {msg}");
    assert!(msg.contains("a-different-build"), "got: {msg}");
}

#[test]
fn flip_refuses_unversioned_artifact() {
    // An artifact without code provenance cannot be tied to a versioned
    // calibration build — refuse even if the current build matches.
    let mut a = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    a.code_sha = "unversioned".to_string();
    let err = flip_plan(&a, "unversioned").expect_err("unversioned ⇒ refuse");
    assert!(
        format!("{err:#}").contains("code provenance"),
        "got: {err:#}"
    );
}

#[test]
fn flip_refuses_unknown_schema() {
    let mut a = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    a.schema = 2;
    let err = flip_plan(&a, FIXTURE_CODE_SHA).expect_err("unknown schema ⇒ refuse");
    assert!(format!("{err:#}").contains("schema"), "got: {err:#}");
}

#[test]
fn flip_refuses_failing_letter_and_prints_the_letter_numbers() {
    // §7: ts_hit2 0.475 < ss_hit2 0.5 ⇒ the letter FAILS. The refusal must
    // print all four letter numbers so the operator sees WHY.
    let a = grid_cell("chunk-hop", 0.55, 12, 0.475, 2, false);
    let err = flip_plan(&a, FIXTURE_CODE_SHA).expect_err("failing letter ⇒ refuse");
    let msg = format!("{err:#}");
    for needle in [
        "does NOT pass",
        "ts_hit2",
        "0.475",
        "ts_fp",
        "2",
        "ss_hit2",
        "0.5",
        "ss_fp",
    ] {
        assert!(
            msg.contains(needle),
            "refusal must print {needle:?}; got: {msg}"
        );
    }
}

#[test]
fn flip_refuses_letter_inconsistent_artifact() {
    let a = grid_cell("chunk-hop", 0.55, 12, 0.475, 3, true);
    let err = flip_plan(&a, FIXTURE_CODE_SHA).expect_err("inconsistent letter ⇒ refuse");
    assert!(format!("{err:#}").contains("contradict"), "got: {err:#}");
}

#[test]
fn flip_accepts_passing_artifact_and_writes_freeze_package() {
    let a = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    let plan = flip_plan(&a, FIXTURE_CODE_SHA).expect("passing letter ⇒ flip proceeds");
    assert_eq!(
        plan.gate_key,
        "external:qwen/qwen3-embedding-4b:instr1:two-stage:chunk-hop"
    );
    assert_eq!(plan.rule, "chunk-hop");
    assert!((plan.floor - 0.55).abs() < 1e-6);
    assert_eq!(plan.k, 12);

    let out = tempfile::tempdir().expect("tempdir");
    let written = write_flip_package(&plan, out.path()).expect("write package");
    assert_eq!(written.len(), 2, "expected.json + patch snippet");
    let expected_path = out
        .path()
        .join(&plan.freeze_relative_dir)
        .join("expected.json");
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&expected_path).expect("read expected.json"))
            .expect("expected.json is valid JSON");
    assert!((expected["floor"].as_f64().unwrap() - 0.55).abs() < 1e-6);
    assert_eq!(expected["rule"].as_str().unwrap(), "chunk-hop");
    assert_eq!(expected["k"].as_u64().unwrap(), 12);
    let patch =
        std::fs::read_to_string(out.path().join("two-stage-flip.patch")).expect("read patch");
    assert!(
        patch.contains("WISDOM_GATE_FLOORS") && patch.contains("two-stage:chunk-hop"),
        "got: {patch}"
    );
}

/// M-2 (task-15-fix): `flip` bypasses `check_grid` (`cmd_flip` goes
/// `load_artifact` → `flip_plan` directly), so `flip_plan` itself must
/// refuse a rule outside the two-stage vocabulary — otherwise a hand-edited
/// artifact with a self-consistent passing letter would reach the flip
/// package with a nonsense composite key. Same bail wording `check_grid`
/// uses for grids.
#[test]
fn flip_refuses_unknown_rule() {
    let mut artifact = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    artifact.candidate.rule = "magic-vibes".to_string();
    let err = flip_plan(&artifact, FIXTURE_CODE_SHA).expect_err("unknown rule ⇒ refuse");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unknown two-stage rule"),
        "reuse check_grid's bail wording, got: {msg}"
    );
    assert!(msg.contains("magic-vibes"), "got: {msg}");
}

// ── 10. Simulated flip satisfies the bench freeze-dir assertion ──────────────

#[test]
fn simulated_flip_freeze_dir_matches_bench_convention() {
    // Mirrors `freeze_dir_for`'s `other` arm in
    // src-tauri/tests/wisdom_gate_bench.rs: ':' and '/' → '_' under
    // wisdom_gate_bench/ (plan §Task 6).
    assert_eq!(
        freeze_relative_dir("external:qwen/qwen3-embedding-4b:instr1:two-stage:chunk-hop"),
        "wisdom_gate_bench/external_qwen_qwen3-embedding-4b_instr1_two-stage_chunk-hop"
    );
}

#[test]
fn simulated_flip_satisfies_freeze_dir_assertion() {
    // Plan §Task 15 Step-1 test 3: after a SIMULATED flip (this test never
    // touches the real repo tree), EVERY non-stub floor key — the REAL
    // `WISDOM_GATE_FLOORS` rows plus the flip's composite key — has a freeze
    // dir holding expected.json. The flip's package supplies the new key's
    // dir; the historical keys keep their committed fixture dirs.
    let a = grid_cell("chunk-hop", 0.55, 12, 0.525, 2, true);
    let plan = flip_plan(&a, FIXTURE_CODE_SHA).expect("passing artifact");
    let out = tempfile::tempdir().expect("tempdir");
    write_flip_package(&plan, out.path()).expect("write package");

    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools has a parent (repo root)")
        .join("src-tauri/tests/fixtures");
    // The pinned historical mapping (bench `freeze_dir_for` match arms).
    let mut covered: Vec<(&str, std::path::PathBuf)> = vec![
        (
            "external:qwen/qwen3-embedding-4b",
            fixtures.join("wisdom_gate"),
        ),
        (
            "external:qwen/qwen3-embedding-4b:instr1",
            fixtures.join("wisdom_gate_instr1"),
        ),
    ];
    // Every REAL floor row not covered above follows the sanitized
    // convention (today the production table has only the two historical
    // keys; if a future key lands without a snapshot, THIS assert fails —
    // same tripwire as the bench test).
    for (k, _) in tauri_app_lib::wisdom_match::WISDOM_GATE_FLOORS {
        if k.starts_with("stub:") || covered.iter().any(|(c, _)| c == k) {
            continue;
        }
        covered.push((
            k,
            fixtures
                .join("wisdom_gate_bench")
                .join(k.replace(':', "_").replace('/', "_")),
        ));
    }
    // The simulated flip adds the winning composite key with the flip's
    // OWN freeze dir (written into the temp out dir).
    let flip_dir = out.path().join(&plan.freeze_relative_dir);
    assert!(
        flip_dir.join("expected.json").exists(),
        "flip package must place expected.json at the bench-convention path"
    );

    let mut keys: Vec<String> = covered.iter().map(|(k, _)| k.to_string()).collect();
    keys.push(plan.gate_key.clone());
    for key in &keys {
        let dir = if *key == plan.gate_key {
            flip_dir.clone()
        } else {
            covered
                .iter()
                .find(|(c, _)| c == key)
                .map(|(_, d)| d.clone())
                .expect("covered key")
        };
        assert!(
            dir.join("expected.json").exists(),
            "after the simulated flip, {key} must have a freeze snapshot at {}",
            dir.join("expected.json").display()
        );
    }

    // Floor consistency: the flip's expected.json carries the artifact's
    // floor (the bench replay asserts `gate_floor(key) == expected.floor`).
    let expected: serde_json::Value = serde_json::from_slice(
        &std::fs::read(flip_dir.join("expected.json")).expect("read expected.json"),
    )
    .expect("valid JSON");
    assert!((expected["floor"].as_f64().unwrap() - 0.55).abs() < 1e-6);
}

// ── pr283-review finding 14: the baseline arm's gate key ─────────────────────────────

/// The baseline arm passes its key to `wisdom_match_in_scheme`, which
/// resolves the floor as `floor_key_for(gate_key, scheme)` — so the key it
/// receives must be the UNSCHEMED profile key. Round-trip property: for
/// every scheme, `floor_key_for(baseline_gate_key(schemed_key, scheme),
/// scheme) == schemed_key` — the key the arm passes resolves to the
/// REGISTERED schemed floor, never to a double-suffixed `…:instr1:instr1`.
#[test]
fn baseline_gate_key_round_trips_through_floor_key_for() {
    use tauri_app_lib::embed_scheme::floor_key_for;

    let profile_key = "local:nomic-embed-code";
    for scheme in [Scheme::Raw, Scheme::Instr1] {
        let schemed = floor_key_for(profile_key, scheme);
        let unschemed = calibrate_two_stage::baseline_gate_key(&schemed, &scheme);
        assert_eq!(
            unschemed, profile_key,
            "{scheme:?}: the baseline arm's key must be the UNSCHEMED profile key"
        );
        assert_eq!(
            floor_key_for(&unschemed, scheme),
            schemed,
            "{scheme:?}: the arm's key must resolve back to the registered \
             schemed floor key"
        );
    }
    // The exact production ghost: the schemed key passed verbatim produces
    // the double suffix the old code measured the letter against.
    let schemed = floor_key_for(profile_key, Scheme::Instr1);
    assert_eq!(
        floor_key_for(&schemed, Scheme::Instr1),
        format!("{schemed}:instr1"),
        "passing the already-schemed key double-suffixes it"
    );
}
