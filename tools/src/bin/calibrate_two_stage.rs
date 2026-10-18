//! `calibrate_two_stage` — paired live two-stage vs single-stage calibration
//! driver (issue #271, plan Task 14; spec §7 + §9 step 3).
//!
//! Runs BOTH arms in the SAME invocation against the LIVE brain (read-only
//! `open_ro`) over the 150-probe real-traffic JSONL set (40 relevant + 110
//! FP probes, §7/r24 m7) and writes the calibration artifact with Kurt's
//! PINNED acceptance letter:
//!
//! ```text
//! two_stage_passes = ts_hit2 >= ss_hit2 && ts_fp <= ss_fp
//! ```
//!
//! (exact boundary: equal hit@2 + equal FP PASSES; no absolute floor — §7).
//!
//! Baseline arm (Opus r1 M3): the CURRENTLY SHIPPED default — named by its
//! model/scheme key (`--baseline-key`, which must equal the live
//! `floor_key_for(profile, scheme)` key) and its registered floor, recorded
//! in the artifact BEFORE the run. No post-hoc arm picking.
//!
//! hit@2 and FP are derived HERE from the probe labels plus the pinned
//! `open`/`top` audit facts — the gate cannot compute them (r25 m1). FP is a
//! gate-OPEN decision on an FP probe (§3: the audit measures gate-OPEN
//! decisions).
//!
//! Guards:
//! - `CURATED_EMBED_STUB` must be UNSET (refuses otherwise, exactly like
//!   `calibrate_wisdom_gate`) — real embeddings only;
//! - candidate-set coverage must be 100% BEFORE any embed (§6/§9 step 3a);
//!   below 100% the run ABORTS with instructions to run Task 13
//!   (`ct reindex verify-scratch`) then Task 12
//!   (`ct reindex repair-embed-key --stale`) — the two-stage arm must not
//!   run on a skewed set.
//!
//! Per-probe embed plan (r26 m3): under `Scheme::Raw` ONE embed per message
//! (the stage-1 raw vector IS the fact-vector embed and is REUSED); under
//! `instr1` TWO (raw stage-1 embed + prefixed fact-vector embed). Counts and
//! latencies are recorded in the artifact (r25 E6).
//!
//! The harness SHIPS; the live run itself is §9 step 3, manual. The offline
//! logic (letter verdict, artifact shape, stratified splits, refusals) is
//! tested by `tools/tests/calibrate_two_stage_harness.rs`.
//!
//! Task 15 adds the OFFLINE decision machinery (spec §9 step 3, §7): `pick
//! --grid <dir>` sweeps per-cell paired artifacts, applies the pinned
//! tie-break (GLM r1 minor-5: higher hit@2 → lower FP → rule (i)) and emits
//! the winning `(rule, floor, k)` + a ready-to-apply patch snippet;
//! `flip --artifact <path>` proceeds ONLY on a passing artifact current for
//! this build (refusing otherwise, printing the letter numbers) and writes
//! the flip package. Neither subcommand runs the flip: `WISDOM_GATE_FLOORS`
//! gains the winning composite key ONLY when a real passing artifact's
//! package is applied after the live calibration (§7; until then the
//! shipped tree stays opt-in).

use anyhow::{bail, Context, Result};
use clap::Parser;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use curated_thoughts_tools::reindex::now_ms_at;
use curated_thoughts_tools::write::{open_ro, resolve};

use tauri_app_lib::embed_scheme::{
    floor_key_for, query_text_for_scheme, read_scheme_for_reader, Scheme,
};
use tauri_app_lib::embedder::{embed_batch, EmbedProfile};
use tauri_app_lib::search::{bytes_to_f32, cosine_similarity};
use tauri_app_lib::two_stage::{
    candidate_set, complement_entry_ids, coverage, member_chunk_ids, stage1_search, CandidateSet,
};
use tauri_app_lib::wiki_graph::tier_weight;
use tauri_app_lib::wisdom_match::{
    gate_floor, gate_model_key, provenance_for, truncate_text, two_stage_entries,
    wisdom_match_in_scheme, TwoStageRule, WisdomItem,
};

/// `hit@2`: both arms run with `max = 2` — a relevant probe hits iff any of
/// the top-2 entry ids is in the probe's relevant-fact set.
const HIT_K: usize = 2;
/// Probe embed batch size (mirrors `calibrate_wisdom_gate`).
const BATCH: usize = 32;
/// Held-out splits recorded INFORMATIONALLY (r24 m7/E5): five stratified
/// 75/75 splits over the 150-probe set; no pass/fail hangs on them (§7).
const HELDOUT_SPLITS: usize = 5;
const HELDOUT_SEED: u64 = 271_828;

#[derive(Parser)]
struct Args {
    /// `pick` sweeps a grid of per-cell artifacts; `flip` applies the
    /// letter-gated flip package. Omitted = the paired LIVE run (§9 step 3).
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Probe JSONL in the EXISTING `calibrate_wisdom_gate` format (serde
    /// `Probe`: `text`, `expect` = relevant-fact ids, optional `vector`),
    /// one line per probe. Do NOT invent a new schema.
    ///
    /// Flat run args are `Option` ONLY so `pick`/`flip` stay parseable —
    /// `main` enforces them (with clap-shaped errors) on the live-run path.
    #[arg(long)]
    probes: Option<PathBuf>,
    /// The calibrated two-stage rule: `restricted-cosine` (rule i) or
    /// `chunk-hop` (rule ii).
    #[arg(long)]
    rule: Option<String>,
    /// The candidate two-stage floor (the rule's OWN decision variable:
    /// restricted fact cosine for rule i, member-CHUNK score for rule ii).
    #[arg(long)]
    floor: Option<f32>,
    /// Stage-1 chunk breadth for rule (ii) (the `TWO_STAGE_K` knob, default
    /// 8 — k is NOT a floor key). Ignored under `restricted-cosine` (rule i
    /// scores the whole rule-reachable set; stage-1 breadth does not apply).
    #[arg(long)]
    k: Option<usize>,
    /// The v1 baseline's model/scheme key — must equal the LIVE
    /// `floor_key_for(gate_model_key(profile), scheme)` key (Opus r1 M3:
    /// baseline named BEFORE the run, no post-hoc arm picking).
    #[arg(long)]
    baseline_key: Option<String>,
    /// Write the artifact JSON here (default: stdout).
    #[arg(long)]
    json_out: Option<PathBuf>,
    /// The query kind this probe set calibrates (issue #282 §7):
    /// `message` (default — the #271 discipline) or one of the activity
    /// kinds `path` / `command` / `pattern`. KIND SEMANTICS (rev-8 m4): the
    /// kind changes probe-set labeling/filtering and the per-kind breakdown
    /// ONLY — never the floor key (ONE shared `:activity` floor across all
    /// three kinds, Kurt ruling 2026-10-18). An activity run ALSO implies:
    /// `v1_floor = None` at every pass-site (the merge never runs in
    /// activity mode), the `user_stated` aperture in scoring, and
    /// `user_stated`-only positives (rev-6 M3) — plus hit@1 as the gated
    /// metric (rev-7 M2).
    #[arg(long)]
    query_kind: Option<String>,
    /// EmbedProfile JSON, as in `calibrate_wisdom_gate` (production default:
    /// OpenRouter `qwen/qwen3-embedding-4b`).
    #[arg(
        long,
        default_value = r#"{"type":"external","base_url":"https://openrouter.ai/api/v1","model":"qwen/qwen3-embedding-4b"}"#
    )]
    profile: String,
}

