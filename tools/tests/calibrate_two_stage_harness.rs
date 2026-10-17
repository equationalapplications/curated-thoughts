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
//!    (`ct reindex repair-embed-key --stale`) (§9 step 3a, r25-M1).
//!
//! The driver's pure logic lives in the bin source, included here as a module
//! (integration tests cannot import a bin target directly).

// `fn main` and the live-only helpers are dead in a test build.
#[allow(dead_code)]
#[path = "../src/bin/calibrate_two_stage.rs"]
mod calibrate_two_stage;

use calibrate_two_stage::{
    arm_metrics, check_coverage, check_embed_stub, letter_verdict, stratified_splits, ArmMetrics,
    BaselineArm, CalibrationArtifact, CandidateCell, Coverage, EmbedStats, GridInfo, HeldOutInfo,
    HeldOutSplit, InjectionStats, MergeAttribution, ProbeSetInfo, ProbeVerdict, TopKAblation,
    V1MergeStats,
};

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
    let artifact = CalibrationArtifact {
        schema: 1,
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
