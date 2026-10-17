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

use anyhow::{bail, Context, Result};
use clap::Parser;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::PathBuf;
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
    /// Probe JSONL in the EXISTING `calibrate_wisdom_gate` format (serde
    /// `Probe`: `text`, `expect` = relevant-fact ids, optional `vector`),
    /// one line per probe. Do NOT invent a new schema.
    #[arg(long)]
    probes: PathBuf,
    /// The calibrated two-stage rule: `restricted-cosine` (rule i) or
    /// `chunk-hop` (rule ii).
    #[arg(long)]
    rule: String,
    /// The candidate two-stage floor (the rule's OWN decision variable:
    /// restricted fact cosine for rule i, member-CHUNK score for rule ii).
    #[arg(long)]
    floor: f32,
    /// Stage-1 chunk breadth for rule (ii) (the `TWO_STAGE_K` knob, default
    /// 8 — k is NOT a floor key). Ignored under `restricted-cosine` (rule i
    /// scores the whole rule-reachable set; stage-1 breadth does not apply).
    #[arg(long)]
    k: usize,
    /// The v1 baseline's model/scheme key — must equal the LIVE
    /// `floor_key_for(gate_model_key(profile), scheme)` key (Opus r1 M3:
    /// baseline named BEFORE the run, no post-hoc arm picking).
    #[arg(long)]
    baseline_key: String,
    /// Write the artifact JSON here (default: stdout).
    #[arg(long)]
    json_out: Option<PathBuf>,
    /// EmbedProfile JSON, as in `calibrate_wisdom_gate` (production default:
    /// OpenRouter `qwen/qwen3-embedding-4b`).
    #[arg(
        long,
        default_value = r#"{"type":"external","base_url":"https://openrouter.ai/api/v1","model":"qwen/qwen3-embedding-4b"}"#
    )]
    profile: String,
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
    /// Gate-OPEN decision (r30-m5: the FINAL entries list is non-empty).
    pub open: bool,
    pub entries: usize,
}

/// hit@2 / FP / opens / entries-per-open for one arm over one probe subset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmMetrics {
    /// hits / relevant probes (steps of 1/40 = 0.025 on the live set, §7).
    pub hit2: f64,
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
    ArmMetrics {
        hit2: if n_rel == 0 {
            0.0
        } else {
            hits as f64 / n_rel as f64
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
/// top-1 vs top-k ablation columns (r31 E5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationArtifact {
    pub schema: u32,
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
    pub heldout: HeldOutInfo,
    pub embeds: EmbedStats,
    pub injection: InjectionStats,
    pub merge_attribution: MergeAttribution,
    pub v1_merge: V1MergeStats,
    pub ablation: Option<TopKAblation>,
    pub stale_embed_key_total: i64,
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
    let mut batch_ms: Vec<f64> = Vec::new();
    for chunk in texts.chunks(BATCH) {
        let t0 = Instant::now();
        let got = embed_batch(profile, chunk.to_vec())
            .context("live embed call failed — calibration needs real embeddings")?;
        batch_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
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
            batch_ms.push(per);
        }
    }
    Ok((out, batch_ms))
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
        open: open && !entries.is_empty(),
        entries: entries.len(),
    }
}

/// The temporal shape of `llm_wiki_entries` (which of `superseded_by` /
/// `valid_to` exist). Mirrors the core's `temporal_columns` column probe —
/// the driver needs the shape to rebuild the shared eligibility predicate
/// for its driver-side rule-(ii) arm (see `eligibility_where`).
struct TemporalShape {
    has_superseded_by: bool,
    has_valid_to: bool,
}

