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
//!    cut-to-max drift simultaneously (task-14-fix I-1).
//!
//! The driver's pure logic lives in the bin source, included here as a module
//! (integration tests cannot import a bin target directly).

// `fn main` and the live-only helpers are dead in a test build.
#[allow(dead_code)]
#[path = "../src/bin/calibrate_two_stage.rs"]
mod calibrate_two_stage;

use calibrate_two_stage::{
    arm_metrics, check_coverage, check_embed_stub, eligibility_where, letter_verdict,
    score_entries, stratified_splits, temporal_shape, ArmMetrics, BaselineArm, CalibrationArtifact,
    CandidateCell, Coverage, EmbedStats, GridInfo, HeldOutInfo, HeldOutSplit, InjectionStats,
    MergeAttribution, ProbeSetInfo, ProbeVerdict, TemporalShape, TopKAblation, V1MergeStats,
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

#[test]
fn artifact_round_trips_through_json() {
    // M-3 fixture: a `#[cfg(test)]`-only provenance value — the test crate
    // cannot inject a compile-time `CT_BUILD_SHA`; the LIVE binary reads
    // `option_env!("CT_BUILD_SHA")` (fallback "unversioned") at the
    // artifact-construction site (`code_sha()`).
    const FIXTURE_CODE_SHA: &str = "test-code-sha-fixture";
    let artifact = CalibrationArtifact {
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
    };
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