/// The OFFLINE subcommands (Task 15). The paired live run is the default
/// (no subcommand) — same interface as Task 14 shipped.
#[derive(clap::Subcommand)]
enum Cmd {
    /// Sweep the (rule × floor × k) grid from per-cell paired artifacts in
    /// DIR, apply the pinned tie-break (GLM r1 minor-5), and emit the
    /// winning (rule, floor, k) + a ready-to-apply patch snippet for
    /// `WISDOM_GATE_FLOORS`/`TWO_STAGE_K` (§9 step 3).
    Pick {
        /// Directory holding one `CalibrationArtifact` JSON per grid cell
        /// (each written by a separate paired-driver invocation).
        #[arg(long)]
        grid: PathBuf,
        /// Write the pick report JSON here (default: stdout only).
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
    /// Load a calibration artifact and — ONLY if `two_stage_passes == true`
    /// AND the artifact is current for THIS build (§7) — write the flip
    /// package (freeze-dir `expected.json` + ready-to-apply patch snippet).
    /// Refuses otherwise, printing the letter numbers.
    Flip {
        /// The paired-run artifact to flip on.
        #[arg(long)]
        artifact: PathBuf,
        /// Output directory for the flip package (default: the artifact's
        /// own directory).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

/// Probe schema: byte-compatible with `calibrate_wisdom_gate`'s `Probe`
/// (`read_jsonl::<Probe>`) — same fields, same JSONL shape.
#[derive(Deserialize, Serialize, Clone)]
struct Probe {
    text: String,
    expect: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vector: Vec<f32>,
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(p: &PathBuf) -> Result<(Vec<T>, String)> {
    let raw = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    let sha = hex::encode(Sha256::digest(&raw));
    let items = String::from_utf8(raw)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<T>, _>>()?;
    Ok((items, sha))
}

// ── Offline-testable pure logic ──────────────────────────────────────────────

/// Kurt's PINNED acceptance letter (§7): two-stage passes iff it is not
/// worse than single-stage on EITHER axis — hit@2 ≥ AND FP ≤. Exact
/// boundary: equal hit@2 + equal FP PASSES.
pub fn letter_verdict(ts_hit2: f64, ts_fp: usize, ss_hit2: f64, ss_fp: usize) -> bool {
    ts_hit2 >= ss_hit2 && ts_fp <= ss_fp
}

/// The ACTIVITY letter (issue #282 §7): `act_hit1 ≥ base_hit1 && act_fp ≤
/// base_fp` — hit@1 is the gated metric (the integrations fetch `--max 1`
/// and the generic-vs-actionable failure mode is a TOP-1 failure); hit@2 is
/// recorded, not gated.
pub fn letter_verdict_hit1(act_hit1: f64, act_fp: usize, base_hit1: f64, base_fp: usize) -> bool {
    act_hit1 >= base_hit1 && act_fp <= base_fp
}

/// Which metric a cell's letter was evaluated on (rev-7 M2: activity cells
/// gate on hit@1; message cells keep the #271 hit@2 letter).
pub fn letter_metric_of(artifact: &CalibrationArtifact) -> &'static str {
    if artifact.letter_metric.as_deref() == Some("hit1") {
        "hit1"
    } else {
        "hit2"
    }
}

/// The cell's gated two-stage hit under its OWN letter metric.
pub fn gated_hit(artifact: &CalibrationArtifact) -> f64 {
    if letter_metric_of(artifact) == "hit1" {
        artifact.two_stage.hit1
    } else {
        artifact.two_stage.hit2
    }
}

/// The cell's baseline hit under its OWN letter metric.
pub fn baseline_hit(artifact: &CalibrationArtifact) -> f64 {
    if letter_metric_of(artifact) == "hit1" {
        artifact.single_stage.hit1
    } else {
        artifact.single_stage.hit2
    }
}

/// The recorded letter recomputed under the cell's own metric (check_grid).
pub fn recorded_letter(artifact: &CalibrationArtifact) -> bool {
    if letter_metric_of(artifact) == "hit1" {
        letter_verdict_hit1(
            artifact.two_stage.hit1,
            artifact.two_stage.fp,
            artifact.single_stage.hit1,
            artifact.single_stage.fp,
        )
    } else {
        letter_verdict(
            artifact.two_stage.hit2,
            artifact.two_stage.fp,
            artifact.single_stage.hit2,
            artifact.single_stage.fp,
        )
    }
}

/// Refuse to run with `CURATED_EMBED_STUB` set — real embeddings only
/// (same rule as `calibrate_wisdom_gate`). Pure: the caller passes the env
/// value so tests never mutate process env.
pub fn check_embed_stub(stub: Option<&OsStr>) -> Result<()> {
    if stub.is_some() {
        bail!("refusing to calibrate with CURATED_EMBED_STUB set — real embeddings only");
    }
    Ok(())
}

/// Candidate-set coverage at run time (§6/§7: the artifact records the
/// fraction, which must be 100% for the two-stage arm to have run at all).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub candidate_docs: i64,
    pub covered: i64,
    pub fraction: f64,
}

/// Coverage gate (§9 step 3a): the two-stage arm must not run on a skewed
/// set. A probe miss (`None`, pre-V28 brain) cannot record 100% ⇒ refuse
/// too. Below 100% the run ABORTS with the repair ladder: Task 13
/// (`ct reindex verify-scratch`) first, then Task 12
/// (`ct reindex repair-embed-key --stale`, r25-M1) if still short.
pub fn check_coverage(probe: Option<(i64, i64)>) -> Result<Coverage> {
    let Some((candidate_docs, covered)) = probe else {
        bail!(
            "candidate-set coverage probe unavailable (pre-V28 brain) — the two-stage arm \
             must not run on an unverified vector population. \
             Run `ct reindex verify-scratch` first; if coverage is still below 100%, \
             run `ct reindex repair-embed-key --stale`, then re-verify coverage = 100% \
             before calibrating."
        );
    };
    if covered < 0 || covered > candidate_docs {
        bail!("implausible coverage probe ({covered}/{candidate_docs}) — refusing to calibrate");
    }
    if covered < candidate_docs {
        bail!(
            "candidate-set coverage {covered}/{candidate_docs} < 100% — the two-stage arm \
             must not run on a skewed set (a mixed corpus would be measured as if \
             homogeneous). Run `ct reindex verify-scratch` first; if coverage is still \
             below 100%, run `ct reindex repair-embed-key --stale`, then re-verify \
             coverage = 100% before calibrating."
        );
    }
    Ok(Coverage {
        candidate_docs,
        covered,
        fraction: 1.0,
    })
}

/// One probe's runtime outcome under one arm, derived from the pinned
/// audit facts + the final entries (r25 m1: the harness derives hit/FP from
/// probe labels plus `open`/`top` — the gate cannot compute them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeVerdict {
    /// Relevant probe: any of the top-2 entry ids is in the probe's
    /// relevant-fact set.
    pub hit: bool,
    /// Relevant probe: the TOP-1 entry id is in the probe's relevant-fact
    /// set (issue #282 §7 — the gated metric for activity cells).
    pub hit1: bool,
    /// Gate-OPEN decision (r30-m5: the FINAL entries list is non-empty).
    pub open: bool,
    pub entries: usize,
}

/// hit@2 / FP / opens / entries-per-open for one arm over one probe subset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmMetrics {
    /// hits / relevant probes (steps of 1/40 = 0.025 on the live set, §7).
    pub hit2: f64,
    /// TOP-1 hits / relevant probes (issue #282 §7 — the gated metric for
    /// activity cells; recorded for message cells).
    pub hit1: f64,
    /// FP probes (empty `expect`) where the gate OPENED.
    pub fp: usize,
    pub opens: usize,
    /// Mean entries per open (0.0 when the arm never opened).
    pub entries_per_open: f64,
}

/// Reduce per-probe verdicts + labels into arm metrics (pure; the same fn
/// scores the full run and each informational held-out split).
pub fn arm_metrics(verdicts: &[ProbeVerdict], relevant: &[bool]) -> ArmMetrics {
    debug_assert_eq!(verdicts.len(), relevant.len());
    let n_rel = relevant.iter().filter(|&&r| r).count();
    let hits = verdicts
        .iter()
        .zip(relevant)
        .filter(|(v, &r)| r && v.hit)
        .count();
    let opens = verdicts.iter().filter(|v| v.open).count();
    let entries: usize = verdicts.iter().map(|v| v.entries).sum();
    let hits1 = verdicts
        .iter()
        .zip(relevant)
        .filter(|(v, &r)| r && v.hit1)
        .count();
    ArmMetrics {
        hit2: if n_rel == 0 {
            0.0
        } else {
            hits as f64 / n_rel as f64
        },
        hit1: if n_rel == 0 {
            0.0
        } else {
            hits1 as f64 / n_rel as f64
        },
        fp: verdicts
            .iter()
            .zip(relevant)
            .filter(|(v, &r)| !r && v.open)
            .count(),
        opens,
        entries_per_open: if opens == 0 {
            0.0
        } else {
            entries as f64 / opens as f64
        },
    }
}

/// One stratified 75/75 split: `held_out` (the informational held-out half)
/// + `rest`, as probe indices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StratifiedSplit {
    pub held_out: Vec<usize>,
    pub rest: Vec<usize>,
}

/// `splitmix64` — tiny seeded PRNG so the splits are deterministic given a
/// seed (r24 m7/E5) with no new dependency.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn shuffled_indices(n: usize, seed: u64) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    let mut state = seed;
    for i in (1..v.len()).rev() {
        let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    v
}

/// Five (or `splits`) stratified 75/75 splits over the labeled probe set:
/// each half keeps the relevant/FP ratio exactly (40 relevant + 110 FP ⇒
/// 20 + 55 per half). Deterministic given `seed`.
pub fn stratified_splits(relevant: &[bool], splits: usize, seed: u64) -> Vec<StratifiedSplit> {
    let rel: Vec<usize> = relevant
        .iter()
        .enumerate()
        .filter(|(_, &r)| r)
        .map(|(i, _)| i)
        .collect();
    let fp: Vec<usize> = relevant
        .iter()
        .enumerate()
        .filter(|(_, &r)| !r)
        .map(|(i, _)| i)
        .collect();
    let (rel_half, fp_half) = (rel.len() / 2, fp.len() / 2);
    (0..splits)
        .map(|s| {
            // Per-split seed derivation: same seed ⇒ same splits; distinct
            // split index ⇒ independent draws.
            let sseed = seed ^ (s as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let rel_shuffled = shuffled_indices(rel.len(), sseed);
            let fp_shuffled = shuffled_indices(fp.len(), sseed ^ 0x1234_5678_9ABC_DEF0);
            let mut held_out: Vec<usize> = rel_shuffled[..rel_half]
                .iter()
                .map(|&j| rel[j])
                .chain(fp_shuffled[..fp_half].iter().map(|&j| fp[j]))
                .collect();
            held_out.sort_unstable();
            let held: HashSet<usize> = held_out.iter().copied().collect();
            let rest: Vec<usize> = (0..relevant.len()).filter(|i| !held.contains(i)).collect();
            StratifiedSplit { held_out, rest }
        })
        .collect()
}

// ── Artifact shape ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeSetInfo {
    pub path: String,
    pub sha256: String,
    pub count: usize,
    pub relevant: usize,
    pub fp: usize,
}