fn temporal_shape(conn: &Connection) -> Result<TemporalShape> {
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
fn eligibility_where(shape: &TemporalShape, scheme: Scheme) -> String {
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
#[allow(clippy::too_many_arguments)]
fn score_entries(
    conn: &Connection,
    query_vec: &[f32],
    floor: Option<f32>,
    max: usize,
    ids: &[String],
    shape: &TemporalShape,
    scheme: Scheme,
    now_ms: i64,
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

fn main() -> Result<()> {
    let args = Args::parse();
    check_embed_stub(std::env::var_os("CURATED_EMBED_STUB").as_deref())?;
    let rule = parse_rule(&args.rule)?;
    if !args.floor.is_finite() || !(0.0..1.0).contains(&args.floor) {
        bail!(
            "--floor must be a finite cosine floor in (0, 1), got {}",
            args.floor
        );
    }
    if args.k == 0 {
        bail!("--k must be >= 1");
    }
    let profile: EmbedProfile = serde_json::from_str(&args.profile).context("--profile")?;
    let profile_key = gate_model_key(&profile, None);
    let (probes, probes_sha) = read_jsonl::<Probe>(&args.probes)?;
    if probes.is_empty() {
        bail!("no probes in {}", args.probes.display());
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
    if args.baseline_key != scheme_key {
        bail!(
            "--baseline-key {:?} does not name the LIVE single-stage key {:?} \
             (profile {profile_key} under scheme {}); the baseline must be the \
             currently shipped default, named before the run (Opus r1 M3)",
            args.baseline_key,
            scheme_key,
            scheme.as_str(),
        );
    }
    let v1_floor = gate_floor(&args.baseline_key).with_context(|| {
        format!(
            "--baseline-key {} has no registered floor in WISDOM_GATE_FLOORS",
            args.baseline_key
        )
    })?;

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

    for (i, probe) in probes.iter().enumerate() {
        // ---- Arm 1: the SHIPPED single-stage default (baseline). ----
        let t0 = Instant::now();
        let ss = wisdom_match_in_scheme(
            &conn,
            &schemed_vecs[i],
            &args.baseline_key,
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
                    Some(v1_floor),
                    Some(args.floor),
                    HIT_K,
                    &HashSet::new(),
                    &HashSet::new(),
                    now,
                    &profile_key,
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
                    Some(v1_floor),
                    args.floor,
                    args.k,
                    HIT_K,
                    now,
                    &cands,
                    &shape,
                    false, // the arm opens TOP-1 (pinned r30-m1)
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
                Some(v1_floor),
                args.floor,
                args.k,
                HIT_K,
                now,
                &cands,
                &shape,
                true, // top-k member-chunk opening
            )?;
            ablation_verdicts.push(verdict_of(&run.entries, !run.entries.is_empty(), probe));
        }
    }

    // ---- Letter (§7, pinned): not worse on BOTH axes. ----
    let ss_metrics = arm_metrics(&ss_verdicts, &labels);
    let ts_metrics = arm_metrics(&ts_verdicts, &labels);
    let passes = letter_verdict(
        ts_metrics.hit2,
        ts_metrics.fp,
        ss_metrics.hit2,
        ss_metrics.fp,
    );

    // ---- Informational held-out splits (r24 m7/E5) over the SAME run. ----
    let splits = stratified_splits(&labels, HELDOUT_SPLITS, HELDOUT_SEED);
    let per_split: Vec<HeldOutSplit> = splits
        .iter()
        .map(|s| {
            let rel_idx: Vec<usize> = s.held_out.iter().copied().collect();
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
            k: args.k,
            top1: ts_metrics.clone(),
            topk: arm_metrics(&ablation_verdicts, &labels),
        })
    } else {
        None
    };

    let artifact = CalibrationArtifact {
        schema: 1,
        generated_utc: chrono::Utc::now().to_rfc3339(),
        probes: ProbeSetInfo {
            path: args.probes.display().to_string(),
            sha256: probes_sha,
            count: probes.len(),
            relevant: n_rel,
            fp: n_fp,
        },
        baseline: BaselineArm {
            key: args.baseline_key.clone(),
            floor: v1_floor,
            scheme: scheme.as_str().to_string(),
        },
        candidate: CandidateCell {
            rule: rule.as_str().to_string(),
            floor: args.floor,
            k: args.k,
        },
        grid: GridInfo {
            cells: 1,
            rule: rule.as_str().to_string(),
            floor: args.floor,
            k: args.k,
        },
        coverage,
        single_stage: ss_metrics.clone(),
        two_stage: ts_metrics.clone(),
        two_stage_passes: passes,
        letter: "two_stage_passes = ts_hit2 >= ss_hit2 && ts_fp <= ss_fp".to_string(),
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
        args.floor,
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