/// The baseline arm, named by key + floor BEFORE the run (Opus r1 M3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineArm {
    pub key: String,
    pub floor: f32,
    pub scheme: String,
}

/// The calibrated candidate cell this run measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateCell {
    pub rule: String,
    pub floor: f32,
    pub k: usize,
}

/// §7: the artifact records the grid size. One invocation measures ONE
/// (rule × floor × k) cell (`cells = 1`); the picker (Task 15) aggregates
/// across artifacts into the full grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridInfo {
    pub cells: usize,
    pub rule: String,
    pub floor: f32,
    pub k: usize,
}

/// Informational held-out split record (r24 m7/E5): no pass/fail hangs on
/// these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeldOutSplit {
    pub relevant: usize,
    pub fp: usize,
    pub single_stage_hit2: f64,
    pub two_stage_hit2: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeldOutInfo {
    pub splits: usize,
    pub half_size: usize,
    pub seed: u64,
    pub per_split: Vec<HeldOutSplit>,
}

/// Per-message embed count + latency (r25 E6 / r26 m3): ONE embed per
/// message under `raw` (stage-1 vector reused), TWO under `instr1`.
/// Latencies are per-message averages of the batched embed calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedStats {
    pub per_message: usize,
    pub scheme: String,
    pub latency_ms_p50: f64,
    pub latency_ms_p95: f64,
}

/// Rule-(ii) injection volume (r24 E7 / r25 E5): entries per open under the
/// pinned no-fact-floor reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InjectionStats {
    pub rule: String,
    pub opens: usize,
    pub entries_per_open_median: f64,
}

/// Merge attribution (r27 E4): how many opens/hits came ONLY from the §3
/// v1-merge (all final entry ids ∈ the structural complement).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergeAttribution {
    pub opens_via_merge_only: usize,
    pub hits_via_merge_only: usize,
    pub v1merged_total: usize,
}

/// v1-merge latency & volume overhead (r31 E7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V1MergeStats {
    pub latency_ms_p95: f64,
    pub volume_total: usize,
    /// `merge-segment` = the driver-side rule-(ii) merge, timed directly;
    /// `combined-call` = the core path (`two_stage_entries`) exposes no
    /// segment timing, so the full two-stage call latency is recorded.
    pub measured: String,
}

/// Rule-(ii) top-1 vs top-k ablation columns (r31 E5 / r30-m1): the arm
/// itself opens top-1 (pinned); `topk` re-scores the same run with top-k
/// member-chunk opening. `None` under `restricted-cosine` (rule i has no
/// member-chunk decision).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopKAblation {
    pub k: usize,
    pub top1: ArmMetrics,
    pub topk: ArmMetrics,
}

/// The calibration artifact (§7): both arms' hit@2/FP, the pinned letter
/// verdict, grid size, informational held-out splits, coverage fraction at
/// run time, embed count + latency (r25 E6), rule-(ii) injection volume
/// (r24 E7), merge attribution (r27 E4), v1-merge latency/volume (r31 E7),
/// top-1 vs top-k ablation columns (r31 E5), and the producing build's
/// code provenance (`code_sha`, task-14-fix M-3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationArtifact {
    pub schema: u32,
    /// Code provenance (task-14-fix M-3): the git rev of the build that
    /// produced this artifact, injected at compile time via
    /// `CT_BUILD_SHA` (controller recipe); `unversioned` when unset
    /// (offline/dev builds). A drifted side-core rebuild is auditable
    /// from the artifact the human reviews.
    pub code_sha: String,
    pub generated_utc: String,
    pub probes: ProbeSetInfo,
    pub baseline: BaselineArm,
    pub candidate: CandidateCell,
    pub grid: GridInfo,
    pub coverage: Coverage,
    pub single_stage: ArmMetrics,
    pub two_stage: ArmMetrics,
    pub two_stage_passes: bool,
    pub letter: String,
    /// Which metric the letter gated on (issue #282 rev-7 M2): `Some("hit1")`
    /// for activity cells, `None` (the #271 hit@2 letter) for message cells.
    /// `#[serde(default)]` keeps pre-#282 artifacts readable.
    #[serde(default)]
    pub letter_metric: Option<String>,
    /// The query kind this probe set calibrates (rev-8 m4): `message` or
    /// `path`/`command`/`pattern`. Labeling/breakdown only — never a floor
    /// key.
    #[serde(default)]
    pub query_kind: String,
    pub heldout: HeldOutInfo,
    pub embeds: EmbedStats,
    pub injection: InjectionStats,
    pub merge_attribution: MergeAttribution,
    pub v1_merge: V1MergeStats,
    pub ablation: Option<TopKAblation>,
    pub stale_embed_key_total: i64,
}

// ── Task 15: grid picker + letter-gated flip (spec §9 step 3, §7, GLM r1 minor-5) ──

/// Composite two-stage floor key (spec §3): `<v1 schemed key>:two-stage:<rule>`
/// — exactly the key `two_stage_rule_floor` resolves and the flip adds to
/// `WISDOM_GATE_FLOORS`. The artifact's `baseline.key` IS the v1 schemed
/// floor key (`floor_key_for(model, scheme)`, named before the run, Opus r1
/// M3), so the composite key derives offline from the artifact alone.
pub fn composite_floor_key(artifact: &CalibrationArtifact) -> String {
    // Issue #282 (CodeRabbit PR-288 Major 2): an ACTIVITY artifact's
    // composite is `{schemed base}:activity:two-stage:{rule}` — the
    // `:activity` segment lands AFTER the schemed base (raw base keys carry
    // no scheme suffix; instr1 bases are `model:instr1`, giving the pinned
    // `model:instr1:activity:two-stage:…` order, spec rev-8 m5). Message
    // artifacts keep the #271 shape unchanged.
    let base = if artifact.query_kind == "message" {
        artifact.baseline.key.clone()
    } else {
        format!("{}:activity", artifact.baseline.key)
    };
    format!("{base}:two-stage:{}", artifact.candidate.rule)
}

/// Freeze dir (relative to `src-tauri/tests/fixtures`) for a floor key —
/// mirrors `freeze_dir_for`'s `other` arm in `wisdom_gate_bench.rs` (plan
/// §Task 6): ':' and '/' → '_' under `wisdom_gate_bench/`. The two
/// historical v1 keys keep their dedicated dirs and never route through
/// here (the flip only ever writes composite two-stage keys).
pub fn freeze_relative_dir(key: &str) -> String {
    format!("wisdom_gate_bench/{}", key.replace([':', '/'], "_"))
}

/// Campaign homogeneity + integrity checks over a grid (§7: one paired run,
/// baseline named before the run, `cells = 1` per invocation). The picker
/// refuses to compare across campaigns or to trust an artifact whose
/// recorded letter verdict contradicts its own numbers.
pub fn check_grid(cells: &[CalibrationArtifact]) -> Result<()> {
    if cells.is_empty() {
        bail!(
            "no calibration artifacts in the grid — run the paired driver once per \
             (rule × floor × k) cell first (§7: one invocation measures ONE cell)"
        );
    }
    let first = &cells[0];
    for c in cells {
        if c.schema != 1 {
            bail!(
                "unsupported artifact schema {} in the grid (expected 1)",
                c.schema
            );
        }
        if c.baseline.key != first.baseline.key
            || c.baseline.floor != first.baseline.floor
            || c.baseline.scheme != first.baseline.scheme
        {
            bail!(
                "grid mixes baseline arms ({:?} vs {:?}) — the picker only compares cells \
                 of the SAME paired campaign (§7: same paired run for both arms; the \
                 baseline is named before the run, Opus r1 M3)",
                first.baseline.key,
                c.baseline.key
            );
        }
        if c.probes.sha256 != first.probes.sha256 {
            bail!(
                "grid mixes probe sets ({:?} vs {:?}) — all cells must come from the same \
                 paired run over the same probe set (§7)",
                first.probes.sha256,
                c.probes.sha256
            );
        }
        // M-3 (task-15-fix): a grid mixing artifacts from two builds of the
        // driver is not one campaign — refuse it the way a stale artifact is
        // refused at flip time, naming both builds.
        if c.code_sha != first.code_sha {
            bail!(
                "grid mixes builds (produced by {:?} but also by {:?}) — all cells must \
                 carry the SAME code_sha (one paired calibration build per grid); re-run \
                 the paired calibration so every cell is current",
                first.code_sha,
                c.code_sha
            );
        }
        if c.candidate.rule != "restricted-cosine" && c.candidate.rule != "chunk-hop" {
            bail!("unknown two-stage rule {:?} in the grid", c.candidate.rule);
        }
        let recorded = recorded_letter(c);
        if recorded != c.two_stage_passes {
            bail!(
                "artifact rule={} floor={} k={} records two_stage_passes={} but its own \
                 numbers contradict it (the letter says {recorded}) — refusing to pick \
                 from an inconsistent or tampered artifact",
                c.candidate.rule,
                c.candidate.floor,
                c.candidate.k,
                c.two_stage_passes
            );
        }
    }
    Ok(())
}

/// The pinned tie-break (GLM r1 minor-5): among PASSING cells — higher
/// hit@2, then lower FP, then rule (i) (the simpler path). Two DOCUMENTED
/// EXTENSIONS make the pinned order total (the spec pins only three keys):
/// full metric+rule ties prefer the stricter (higher) floor — the measured
/// letter is identical and a higher floor admits strictly fewer marginal
/// opens — then the smaller k. `two_stage_passes == false` cells are NEVER
/// eligible.
pub fn pick_winner(cells: &[CalibrationArtifact]) -> Result<CalibrationArtifact> {
    check_grid(cells)?;
    let rule_rank = |rule: &str| {
        if rule == "restricted-cosine" {
            0usize
        } else {
            1
        }
    };
    let mut passing: Vec<&CalibrationArtifact> =
        cells.iter().filter(|c| c.two_stage_passes).collect();
    if passing.is_empty() {
        let mut all: Vec<&CalibrationArtifact> = cells.iter().collect();
        all.sort_by(|a, b| {
            gated_hit(b)
                .partial_cmp(&gated_hit(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.two_stage.fp.cmp(&b.two_stage.fp))
        });
        let best = all[0];
        bail!(
            "no grid cell passes the acceptance letter (§7) — two-stage does NOT flip. \
             Best cell rule={} floor={} k={}: ts_{}={} ts_fp={} vs ss_{}={} ss_fp={}",
            best.candidate.rule,
            best.candidate.floor,
            best.candidate.k,
            letter_metric_of(best),
            gated_hit(best),
            best.two_stage.fp,
            letter_metric_of(best),
            baseline_hit(best),
            best.single_stage.fp
        );
    }
    passing.sort_by(|a, b| {
        gated_hit(b)
            .partial_cmp(&gated_hit(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.two_stage.fp.cmp(&b.two_stage.fp))
            .then_with(|| rule_rank(&a.candidate.rule).cmp(&rule_rank(&b.candidate.rule)))
            .then_with(|| {
                b.candidate
                    .floor
                    .partial_cmp(&a.candidate.floor)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.candidate.k.cmp(&b.candidate.k))
    });
    Ok(passing[0].clone())
}

/// The UNSCHEMED profile key the production k resolver looks rows up by
/// (`two_stage_k(gate_model_key(..))`, wisdom_match.rs:666 via
/// tools/src/queries.rs:839's `gate_model_key`): the v1 floor key minus the
/// scheme suffix `floor_key_for` composed onto it (only `instr1` appends
/// `:instr1`; raw keys are already byte-identical to `gate_model_key`).
/// I-1 (task-15-fix): keying the emitted k row by the SCHEMED baseline.key
/// left it unreachable from the resolver — an inert row, production silently
/// running k=8.
fn profile_key_for_k_row(baseline_key: &str, scheme: &str) -> String {
    match scheme {
        "instr1" => baseline_key
            .strip_suffix(":instr1")
            .unwrap_or(baseline_key)
            .to_string(),
        _ => baseline_key.to_string(),
    }
}

/// pr283-review finding 14: the gate key the BASELINE arm passes to
/// `wisdom_match_in_scheme` — the UNSCHEMED profile key. `--baseline-key`
/// is validated as `floor_key_for(profile_key, scheme)` (already schemed),
/// but `wisdom_match_in_scheme` applies `floor_key_for` AGAIN inside
/// `match_with_resolved_floor`; passing the schemed key produced
/// `…:instr1:instr1`, an unregistered key, a None floor, and a vacuous
/// 0/0 baseline arm. Same derivation rule as `profile_key_for_k_row`.
pub fn baseline_gate_key(schemed_baseline_key: &str, scheme: &Scheme) -> String {
    match scheme {
        Scheme::Instr1 => schemed_baseline_key
            .strip_suffix(":instr1")
            .unwrap_or(schemed_baseline_key)
            .to_string(),
        Scheme::Raw => schemed_baseline_key.to_string(),
    }
}

/// The ready-to-apply patch snippet for the winner (plan §Task 15
/// Interfaces): the `WISDOM_GATE_FLOORS` composite row, plus the
/// `TWO_STAGE_K` row under rule (ii) ONLY — rule (i) scores the whole
/// rule-reachable set, stage-1 breadth does not apply, k is inert.
pub fn patch_snippet(winner: &CalibrationArtifact) -> String {
    let key = composite_floor_key(winner);
    let mut out = format!(
        "// WISDOM_GATE_FLOORS (src-tauri/src/wisdom_match.rs) — add:\n    (\"{key}\", {}),\n",
        winner.candidate.floor
    );
    if winner.candidate.rule == "chunk-hop" {
        // I-1 (task-15-fix): the resolver looks k rows up by the UNSCHEMED
        // profile key, NOT the schemed baseline.key — emit the key it will
        // actually bind at, naming the scheme the k was measured under.
        let k_key = profile_key_for_k_row(&winner.baseline.key, &winner.baseline.scheme);
        out.push_str(&format!(
            "// TWO_STAGE_K (src-tauri/src/wisdom_match.rs) — add (rule (ii) only; \
             rule (i) never uses stage-1 breadth; k measured under scheme {:?}):\n    (\"{}\", {}),\n",
            winner.baseline.scheme, k_key, winner.candidate.k
        ));
    }
    out
}

/// The winning cell of a grid sweep, in reportable shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickWinner {
    pub rule: String,
    pub floor: f32,
    pub k: usize,
    /// Composite `WISDOM_GATE_FLOORS` key the flip adds (spec §3 shape).
    pub gate_key: String,
}

/// `pick`'s output: the winner, the sweep's shape, and the patch snippet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickReport {
    pub schema: u32,
    pub winner: PickWinner,
    pub cells_considered: usize,
    pub passing_cells: usize,
    /// The pinned tie-break order, recorded for the audit trail.
    pub tie_break: String,
    pub patch_snippet: String,
}

/// Sweep a grid and emit the winner report (§9 step 3: pick rule + floor;
/// the flip is a SEPARATE letter-gated step).
pub fn pick_report(cells: &[CalibrationArtifact]) -> Result<PickReport> {
    let winner = pick_winner(cells)?;
    Ok(PickReport {
        schema: 1,
        winner: PickWinner {
            rule: winner.candidate.rule.clone(),
            floor: winner.candidate.floor,
            k: winner.candidate.k,
            gate_key: composite_floor_key(&winner),
        },
        cells_considered: cells.len(),
        passing_cells: cells.iter().filter(|c| c.two_stage_passes).count(),
        tie_break: "higher hit@2 → lower FP → rule (i) (GLM r1 minor-5); \
                    full ties → stricter floor → smaller k (documented extension)"
            .to_string(),
        patch_snippet: patch_snippet(&winner),
    })
}

/// Every parseable `CalibrationArtifact` JSON in `dir` — one per grid cell.
/// Unparseable or non-artifact JSON files are SKIPPED with a stderr note
/// (the grid dir may legitimately hold the previous pick's report or the
/// flip package); zero parseable artifacts ⇒ refuse.
pub fn grid_cells(dir: &Path) -> Result<Vec<CalibrationArtifact>> {
    if !dir.is_dir() {
        bail!("grid dir {} does not exist", dir.display());
    }
    let mut cells = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("read grid dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    for path in entries {
        match std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(|raw| {
                serde_json::from_slice::<CalibrationArtifact>(&raw).map_err(anyhow::Error::from)
            }) {
            Ok(artifact) => cells.push(artifact),
            Err(e) => eprintln!(
                "note: skipping {} (not a calibration artifact: {e})",
                path.display()
            ),
        }
    }
    if cells.is_empty() {
        bail!(
            "no calibration artifacts in {} — run the paired driver once per \
             (rule × floor × k) cell first (§7)",
            dir.display()
        );
    }
    Ok(cells)
}

/// Load one artifact file (the `flip` entry point).
pub fn load_artifact(path: &Path) -> Result<CalibrationArtifact> {
    let raw = std::fs::read(path).map_err(|io_err| {
        // M-4 (task-15-fix): name the ACTUAL io error kind — a permission or
        // is-a-directory error is not "no such file".
        let detail = if io_err.kind() == std::io::ErrorKind::NotFound {
            "no such file".to_string()
        } else {
            format!("failed to read ({io_err})")
        };
        anyhow::anyhow!("read calibration artifact {} — {detail}", path.display())
    })?;
    serde_json::from_slice(&raw)
        .with_context(|| format!("parse calibration artifact {}", path.display()))
}

/// The flip decision output: everything the operator applies to flip the
/// two-stage default (§9 step 3: flip-to-default IF the letter passes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlipPlan {
    /// Composite `WISDOM_GATE_FLOORS` key (spec §3 shape).
    pub gate_key: String,
    pub rule: String,
    pub floor: f32,
    pub k: usize,
    /// Freeze dir relative to `src-tauri/tests/fixtures` (bench convention,
    /// plan §Task 6).
    pub freeze_relative_dir: String,
    pub patch_snippet: String,
    /// The freeze-dir `expected.json` content, derived from the artifact.
    pub expected_json: String,
}

/// The letter-gated flip decision (plan §Task 15): the flip proceeds ONLY on
/// a passing artifact current for THIS build. Refusals, in order: unknown
/// schema; unknown two-stage rule (M-2, task-15-fix — `flip` bypasses
/// `check_grid`, so `flip_plan` validates the rule name itself, same bail
/// wording); artifact without code provenance (unversioned build — cannot be
/// tied to a real calibration run); STALE artifact (produced by a different
/// build than the one asked to flip); recorded verdict contradicting the
/// artifact's own numbers; FAILING letter (prints the letter numbers, §7).
///
/// NOTE the honest scope: this returns the flip PACKAGE (freeze-dir
/// `expected.json` + patch snippet). It does not edit source: the shipped
/// tree stays UNFLIPPED until a real passing artifact exists, and
/// `replay_two_stage` cannot un-ignore on `expected.json` alone —
/// `vectors.json.gz` only comes from the freeze-producing live run.
pub fn flip_plan(artifact: &CalibrationArtifact, current_sha: &str) -> Result<FlipPlan> {
    if artifact.schema != 1 {
        bail!(
            "unknown artifact schema {} — refusing to flip (expected 1)",
            artifact.schema
        );
    }
    // M-2 (task-15-fix): `flip` loads the artifact and calls `flip_plan`
    // directly (cmd_flip), bypassing `check_grid` — so the rule vocabulary is
    // re-validated here, with `check_grid`'s bail wording.
    if artifact.candidate.rule != "restricted-cosine" && artifact.candidate.rule != "chunk-hop" {
        bail!(
            "unknown two-stage rule {:?} in the flip artifact",
            artifact.candidate.rule
        );
    }
    if artifact.code_sha == "unversioned" {
        bail!(
            "artifact lacks code provenance (built without CT_BUILD_SHA) — refusing to \
             flip an artifact that cannot be tied to a versioned calibration build"
        );
    }
    if current_sha == "unversioned" {
        bail!(
            "this build is unversioned (no CT_BUILD_SHA) — refusing to flip: cannot \
             verify artifact {} is current for this build",
            artifact.code_sha
        );
    }
    if artifact.code_sha != current_sha {
        bail!(
            "STALE artifact: produced by build {} but this is build {} — re-run the \
             paired calibration before flipping",
            artifact.code_sha,
            current_sha
        );
    }
    // Metric-aware letter recompute (issue #282 rev-7 M2): activity cells
    // gate on hit@1 (`letter_metric = Some("hit1")`), message cells keep the
    // #271 hit@2 letter — CodeRabbit PR-288 Major 1: a hit@1-gated artifact
    // must never be re-checked against the hit@2 letter here.
    let recorded = recorded_letter(artifact);
    if recorded != artifact.two_stage_passes {
        bail!(
            "artifact records two_stage_passes={} but its own numbers contradict it \
             (the {} letter says {recorded}) — refusing to flip",
            artifact.two_stage_passes,
            letter_metric_of(artifact)
        );
    }
    if !artifact.two_stage_passes {
        bail!(
            "the acceptance letter does NOT pass — refusing to flip (§7: not worse \
             than the baseline under the cell's own metric): ts_{}={} ts_fp={} vs \
             ss_{}={} ss_fp={}",
            letter_metric_of(artifact),
            gated_hit(artifact),
            artifact.two_stage.fp,
            letter_metric_of(artifact),
            baseline_hit(artifact),
            artifact.single_stage.fp
        );
    }
    let gate_key = composite_floor_key(artifact);
    let expected = serde_json::json!({
        "schema": 1,
        "gate_key": gate_key,
        "rule": artifact.candidate.rule,
        "floor": artifact.candidate.floor,
        "k": artifact.candidate.k,
        "probe_count": artifact.probes.count,
        "probes_sha256": artifact.probes.sha256,
        "baseline_key": artifact.baseline.key,
        "baseline_floor": artifact.baseline.floor,
        "source_artifact_code_sha": artifact.code_sha,
        "note": "Floor/rule/k derived from the paired calibration artifact. \
                 replay_two_stage additionally requires vectors.json.gz, which ONLY \
                 the freeze-producing live run writes; never hand-edit this file.",
    });
    Ok(FlipPlan {
        gate_key,
        rule: artifact.candidate.rule.clone(),
        floor: artifact.candidate.floor,
        k: artifact.candidate.k,
        freeze_relative_dir: freeze_relative_dir(&composite_floor_key(artifact)),
        patch_snippet: patch_snippet(artifact),
        expected_json: serde_json::to_string_pretty(&expected)?,
    })
}

/// Write the flip package into `out_dir`: the freeze-dir `expected.json` at
/// the bench-convention path and the ready-to-apply patch snippet beside it.
/// Returns the written paths.
pub fn write_flip_package(plan: &FlipPlan, out_dir: &Path) -> Result<Vec<PathBuf>> {
    let freeze_dir = out_dir.join(&plan.freeze_relative_dir);
    std::fs::create_dir_all(&freeze_dir)
        .with_context(|| format!("create freeze dir {}", freeze_dir.display()))?;
    let expected_path = freeze_dir.join("expected.json");
    std::fs::write(&expected_path, &plan.expected_json)
        .with_context(|| format!("write {}", expected_path.display()))?;
    let patch_path = out_dir.join("two-stage-flip.patch");
    std::fs::write(&patch_path, &plan.patch_snippet)
        .with_context(|| format!("write {}", patch_path.display()))?;
    Ok(vec![expected_path, patch_path])
}

// ── Live driver (not exercised offline; the live run is §9 step 3, manual) ──

fn parse_rule(s: &str) -> Result<TwoStageRule> {
    match s {
        "restricted-cosine" => Ok(TwoStageRule::RestrictedCosine),
        "chunk-hop" => Ok(TwoStageRule::ChunkHop),
        other => bail!("--rule must be `restricted-cosine` or `chunk-hop`, got {other:?}"),
    }
}

fn embed_all_timed(
    profile: &EmbedProfile,
    texts: Vec<String>,
) -> Result<(Vec<Vec<f32>>, Vec<f64>)> {
    let mut out = Vec::with_capacity(texts.len());
    // Per-message averages ONLY (task-14-fix M-2): the whole-batch total is
    // a different unit and must not mix into this sample — `EmbedStats`'s
    // doc promises "per-message averages of the batched embed calls", and
    // the artifact percentiles are computed from this vector verbatim.
    let mut per_message_ms: Vec<f64> = Vec::new();
    for chunk in texts.chunks(BATCH) {
        let t0 = Instant::now();
        let got = embed_batch(profile, chunk.to_vec())
            .context("live embed call failed — calibration needs real embeddings")?;
        if got.len() != chunk.len() {
            bail!(
                "embed backend returned {} vectors for {}",
                got.len(),
                chunk.len()
            );
        }
        // Per-message attribution of the batched call (r25 E6).
        let per = (t0.elapsed().as_secs_f64() * 1000.0) / chunk.len() as f64;
        out.extend(got);
        for _ in chunk {
            per_message_ms.push(per);
        }
    }
    Ok((out, per_message_ms))
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (((p / 100.0) * (values.len() as f64 - 1.0)).round()) as usize;
    values[idx.min(values.len() - 1)]
}

fn probe_hit(entries: &[WisdomItem], probe: &Probe) -> bool {
    entries
        .iter()
        .any(|e| probe.expect.iter().any(|id| id == &e.id))
}

fn verdict_of(entries: &[WisdomItem], open: bool, probe: &Probe) -> ProbeVerdict {
    ProbeVerdict {
        hit: probe_hit(entries, probe),
        hit1: entries
            .first()
            .is_some_and(|e| probe.expect.iter().any(|id| id == &e.id)),
        open: open && !entries.is_empty(),
        entries: entries.len(),
    }
}

/// The temporal shape of `llm_wiki_entries` (which of `superseded_by` /
/// `valid_to` exist). Mirrors the core's `temporal_columns` column probe —
/// the driver needs the shape to rebuild the shared eligibility predicate
/// for its driver-side rule-(ii) arm (see `eligibility_where`).
/// `pub` for the harness's parity tests (`calibrate_two_stage_harness.rs`,
/// task-14-fix I-1).
pub struct TemporalShape {
    pub has_superseded_by: bool,
    pub has_valid_to: bool,
}

pub fn temporal_shape(conn: &Connection) -> Result<TemporalShape> {
    let mut stmt = conn.prepare("PRAGMA table_info(llm_wiki_entries)")?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(TemporalShape {
        has_superseded_by: names.iter().any(|c| c == "superseded_by"),
        has_valid_to: names.iter().any(|c| c == "valid_to"),
    })
}

/// The shared eligibility predicate (spec §3.2), rebuilt for the driver-side
/// rule-(ii) arm. BYTE-EQUAL to the core's private
/// `wisdom_match::entry_eligibility_where` (same fragment `gated_entries`
/// interpolates): `deleted_at IS NULL`, non-null title/body/blob, the live
/// scheme filter, and the not-yet-in-effect supersession/expiry arms.
/// Keep in sync with the core — the parity tests in `wisdom_match.rs` are
/// the authority.
pub fn eligibility_where(shape: &TemporalShape, scheme: Scheme) -> String {
    let scheme_filter = tauri_app_lib::embed_scheme::scheme_filter_sql(Some(scheme));
    let mut sql = format!(
        "e.deleted_at IS NULL \
         AND e.title IS NOT NULL AND e.body IS NOT NULL \
         AND e.embedding_blob IS NOT NULL{scheme_filter}"
    );
    if shape.has_superseded_by {
        sql.push_str(" AND (e.superseded_by IS NULL OR e.superseded_at > :now)");
    }
    if shape.has_valid_to {
        sql.push_str(" AND (e.valid_to IS NULL OR e.valid_to > :now)");
    }
    sql
}

/// Score a filtered entry-id set exactly like the core's `gated_entries`:
/// RAW cosine against the stored fact vector (`embedding_blob`), optional
/// `raw < floor` drop, weighted-cosine (raw × tier weight) sort, truncate
/// to `max`. Rule (ii) passes `floor = None` (r25-M4 / r26 m2: NO
/// fact-level floor — the chunk-score floor is the ONLY abstention test).
///
/// Declared edge deviations from the core (task-14 review M-1; none can
/// alter hit@2/FP on a live OKF brain):
/// - NULL `entity_id` is kept here at tier weight 1.0
///   (`unwrap_or("")`), while the core's `CandidateRow` types it as
///   `String` and SKIPS such a row. Unreachable in production: `okf_ddl`
///   declares `entity_id TEXT NOT NULL`.
/// - A malformed row (bad types in the mapped columns) PROPAGATES the
///   error here, while the core skips it and continues. Stricter, not
///   looser — acceptable for a calibration harness.
/// - `text` is `truncate_text(&body)` here vs the core's raw `body`.
///   Cosmetic: verdicts compare entry IDS only (`probe_hit`).
#[allow(clippy::too_many_arguments)]
pub fn score_entries(
    conn: &Connection,
    query_vec: &[f32],
    floor: Option<f32>,
    max: usize,
    ids: &[String],
    shape: &TemporalShape,
    scheme: Scheme,
    now_ms: i64,
    provenance_filter: Option<&str>,
) -> Result<Vec<(f32, WisdomItem)>> {
    if ids.is_empty() || query_vec.is_empty() {
        return Ok(Vec::new());
    }
    let list = ids
        .iter()
        .map(|i| format!("'{i}'"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT e.id, e.entity_id, e.title, e.body, e.source_type, e.embedding_blob \
         FROM llm_wiki_entries e WHERE {} AND e.id IN ({list})",
        eligibility_where(shape, scheme),
    );
    let mut stmt = conn.prepare(&sql)?;
    let bind_now = shape.has_superseded_by || shape.has_valid_to;
    let map = |r: &rusqlite::Row<'_>| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Vec<u8>>(5)?,
        ))
    };
    let rows: Vec<_> = if bind_now {
        stmt.query_map(rusqlite::named_params! { ":now": now_ms }, map)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut scored: Vec<(f32, WisdomItem)> = Vec::new();
    for (id, entity_id, title, body, source_type, blob) in rows {
        let vec = bytes_to_f32(&blob);
        if vec.len() != query_vec.len() {
            continue; // dimension skew never fails the call (core rule)
        }
        let raw = cosine_similarity(query_vec, &vec);
        if let Some(f) = floor {
            if raw < f {
                continue; // the core's `raw < floor` drop
            }
        }
        // The delivery aperture (rev-6 M3): BEFORE the arm-internal
        // truncate — a top-N of filtered-out facts must never shadow a
        // `user_stated` fact ranked below it.
        if let Some(want) = provenance_filter {
            if provenance_for(source_type.as_deref()).as_deref() != Some(want) {
                continue;
            }
        }
        let weighted = raw * tier_weight(entity_id.as_deref().unwrap_or(""));
        scored.push((
            weighted,
            WisdomItem {
                id,
                title,
                text: truncate_text(&body).to_string(),
                score: Some(raw),
                supersedes: Vec::new(),
                provenance: provenance_for(source_type.as_deref()),
            },
        ));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    scored.truncate(max);
    Ok(scored)
}

/// Facts evidenced into one doc: the EVIDENCE-HASH edge (r30-M3, pinned) —
/// the same join the core's rule (ii) runs.
fn evidence_facts_of_doc(conn: &Connection, doc_id: i64) -> Result<Vec<String>> {
    let sql = "
        SELECT DISTINCT le.entry_id FROM librarian_evidence le
        JOIN json_each(le.evidence_json, '$.evidence') je
        JOIN chunks c ON c.content_hash = json_extract(je.value, '$.content_hash')
       WHERE c.doc_id = ?1";
    let ids: Vec<String> = conn
        .prepare(sql)?
        .query_map([doc_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// One rule-(ii) arm run (driver-side: the core's `two_stage_entries`
/// attributes rules by matching the floor against REGISTERED composite keys,
/// so an unregistered calibration floor can only reach rule-(i) mechanics —
/// the harness therefore drives the pub stage-1/edge primitives itself and
/// replicates the PINNED rule-(ii) mechanics: top-1 member chunk above the
/// floor (r30-m1), evidence-hash doc→facts edge (r30-M3), NO fact-level
/// floor (r25-M4/r26 m2), §3 v1-merge over the structural complement,
/// weighted-cosine re-rank, truncate to max.
struct ChunkHopRun {
    entries: Vec<WisdomItem>,
    v1merged: usize,
    merge_ms: f64,
}

#[allow(clippy::too_many_arguments)]
fn chunk_hop_arm(
    conn: &Connection,
    query_raw: &[f32],
    query_schemed: &[f32],
    scheme: Scheme,
    v1_floor: Option<f32>,
    ts_floor: f32,
    k: usize,
    max: usize,
    now_ms: i64,
    cands: &CandidateSet,
    shape: &TemporalShape,
    top_k_opening: bool,
    provenance_filter: Option<&str>,
) -> Result<ChunkHopRun> {
    // Stage 1 with the RAW vector, breadth = the calibrated k.
    let hits = stage1_search(conn, query_raw, cands, k)?;
    let members: HashSet<i64> = member_chunk_ids(conn, cands)?.into_iter().collect();
    let above: Vec<&tauri_app_lib::two_stage::Stage1Hit> = hits
        .iter()
        .filter(|h| members.contains(&h.chunk_id) && h.score >= ts_floor)
        .collect();
    // top-1 (pinned r30-m1) vs top-k opening (the r31 E5 ablation variant).
    let opened: Vec<&tauri_app_lib::two_stage::Stage1Hit> = if top_k_opening {
        above
    } else {
        above.into_iter().take(1).collect()
    };
    if opened.is_empty() {
        return Ok(ChunkHopRun {
            entries: Vec::new(),
            v1merged: 0,
            merge_ms: 0.0,
        });
    }
    // Facts of the opened doc(s): evidence-hash edge, deduped across docs.
    let mut fact_ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for h in &opened {
        for id in evidence_facts_of_doc(conn, h.doc_id)? {
            if seen.insert(id.clone()) {
                fact_ids.push(id);
            }
        }
    }
    // Rule-(ii) entries: NO fact-level floor (r25-M4/r26 m2).
    let rule_entries = score_entries(
        conn,
        query_schemed,
        None,
        max,
        &fact_ids,
        shape,
        scheme,
        now_ms,
        provenance_filter,
    )?;
    // §3 v1-merge over the STRUCTURAL complement, v1 floor, then ONE
    // weighted re-rank + truncate (r28-m2: schemed vector; r27-M1/r29-M1:
    // structural complement, never provenance).
    let t0 = Instant::now();
    let mut merged: Vec<(f32, WisdomItem)> = Vec::new();
    let mut v1merged = 0usize;
    if let Some(vf) = v1_floor {
        let complement = complement_entry_ids(conn, cands, "chunk-hop", now_ms, Some(scheme))?;
        let merge_scored = score_entries(
            conn,
            query_schemed,
            Some(vf),
            usize::MAX,
            &complement,
            shape,
            scheme,
            now_ms,
            provenance_filter,
        )?;
        v1merged = merge_scored.len();
        merged = merge_scored;
    }
    let merge_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut combined = rule_entries;
    combined.append(&mut merged);
    combined.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    combined.truncate(max);
    Ok(ChunkHopRun {
        entries: combined.into_iter().map(|(_, item)| item).collect(),
        v1merged,
        merge_ms,
    })
}

fn median(values: &mut [f64]) -> f64 {
    percentile(values, 50.0)
}

/// Code provenance for the artifact (task-14-fix M-3): the compile-time
/// `CT_BUILD_SHA` git rev, or `unversioned` when the build did not inject
/// one. Serde backward-compat is irrelevant — no artifacts exist in the
/// wild yet (task-14-fix brief).
fn code_sha() -> String {
    option_env!("CT_BUILD_SHA")
        .unwrap_or("unversioned")
        .to_string()
}

fn cmd_pick(grid: &Path, json_out: Option<&Path>) -> Result<()> {
    let cells = grid_cells(grid)?;
    let report = pick_report(&cells)?;
    println!(
        "picker: {} cells considered, {} pass the letter",
        report.cells_considered, report.passing_cells
    );
    println!(
        "winner: rule={} floor={} k={} → gate_key={}",
        report.winner.rule, report.winner.floor, report.winner.k, report.winner.gate_key
    );
    println!("tie-break: {}", report.tie_break);
    print!("{}", report.patch_snippet);
    if let Some(out) = json_out {
        std::fs::write(out, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("write pick report {}", out.display()))?;
        println!("report written to {}", out.display());
    }
    Ok(())
}

fn cmd_flip(artifact_path: &Path, out: Option<&Path>) -> Result<()> {
    let artifact = load_artifact(artifact_path)?;
    let plan = flip_plan(&artifact, &code_sha())?;
    // Default flip-package output: the artifact's own directory (Task 16's
    // runbook runs `flip --artifact artifact.json` in the grid dir).
    let out_dir = out
        .map(|p| p.to_path_buf())
        .or_else(|| artifact_path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let written = write_flip_package(&plan, &out_dir)?;
    println!("LETTER PASSES — flip package written:");
    for p in &written {
        println!("  {}", p.display());
    }
    print!("{}", plan.patch_snippet);
    println!(
        "next: apply the snippet, run the freeze-producing live calibration \
         (`calibrate_wisdom_gate --freeze`) so the freeze dir gains vectors.json.gz, \
         un-ignore replay_two_stage, run the gates — THEN commit the flip \
         (§9 step 3 / plan §Task 15). The shipped tree stays UNFLIPPED until then."
    );
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.cmd {
        Some(Cmd::Pick { grid, json_out }) => return cmd_pick(&grid, json_out.as_deref()),
        Some(Cmd::Flip { artifact, out }) => return cmd_flip(&artifact, out.as_deref()),
        None => {}
    }
    // Live-run path: the flat args are required here (clap can't express
    // "required unless subcommand" for grouped args, so main enforces it).
    fn missing(name: &str, v: &Option<impl Sized>) -> Option<String> {
        v.is_none().then(|| name.to_string())
    }
    let mut absent: Vec<String> = [
        missing("--probes", &args.probes),
        missing("--rule", &args.rule),
        missing("--floor", &args.floor),
        missing("--k", &args.k),
        missing("--baseline-key", &args.baseline_key),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !absent.is_empty() {
        absent.sort();
        bail!(
            "the following required arguments were not provided: {} \
             (the paired live run needs --probes/--rule/--floor/--k/--baseline-key; \
             `pick` and `flip` are subcommands)",
            absent.join(", ")
        );
    }
    let probes_path = args.probes.clone().expect("checked above");
    let rule = args.rule.clone().expect("checked above");
    let floor = args.floor.expect("checked above");
    let k = args.k.expect("checked above");
    let baseline_key = args.baseline_key.clone().expect("checked above");
    check_embed_stub(std::env::var_os("CURATED_EMBED_STUB").as_deref())?;
    // Issue #282 §7: the query kind (message default). Activity kinds imply
    // v1_floor=None, the user_stated aperture, user_stated-only positives,
    // and the hit@1 letter.
    let query_kind_raw = args
        .query_kind
        .clone()
        .unwrap_or_else(|| "message".to_string());
    let activity = query_kind_raw != "message";
    if !["message", "path", "command", "pattern"].contains(&query_kind_raw.as_str()) {
        bail!(
            "--query-kind must be one of message|path|command|pattern, got {query_kind_raw:?}"
        );
    }
    let kind = if activity {
        tauri_app_lib::wisdom_match::QueryKind::Activity
    } else {
        tauri_app_lib::wisdom_match::QueryKind::Message
    };
    let rule = parse_rule(&rule)?;
    if !floor.is_finite() || !(0.0..1.0).contains(&floor) {
        bail!("--floor must be a finite cosine floor in (0, 1), got {floor}",);
    }
    if k == 0 {
        bail!("--k must be >= 1");
    }
    let profile: EmbedProfile = serde_json::from_str(&args.profile).context("--profile")?;
    let profile_key = gate_model_key(&profile, None);
    let (probes, probes_sha) = read_jsonl::<Probe>(&probes_path)?;
    if probes.is_empty() {
        bail!("no probes in {}", probes_path.display());
    }
    let n_rel = probes.iter().filter(|p| !p.expect.is_empty()).count();
    let n_fp = probes.len() - n_rel;

    // LIVE brain, READ-ONLY (§9 step 3; the calibration never writes).
    let brain = resolve().context("resolve the live brain")?;
    let conn = open_ro(&brain)?;
    let scheme = read_scheme_for_reader(&conn)?;
    let scheme_key = floor_key_for(&profile_key, scheme);
    // Opus r1 M3: the baseline is the CURRENTLY SHIPPED default. A
    // --baseline-key that does not name the live cell would allow post-hoc
    // arm picking — refuse.
    if baseline_key != scheme_key {
        bail!(
            "--baseline-key {:?} does not name the LIVE single-stage key {:?} \
             (profile {profile_key} under scheme {}); the baseline must be the \
             currently shipped default, named before the run (Opus r1 M3)",
            baseline_key,
            scheme_key,
            scheme.as_str(),
        );
    }
    // rev-6 M3 / rev-8 m1: under activity the merge NEVER runs —
    // `v1_floor = None` at ALL THREE pass-sites (two_stage_entries +
    // chunk_hop_arm's two merge legs). The baseline floor itself is still
    // demanded for the message baseline arm.
    let message_v1_floor = gate_floor(&baseline_key).with_context(|| {
        format!(
            "--baseline-key {} has no registered floor in WISDOM_GATE_FLOORS",
            baseline_key
        )
    })?;
    let v1_floor = if activity { None } else { Some(message_v1_floor) };
    // rev-6 M3: activity positives are drawn from `user_stated` facts ONLY —
    // a non-`user_stated` positive reachable by baseline v1 but never by the
    // activity aperture would read as "the floor bet failed" (rev-7 M3/M2).
    if activity {
        let mut stmt = conn
            .prepare("SELECT source_type FROM llm_wiki_entries WHERE id = ?1")
            .context("preparing the provenance gate")?;
        for probe in &probes {
            for id in &probe.expect {
                let source: Option<String> = stmt
                    .query_row([id], |r| r.get(0))
                    .with_context(|| format!("positive fact {id} not found in llm_wiki_entries"))?;
                if provenance_for(source.as_deref()).as_deref() != Some("user_stated") {
                    bail!(
                        "probe {:?} expects fact {id} with provenance {:?} — activity runs \
                         calibrate the `user_stated` aperture (rev-6 M3): regenerate the \
                         probe set from `user_stated` facts only",
                        probe.text,
                        source
                    );
                }
            }
        }
    }


    // §9 step 3a / §6: coverage BEFORE any embed; < 100% ⇒ abort (Task 13
    // then Task 12) — the pure gate records the fraction for the artifact.
    let now = now_ms_at();
    let coverage = check_coverage(coverage(&conn, &profile_key, now)?)?;
    println!(
        "coverage: {}/{} candidate docs = 100% — proceeding with the paired run",
        coverage.covered, coverage.candidate_docs
    );

    // Embed probes (r26 m3): raw texts ALWAYS; schemed texts only under
    // instr1 (raw REUSES the single stage-1 embed — one embed per message).
    let raw_texts: Vec<String> = probes
        .iter()
        .map(|p| truncate_text(&p.text).to_string())
        .collect();
    let (raw_vecs, raw_ms) = embed_all_timed(&profile, raw_texts)?;
    let (schemed_vecs, schemed_ms, per_message) = if scheme == Scheme::Raw {
        (raw_vecs.clone(), Vec::new(), 1usize)
    } else {
        let schemed: Vec<String> = probes
            .iter()
            .map(|p| query_text_for_scheme(truncate_text(&p.text), scheme))
            .collect();
        let (v, ms) = embed_all_timed(&profile, schemed)?;
        (v, ms, 2usize)
    };
    let mut all_embed_ms = raw_ms;
    all_embed_ms.extend(&schemed_ms);
    let embed_p50 = percentile(&mut all_embed_ms.clone(), 50.0);
    let embed_p95 = percentile(&mut all_embed_ms, 95.0);

    // The candidate set + structural complement are call-independent (the
    // same for every probe at this now_ms).
    let cands = candidate_set(&conn, now, Some(scheme))?
        .context("candidate set unavailable between the coverage probe and the run")?;
    let complement: HashSet<String> =
        complement_entry_ids(&conn, &cands, rule.as_str(), now, Some(scheme))?
            .into_iter()
            .collect();
    let shape = temporal_shape(&conn)?;

    let labels: Vec<bool> = probes.iter().map(|p| !p.expect.is_empty()).collect();
    let mut ss_verdicts: Vec<ProbeVerdict> = Vec::with_capacity(probes.len());
    let mut ts_verdicts: Vec<ProbeVerdict> = Vec::with_capacity(probes.len());
    let mut ablation_verdicts: Vec<ProbeVerdict> = Vec::new();
    let mut ss_ms: Vec<f64> = Vec::new();
    let mut ts_ms: Vec<f64> = Vec::new();
    let mut merge_ms: Vec<f64> = Vec::new();
    let mut per_open_entries: Vec<f64> = Vec::new();
    let mut v1merged_total = 0usize;
    let mut opens_via_merge_only = 0usize;
    let mut hits_via_merge_only = 0usize;
    let mut stale_total = 0i64;

    // pr283-review finding 14: `wisdom_match_in_scheme` resolves the floor
    // as `floor_key_for(gate_key, scheme)` — pass the UNSCHEMED profile key
    // or an instr1 run double-suffixes to an unregistered key and measures
    // the letter against a vacuous 0/0 baseline.
    let ss_gate_key = baseline_gate_key(&baseline_key, &scheme);
    for (i, probe) in probes.iter().enumerate() {
        // ---- Arm 1: the SHIPPED single-stage default (baseline). ----
        let t0 = Instant::now();
        let ss = wisdom_match_in_scheme(
            &conn,
            &schemed_vecs[i],
            &ss_gate_key,
            scheme,
            HIT_K,
            &[],
            now,
        )?;
        ss_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        let open = !ss.entries.is_empty();
        ss_verdicts.push(verdict_of(&ss.entries, open, probe));

        // ---- Arm 2: the two-stage candidate, SAME run (§7 paired). ----
        let t1 = Instant::now();
        let (ts_entries, open, stale, merged): (Vec<WisdomItem>, bool, i64, usize) = match rule {
            TwoStageRule::RestrictedCosine => {
                // Explicit calibration floor ⇒ the core's attribution lands
                // on the unattributed tie-break (by design —
                // `two_stage_rule_floor` only knows REGISTERED keys); the
                // mechanics are rule (i)'s and the artifact carries the
                // real cell from --rule/--floor/--k.
                let (entries, outcome) = two_stage_entries(
                    &conn,
                    &raw_vecs[i],
                    &schemed_vecs[i],
                    scheme,
                    v1_floor,
                    Some(floor),
                    HIT_K,
                    &HashSet::new(),
                    &HashSet::new(),
                    now,
                    &profile_key,
                    kind,
                )?;
                (
                    entries,
                    outcome.open,
                    outcome.stale_embed_key,
                    outcome.v1merged,
                )
            }
            TwoStageRule::ChunkHop => {
                let run = chunk_hop_arm(
                    &conn,
                    &raw_vecs[i],
                    &schemed_vecs[i],
                    scheme,
                    v1_floor,
                    floor,
                    k,
                    HIT_K,
                    now,
                    &cands,
                    &shape,
                    false, // the arm opens TOP-1 (pinned r30-m1)
                    if activity { Some("user_stated") } else { None },
                )?;
                merge_ms.push(run.merge_ms);
                let open = !run.entries.is_empty();
                (run.entries, open, 0, run.v1merged) // coverage probed at 100% above
            }
        };
        ts_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
        stale_total += stale;
        v1merged_total += merged;
        if open {
            per_open_entries.push(ts_entries.len() as f64);
            // r27 E4: did this open come ONLY from the v1-merge?
            let only_merge =
                !ts_entries.is_empty() && ts_entries.iter().all(|e| complement.contains(&e.id));
            if only_merge && merged > 0 {
                opens_via_merge_only += 1;
            }
        }
        let verdict = verdict_of(&ts_entries, open, probe);
        if verdict.hit && ts_entries.iter().all(|e| complement.contains(&e.id)) {
            hits_via_merge_only += 1;
        }
        ts_verdicts.push(verdict);

        // ---- r31 E5 ablation: rule (ii) scored BOTH ways, same run. ----
        if rule == TwoStageRule::ChunkHop {
            let run = chunk_hop_arm(
                &conn,
                &raw_vecs[i],
                &schemed_vecs[i],
                scheme,
                v1_floor,
                floor,
                k,
                HIT_K,
                now,
                &cands,
                &shape,
                true, // top-k member-chunk opening
                if activity { Some("user_stated") } else { None },
            )?;
            ablation_verdicts.push(verdict_of(&run.entries, !run.entries.is_empty(), probe));
        }
    }

    // ---- Letter: message cells keep the #271 hit@2 letter; ACTIVITY cells
    // gate on hit@1 (issue #282 §7, rev-7 M2) — hit@2 is recorded, not
    // gated. ----
    let ss_metrics = arm_metrics(&ss_verdicts, &labels);
    let ts_metrics = arm_metrics(&ts_verdicts, &labels);
    let letter_metric = if activity { Some("hit1") } else { None };
    let (passes, letter) = if activity {
        (
            letter_verdict_hit1(
                ts_metrics.hit1,
                ts_metrics.fp,
                ss_metrics.hit1,
                ss_metrics.fp,
            ),
            "act_hit1 >= base_hit1 && act_fp <= base_fp".to_string(),
        )
    } else {
        (
            letter_verdict(
                ts_metrics.hit2,
                ts_metrics.fp,
                ss_metrics.hit2,
                ss_metrics.fp,
            ),
            "two_stage_passes = ts_hit2 >= ss_hit2 && ts_fp <= ss_fp".to_string(),
        )
    };

    // ---- Informational held-out splits (r24 m7/E5) over the SAME run. ----
    let splits = stratified_splits(&labels, HELDOUT_SPLITS, HELDOUT_SEED);
    let per_split: Vec<HeldOutSplit> = splits
        .iter()
        .map(|s| {
            let rel_idx: Vec<usize> = s.held_out.to_vec();
            let rel: Vec<bool> = rel_idx.iter().map(|&i| labels[i]).collect();
            let ss_v: Vec<ProbeVerdict> = rel_idx.iter().map(|&i| ss_verdicts[i].clone()).collect();
            let ts_v: Vec<ProbeVerdict> = rel_idx.iter().map(|&i| ts_verdicts[i].clone()).collect();
            HeldOutSplit {
                relevant: rel.iter().filter(|&&r| r).count(),
                fp: rel.iter().filter(|&&r| !r).count(),
                single_stage_hit2: arm_metrics(&ss_v, &rel).hit2,
                two_stage_hit2: arm_metrics(&ts_v, &rel).hit2,
            }
        })
        .collect();

    let mut open_lat = ts_ms.clone();
    let v1_latency_p95 = if merge_ms.is_empty() {
        // Core path (`two_stage_entries`) exposes no merge-segment timing —
        // record the combined-call p95 and say so (r31 E7 honesty note).
        percentile(&mut open_lat, 95.0)
    } else {
        percentile(&mut merge_ms.clone(), 95.0)
    };

    let ablation = if rule == TwoStageRule::ChunkHop {
        Some(TopKAblation {
            k,
            top1: ts_metrics.clone(),
            topk: arm_metrics(&ablation_verdicts, &labels),
        })
    } else {
        None
    };

    let artifact = CalibrationArtifact {
        schema: 1,
        code_sha: code_sha(),
        generated_utc: chrono::Utc::now().to_rfc3339(),
        probes: ProbeSetInfo {
            path: probes_path.display().to_string(),
            sha256: probes_sha,
            count: probes.len(),
            relevant: n_rel,
            fp: n_fp,
        },
        baseline: BaselineArm {
            key: baseline_key.clone(),
            floor: message_v1_floor,
            scheme: scheme.as_str().to_string(),
        },
        candidate: CandidateCell {
            rule: rule.as_str().to_string(),
            floor,
            k,
        },
        grid: GridInfo {
            cells: 1,
            rule: rule.as_str().to_string(),
            floor,
            k,
        },
        coverage,
        single_stage: ss_metrics.clone(),
        two_stage: ts_metrics.clone(),
        two_stage_passes: passes,
        letter: letter.clone(),
        letter_metric: letter_metric.map(str::to_string),
        query_kind: query_kind_raw.clone(),
        heldout: HeldOutInfo {
            splits: HELDOUT_SPLITS,
            half_size: probes.len() / 2,
            seed: HELDOUT_SEED,
            per_split,
        },
        embeds: EmbedStats {
            per_message,
            scheme: scheme.as_str().to_string(),
            latency_ms_p50: embed_p50,
            latency_ms_p95: embed_p95,
        },
        injection: InjectionStats {
            rule: rule.as_str().to_string(),
            opens: per_open_entries.len(),
            entries_per_open_median: median(&mut per_open_entries),
        },
        merge_attribution: MergeAttribution {
            opens_via_merge_only,
            hits_via_merge_only,
            v1merged_total,
        },
        v1_merge: V1MergeStats {
            latency_ms_p95: v1_latency_p95,
            volume_total: v1merged_total,
            measured: if merge_ms.is_empty() {
                "combined-call".to_string()
            } else {
                "merge-segment".to_string()
            },
        },
        ablation,
        stale_embed_key_total: stale_total,
    };

    let json = serde_json::to_string_pretty(&artifact).context("serialize artifact")?;
    match &args.json_out {
        Some(path) => {
            std::fs::write(path, &json)
                .with_context(|| format!("write artifact {}", path.display()))?;
            println!("artifact: {}", path.display());
        }
        None => println!("{json}"),
    }

    println!(
        "paired run: single-stage hit@2 {:.3} FP {}, two-stage ({} @ {}) hit@2 {:.3} FP {}",
        ss_metrics.hit2,
        ss_metrics.fp,
        rule.as_str(),
        floor,
        ts_metrics.hit2,
        ts_metrics.fp
    );
    println!("acceptance letter: two_stage_passes = {passes}");
    if passes {
        println!("LETTER PASSES — flip-to-default is a Task 15 decision on this artifact");
    } else {
        println!("letter does not pass — the shipped default stays");
    }
    Ok(())
}
