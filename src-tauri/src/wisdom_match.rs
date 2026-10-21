//! `ct wisdom match` core (issue #265): relevance-gated, read-only match of
//! curated wisdom facts against a message.
//!
//! Spec: docs/superpowers/specs/2026-10-06-issue265-wisdom-match-design.md.
//! Consumer: curated-thoughts-integrations live wisdom delivery.
//!
//! - **Gate:** raw cosine against a per-embed-model floor
//!   (`WISDOM_GATE_FLOORS`, CT INTENT rule 7). A model without a floor
//!   abstains (`gate = "uncalibrated"`, no entries). The floor never sees the
//!   tier weight — provenance is not a hidden ranking penalty.
//! - **Current-only:** superseded (`superseded_by`) and expired (`valid_to`)
//!   rows never match; on a pre-V24 table those filters are omitted.
//! - **Corrections:** each excluded id that has been superseded resolves to
//!   the live head of its `superseded_by` chain.
//! - **One id, one list:** a correction head never also appears in entries.
//!
//! Read-only: SELECTs only, bound parameters only.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::embed_scheme::{floor_key_for, resolve_reader_scheme, Scheme};
use crate::embedder::{CloudProvider, EmbedProfile};
use crate::search::{bytes_to_f32, cosine_similarity};
use crate::wiki_graph::tier_weight;

/// Output schema version (`"schema"` in the JSON contract).
pub const SCHEMA_VERSION: u32 = 1;
/// `gate` value when the embed model has no calibrated floor.
pub const GATE_UNCALIBRATED: &str = "uncalibrated";
/// Two-stage stdout/stdin gate-label prefix (issue #271 §5): the active
/// two-stage gate prints `semantic-v2-two-stage:{key}` where `{key}` is the
/// scheme-suffixed floor key (`floor_key_for(gate_model_key, scheme)`).
pub const TWO_STAGE_LABEL_PREFIX: &str = "semantic-v2-two-stage:";
/// `superseded_by` chain bound; mirrors core-llm-wiki `HISTORY_MAX_DEPTH`.
pub const MAX_CHAIN_DEPTH: usize = 100;
/// `--max` is clamped to this.
pub const MAX_ENTRIES: usize = 10;
/// More `--exclude` values than this is a usage error.
pub const MAX_EXCLUDES: usize = 1024;
/// `<text>` is truncated to this many chars before embedding.
pub const MAX_TEXT_CHARS: usize = 2000;
/// CT's provenance vocabulary: stored `source_type` values emitted verbatim.
pub const PROVENANCE_VOCAB: &[&str] = &[
    "librarian_inferred",
    "user_stated",
    "user_confirmed",
    "immutable_document",
];

/// Which shape of query text this call carries (issue #282): a user message
/// (prose) or an activity target (`--path=`/`--command=`/`--pattern=` — a
/// bare path, command or search pattern). The kind selects the ACTIVITY
/// floor-key suffix — ONE shared `:activity` floor across all three target
/// kinds (Kurt ruling 2026-10-18) — and, on the active path, the human-tier
/// delivery aperture (`user_stated` + `user_confirmed`; §5 rev-15 of the
/// #282 spec). Message mode is
/// byte-for-byte untouched: `QueryKind::Message` reproduces today's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Message,
    Activity,
}

impl QueryKind {
    /// The floor-key BASE: the profile key for message queries, the profile
    /// key + `:activity` for activity queries. Composite suffixes
    /// (`:two-stage:<rule>`) land AFTER this base — never at the composite's
    /// end (spec rev-6 B1: resolver and calibration table must never drift).
    /// Site-4 note (rev-10 m1): callers pass this base as `gate_key`, so BOTH
    /// arms of `match_with_resolved_floor` (schemed and scheme-`None`) carry
    /// the suffix; `QueryKind::Activity` never resolves a floor there.
    pub fn floor_base_key(self, profile_key: &str) -> String {
        match self {
            QueryKind::Message => profile_key.to_string(),
            QueryKind::Activity => format!("{profile_key}:activity"),
        }
    }
}

/// Abstention floors, one per embedding model (CT INTENT rule 7). RAW-cosine
/// keys (`<model-key>[:scheme]`) abstain on the entry's RAW cosine; a
/// `…:two-stage:chunk-hop` key abstains on a CHUNK score instead — never on
/// raw cosine (spec §3, Opus r5 MINOR-4). Values come only from a
/// `calibrate_wisdom_gate` run recorded under docs/benchmarks/. A model not
/// listed here abstains.
pub const WISDOM_GATE_FLOORS: &[(&str, f32)] = &[
    // Test-only keys: reachable only with CURATED_EMBED_STUB=constant8. The
    // stub's vectors are scheme-independent, so it carries the same floor
    // under both schemes — otherwise stub-backed tests and dev runs would
    // silently flip to abstention at cutover.
    ("stub:constant8", 0.5),
    ("stub:constant8:instr1", 0.5),
    // Calibrated 2026-10-07 (OpenRouter, GLM-5.3-flash probes): hit@2 0.44, FP 0.05
    // — docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md
    ("external:qwen/qwen3-embedding-4b", 0.70),
    // instr1 scheme (both-side instruction prefix, spec-rev2 cell E): hit@2 0.57 at
    // this floor — docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md.
    // Key form: <raw gate key>:instr1 (see embed_scheme::floor_key_for).
    ("external:qwen/qwen3-embedding-4b:instr1", 0.64),
    // Two-stage composite key (spec §3, `<raw>:two-stage:<rule>`): calibrated
    // 2026-10-18 on the live brain — chunk-hop @ 0.65, k=8: hit@2 0.143 (= v1
    // baseline), FP 0 vs baseline 4, opens 1 vs baseline 5. Letter per #271 §7
    // passed; campaign `calibration-run/grid-20261018-100351/` (14 cells, build
    // 05dfe7bb) — docs/benchmarks/2026-10-18-wisdom-gate-two-stage-chunk-hop.md.
    ("external:qwen/qwen3-embedding-4b:two-stage:chunk-hop", 0.65),
    // Activity composite key (issue #282, spec §3 `<raw>:activity:two-stage:<rule>`;
    // ONE shared `:activity` floor across all three kinds — Kurt ruling 2026-10-18):
    // calibrated 2026-10-18 on the Kurt-approved scratch COPY of the live brain (14
    // facts relabeled user_stated in the copy only; 12–18 positives per kind — SMALL
    // SAMPLES, scratch-label stand-in pending Option 2 app attestations).
    // restricted-cosine @ 0.65, k=8
    // (TWO_STAGE_K default): hit@1 0.278/0.250/0.250 vs v1-baseline 0.111/0.083/0.167
    // (path/command/pattern), FP 0 everywhere vs 1. Letter per #282 §7 passed in every
    // kind; campaign `calibration-run/grid-20261018-activity/` (24 recorded cells +
    // 3 winning cells re-run at build bad0cdbb91f3) —
    // docs/benchmarks/2026-10-18-wisdom-gate-activity-restricted-cosine.md.
    // Provenance note (Tier-1.5 r4): the freeze ran via a TEMPORARY, disclosed
    // gate-disable of calibrate_wisdom_gate's fixture-size gate (reverted after
    // the freeze — see freeze_provenance in the fixture's expected.json); the
    // floor registration itself is this constant, unaffected by tooling. Flip
    // go (covering the scratch-label stand-in AND the calibration methodology)
    // approved by Kurt 2026-10-19.
    (
        "external:qwen/qwen3-embedding-4b:activity:two-stage:restricted-cosine",
        0.65,
    ),
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WisdomItem {
    pub id: String,
    pub title: String,
    pub text: String,
    /// Raw cosine for entries; `null` for corrections.
    pub score: Option<f32>,
    pub supersedes: Vec<String>,
    pub provenance: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WisdomMatch {
    pub schema: u32,
    pub gate: String,
    pub entries: Vec<WisdomItem>,
    pub corrections: Vec<WisdomItem>,
}

/// Floor for a gate key, if calibrated.
pub fn gate_floor(key: &str) -> Option<f32> {
    WISDOM_GATE_FLOORS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, floor)| *floor)
}

#[cfg(test)]
/// Test-only two-stage composite floors (spec §3 floor storage): same shape
/// the calibration flip will add per production model — distinct values so
/// the stub-backed tests can attribute a rule by the floor's composite key
/// (rule attribution reads which composite key's floor matches). These live
/// OUTSIDE [`WISDOM_GATE_FLOORS`] so a stub-backed `--two-stage` run can
/// never present a manufactured floor as calibrated (§4 cell 1 fallback must
/// fire); integration binaries compile this file without `cfg(test)`, and
/// the Task-15 harness passes explicit floors during calibration sweeps by
/// design (it never resolves floors via [`gate_floor`]).
const WISDOM_GATE_FLOORS_TEST: &[(&str, f32)] = &[
    // The stub's vectors are scheme-independent, so it carries the same
    // floor under both schemes.
    ("stub:constant8:two-stage:restricted-cosine", 0.5),
    ("stub:constant8:two-stage:chunk-hop", 0.45),
    ("stub:constant8:instr1:two-stage:restricted-cosine", 0.5),
    ("stub:constant8:instr1:two-stage:chunk-hop", 0.45),
    // Issue #282: the ACTIVITY composite (rev-3 M4 — test-only, like its
    // message twin). The value is deliberately DIFFERENT from the message
    // stub's 0.45 so the attribution test exercises value-equality
    // attribution instead of passing by coincidence (rev-6 B1).
    ("stub:constant8:activity:two-stage:chunk-hop", 0.40),
];

/// cfg(test) floor lookup: [`gate_floor`] over the production table, plus
/// the test-only composite slice above. Production builds resolve ONLY the
/// production table (this fn is compiled out), so the §4 cell-1
/// never-calibrated fallback fires in stub-backed `--two-stage` runs.
#[cfg(test)]
fn gate_floor_with_test_keys(key: &str) -> Option<f32> {
    gate_floor(key).or_else(|| {
        WISDOM_GATE_FLOORS_TEST
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, floor)| *floor)
    })
}

/// cfg-split floor lookup for the two-stage rule attribution: production
/// builds resolve ONLY [`gate_floor`] (the production table), so a
/// stub-backed `--two-stage` run finds no composite floor and the §4 cell-1
/// fallback fires; test builds additionally see [`WISDOM_GATE_FLOORS_TEST`]
/// so rule attribution works for stub-backed unit tests.
#[cfg(test)]
fn two_stage_rule_floor(key: &str) -> Option<f32> {
    gate_floor_with_test_keys(key)
}

#[cfg(not(test))]
fn two_stage_rule_floor(key: &str) -> Option<f32> {
    gate_floor(key)
}

fn cloud_provider_name(provider: &CloudProvider) -> &'static str {
    match provider {
        CloudProvider::OpenAi => "open_ai",
        CloudProvider::Voyage => "voyage",
        CloudProvider::Cohere => "cohere",
    }
}

/// Gate key for an embed profile. `stub` is the `CURATED_EMBED_STUB` value,
/// passed in by the caller: the stub replaces embeddings whatever the profile
/// says, so it wins.
pub fn gate_model_key(profile: &EmbedProfile, stub: Option<&str>) -> String {
    if let Some(stub) = stub.filter(|s| !s.is_empty()) {
        return format!("stub:{}", stub.to_lowercase());
    }
    match profile {
        EmbedProfile::Local { model } => format!("local:{}", model.to_lowercase()),
        EmbedProfile::Cloud {
            provider, model, ..
        } => format!(
            "cloud:{}:{}",
            cloud_provider_name(provider),
            model.to_lowercase()
        ),
        EmbedProfile::External { profile } => {
            format!("external:{}", profile.model.to_lowercase())
        }
    }
}

/// Gate key under an active read scheme: the raw key, with `:instr1` appended
/// when the read scheme is `instr1` (spec: the floor key is scheme-defining).
/// Raw keys are byte-identical to `gate_model_key`, so pre-#265 behavior is
/// unchanged until the cutover flips `wisdom_active_scheme`.
pub fn gate_model_key_for_scheme(
    profile: &EmbedProfile,
    stub: Option<&str>,
    scheme: Scheme,
) -> String {
    floor_key_for(&gate_model_key(profile, stub), scheme)
}

/// True for an id usable with `--exclude`: `^[A-Za-z0-9._:-]{1,128}$`.
pub fn valid_fact_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// `source_type` → provenance: verbatim when in the vocabulary, else `None`.
pub fn provenance_for(source_type: Option<&str>) -> Option<String> {
    source_type
        .filter(|s| PROVENANCE_VOCAB.contains(s))
        .map(str::to_string)
}

/// First `MAX_TEXT_CHARS` chars of `text`, cut on a char boundary.
pub fn truncate_text(text: &str) -> &str {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// Probed table shape + the call's ONE scheme resolution (`pub(crate)` so
/// Task 3's `two_stage` consumers can hold the same value the gate resolved —
/// the shared predicate and its consumers all take `&Temporal`).
pub(crate) struct Temporal {
// `pub(crate)` for the confirmation queue (user-confirmation-queue plan
// Task 4): db::queue reuses the SAME probe + predicate fragments instead
// of re-probing. `scheme` stays private — delivery-only concern.
pub(crate) has_superseded_by: bool,
pub(crate) has_valid_to: bool,
    /// Read scheme for the SELECT filter — `None` on the pre-V27 shape (no
    /// `embed_scheme` column). Resolved once per call, next to the column
    /// probes, so every scheme-derived tuple member of the call path (floor,
    /// row filter, and — on the production path — the query prefix) shares
    /// one resolution.
    scheme: Option<Scheme>,
}

/// Column probes plus the call's ONE scheme resolution. `declared` is a
/// scheme the caller already resolved (and derived its query text / floor
/// from); `None` resolves the active `wisdom_active_scheme` here.
// Task 3: exposed for two_stage stage-1 SQL (plan §Task 3) — two_stage holds
// the SAME Temporal the gate resolved, so its candidate/complement legs share
// one scheme + table-shape resolution with the gate.
pub(crate) fn temporal_columns(conn: &Connection, declared: Option<Scheme>) -> Result<Temporal> {
    // The scheme rule (fail-closed on unknown values, pre-V27 degradation,
    // declared-instr1-on-pre-V27 refusal) lives once in `embed_scheme`.
    let scheme = resolve_reader_scheme(conn, declared)?;
    let cols = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    Ok(Temporal {
        has_superseded_by: cols.iter().any(|c| c == "superseded_by"),
        has_valid_to: cols.iter().any(|c| c == "valid_to"),
        scheme,
    })
}

/// Match with the calibrated floor for `gate_key` (none → abstain), under the
/// ACTIVE read scheme resolved here. Use [`wisdom_match_in_scheme`] instead
/// when the caller already resolved the scheme to build its query text.
pub fn wisdom_match(
    conn: &Connection,
    query_vec: &[f32],
    gate_key: &str,
    max: usize,
    exclude: &[String],
    now_ms: i64,
) -> Result<WisdomMatch> {
    let temporal = temporal_columns(conn, None)?;
    match_with_resolved_floor(conn, query_vec, gate_key, max, exclude, now_ms, temporal)
}

/// [`wisdom_match`] under a scheme the CALLER resolved — the one it derived
/// its query text from (`embed_scheme::query_text_for_scheme`). The floor and
/// the SELECT filter both follow `scheme`, so a `wisdom_active_scheme` flip
/// between the caller's resolution and this call can never pair the query
/// embedded under one scheme with another scheme's rows or floor. `scheme`
/// comes from `embed_scheme::read_scheme_for_reader` (raw on pre-V27).
pub fn wisdom_match_in_scheme(
    conn: &Connection,
    query_vec: &[f32],
    gate_key: &str,
    scheme: Scheme,
    max: usize,
    exclude: &[String],
    now_ms: i64,
) -> Result<WisdomMatch> {
    let temporal = temporal_columns(conn, Some(scheme))?;
    match_with_resolved_floor(conn, query_vec, gate_key, max, exclude, now_ms, temporal)
}

/// ONE `Temporal` for the whole call: its single scheme drives BOTH the floor
/// (here) and the SELECT filter (inside `wisdom_match_impl`). The pre-V27
/// shape (scheme None) keeps the unsuffixed raw key and the filter omitted;
/// an unregistered (model, scheme) key means abstain (floor None).
fn match_with_resolved_floor(
    conn: &Connection,
    query_vec: &[f32],
    gate_key: &str,
    max: usize,
    exclude: &[String],
    now_ms: i64,
    temporal: Temporal,
) -> Result<WisdomMatch> {
    let floor = match temporal.scheme {
        Some(scheme) => gate_floor(&floor_key_for(gate_key, scheme)),
        None => gate_floor(gate_key),
    };
    wisdom_match_impl(
        conn, query_vec, gate_key, floor, max, exclude, now_ms, temporal,
    )
}

/// `wisdom_match` with an explicit floor (tests and calibration). The caller
/// controls the floor AND declares the `scheme` its floor and `query_vec`
/// belong to; the SELECT filter follows that declared scheme — never the
/// active meta value — so floor, query and candidate rows always share one
/// scheme. `floor = None` abstains exactly like an uncalibrated model. An
/// empty `query_vec` yields no entries (corrections still flow).
#[allow(clippy::too_many_arguments)]
pub fn wisdom_match_with_floor(
    conn: &Connection,
    query_vec: &[f32],
    gate_key: &str,
    scheme: Scheme,
    floor: Option<f32>,
    max: usize,
    exclude: &[String],
    now_ms: i64,
) -> Result<WisdomMatch> {
    let temporal = temporal_columns(conn, Some(scheme))?;
    wisdom_match_impl(
        conn, query_vec, gate_key, floor, max, exclude, now_ms, temporal,
    )
}

/// Shared body: the caller passes the ONE `Temporal` (and therefore the ONE
/// resolved scheme) that produced its floor, and that same value drives the
/// SELECT and correction filters — floor and filter can never disagree.
#[allow(clippy::too_many_arguments)]
fn wisdom_match_impl(
    conn: &Connection,
    query_vec: &[f32],
    gate_key: &str,
    floor: Option<f32>,
    max: usize,
    exclude: &[String],
    now_ms: i64,
    temporal: Temporal,
) -> Result<WisdomMatch> {
    let max = max.min(MAX_ENTRIES);
    let corrections = if temporal.has_superseded_by {
        find_corrections(conn, exclude, now_ms, &temporal)?
    } else {
        Vec::new()
    };
    let gate = match floor {
        Some(_) => format!("semantic-v1:{gate_key}"),
        None => GATE_UNCALIBRATED.to_string(),
    };
    let entries = match floor {
        Some(floor) if max > 0 && !query_vec.is_empty() => {
            // One id, one list: excluded ids and correction heads are skipped.
            let mut skip: HashSet<String> = exclude.iter().cloned().collect();
            skip.extend(corrections.iter().map(|c| c.id.clone()));
            gated_entries(
                conn,
                query_vec,
                Some(floor),
                max,
                &skip,
                now_ms,
                &temporal,
                None,
                None,
            )?
        }
        _ => Vec::new(),
    };
    Ok(WisdomMatch {
        schema: SCHEMA_VERSION,
        gate,
        entries,
        corrections,
    })
}

type CandidateRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
);

/// THE shared eligibility predicate (spec §3.2): ONE WHERE fragment consumed
/// by `gated_entries` AND the stage-1 candidate/complement queries in
/// `two_stage`, so the two stages can never disagree about eligibility.
/// The enumeration is exhaustive for the live-gate-eligible test:
/// `deleted_at IS NULL`, NULL-only title/body (moved from `gated_entries`'s
/// Rust-side `let (Some, Some, Some)` into SQL — plan-approved tightening),
/// supersession/validity per the probed `Temporal` columns, and the V27
/// read-scheme filter (spec §3.2: a stage-1 query without it is a violation).
/// Aliases `e.` for `llm_wiki_entries` — consumers MUST alias the table `e`.
/// When NEITHER temporal column exists no bind params are needed; otherwise
/// bind `:now` (see `eligible_entry_ids` / `gated_entries`).
///
/// Note: the fragment body itself does NOT carry a `{scheme_filter}` literal
/// token — the filter is applied at RUNTIME from `t.scheme`. Every interpolating
/// statement keeps the `{scheme_filter}` token in its own SQL literal so the
/// embed_scheme reader inventory still sees it.
pub(crate) fn entry_eligibility_where(t: &Temporal) -> String {
    let scheme_filter = crate::embed_scheme::scheme_filter_sql(t.scheme);
    let mut sql = format!(
        "e.deleted_at IS NULL \
         AND e.title IS NOT NULL AND e.body IS NOT NULL \
         AND e.embedding_blob IS NOT NULL{scheme_filter}"
    );
    // DEVIATION from the plan sketch (recorded per Task-2 handoff nuance 1):
    // the sketch wrote `e.superseded_at <= :now`, but its own stated
    // semantics — "superseded entries whose superseded_at is still in the
    // future stay eligible" — and v1's behavior (superseded rows never
    // surface once the supersession has taken effect) pin the OPPOSITE
    // comparison: eligible iff no supersession is in effect yet.
    if t.has_superseded_by {
        sql.push_str(" AND (e.superseded_by IS NULL OR e.superseded_at > :now)");
    }
    if t.has_valid_to {
        sql.push_str(" AND (e.valid_to IS NULL OR e.valid_to > :now)");
    }
    sql
}

/// The shared predicate as a standalone query (spec §3.2): Rust-side consumers
/// (Task 3's stage-1 complement) get the eligible id set from the SAME
/// fragment `gated_entries` interpolates.
// Real consumer landed in Task 3 (`two_stage::complement_entry_ids`) — the
// parity tests are no longer the only callers.
pub(crate) fn eligible_entry_ids(
    conn: &Connection,
    t: &Temporal,
    now_ms: i64,
) -> Result<Vec<String>> {
    let where_sql = entry_eligibility_where(t);
    let sql = format!("SELECT e.id FROM llm_wiki_entries e WHERE {where_sql}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = if t.has_superseded_by || t.has_valid_to {
        // Only the temporal arms reference :now — on a shape with neither
        // column, binding an unused named param is a rusqlite error.
        stmt.query_map(rusqlite::named_params! { ":now": now_ms }, |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

#[allow(clippy::too_many_arguments)] // internal helper; the 8 params read as one query shape
fn gated_entries(
    conn: &Connection,
    query_vec: &[f32],
    floor: Option<f32>,
    max: usize,
    skip: &HashSet<String>,
    now_ms: i64,
    temporal: &Temporal,
    id_filter: Option<&[String]>,
    provenance_filter: Option<&[&str]>,
) -> Result<Vec<WisdomItem>> {
    // THE shared eligibility predicate (spec §3.2) — the same fragment
    // `eligible_entry_ids` (and Task 3's stage-1 SQL) consumes, so the v1
    // gate and stage-1 can never disagree about which facts are eligible.
    // The SELECT literal carries a `{scheme_filter}` token so the
    // embed_scheme reader inventory recognizes this candidate reader; the
    // LIVE scheme filter is applied INSIDE the shared fragment (from the
    // same `temporal.scheme`), so the token itself resolves to "" below.
    // The fragment's first arm (`e.deleted_at IS NULL`) anchors the WHERE
    // clause, which keeps the SQL valid when that filter is empty
    // (pre-V27 shape).
    let sql = concat!(
        "SELECT e.id, e.entity_id, e.title, e.body, e.source_type, e.embedding_blob",
        " FROM llm_wiki_entries e",
        " WHERE {scheme_filter}",
    );
    let sql = sql.replacen("{scheme_filter}", "", 1);
    let mut sql = format!("{sql}{}", entry_eligibility_where(temporal));
    if let Some(ids) = id_filter {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Task 4's two-stage selection narrows the scan to a precomputed id
        // set (rule-(i) restricted facts / rule-(ii) evidence-hash facts).
        // Ids are entry ids the queries above produced — never user input.
        let list = ids
            .iter()
            .map(|i| format!("'{i}'"))
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND e.id IN ({list})"));
    }
    let mut stmt = conn.prepare(&sql)?;
    let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<CandidateRow> {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    };
    let rows: Vec<rusqlite::Result<CandidateRow>> =
        if temporal.has_superseded_by || temporal.has_valid_to {
            // The shared fragment's temporal arms reference :now (possibly
            // twice in the SQL — one named binding covers both occurrences).
            stmt.query_map(rusqlite::named_params! { ":now": now_ms }, mapper)?
                .collect()
        } else {
            stmt.query_map([], mapper)?.collect()
        };

    let dim = query_vec.len();
    let mut scored: Vec<(f32, WisdomItem)> = Vec::new();
    for row in rows {
        let (id, entity_id, title, body, source_type, blob) = match row {
            Ok(row) => row,
            Err(e) => {
                // One malformed row never fails the call (rank_wiki_entries rule).
                eprintln!("ct wisdom match: skipping unreadable wiki row: {e}");
                continue;
            }
        };
        if skip.contains(&id) {
            continue;
        }
        let (Some(title), Some(body), Some(blob)) = (title, body, blob) else {
            continue;
        };
        if blob.len() != dim * 4 {
            continue;
        }
        // The delivery aperture (issue #282 §5 rev-15): the activity-active
        // path admits the HUMAN-TIER SET — `user_stated` AND
        // `user_confirmed` — as a provenance-set membership test, applied
        // BEFORE the arm-internal truncate (`:635` below) so a top-N of
        // filtered-out facts never shadows an admitted fact ranked below it.
        // Message mode and the merge pass `None` (unchanged).
        if let Some(want) = provenance_filter {
            let admitted = provenance_for(source_type.as_deref())
                .is_some_and(|p| want.contains(&p.as_str()));
            if !admitted {
                continue;
            }
        }
        let raw = cosine_similarity(query_vec, &bytes_to_f32(&blob));
        if let Some(floor) = floor {
            if raw < floor {
                continue; // the gate compares RAW cosine — never the weighted score
            }
        }
        let weighted = raw * tier_weight(&entity_id);
        scored.push((
            weighted,
            WisdomItem {
                id,
                title,
                text: body,
                score: Some(raw),
                supersedes: Vec::new(),
                provenance: provenance_for(source_type.as_deref()),
            },
        ));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    scored.truncate(max);
    Ok(scored.into_iter().map(|(_, item)| item).collect())
}

// ===================== Task 4: two-stage decision core (spec §3/§4/§6) =====================

/// Two-stage k per gate key — mirrors [`gate_floor`]'s lookup shape but is a
/// SEPARATE table (k is NOT a floor key, Opus r3 M3: an integer must never be
/// matchable by `gate_floor`). Default 8 until calibration pins a key.
pub const TWO_STAGE_K: &[(&str, usize)] = &[
    ("default", 8),
    // Pinned by the 2026-10-18 live calibration (chunk-hop @ 0.65 won; k
    // measured under scheme "raw" — rule (i) never uses stage-1 breadth, so
    // this row keys the UNSCHEMED profile key per Task 15's resolver pin).
    ("external:qwen/qwen3-embedding-4b", 8),
];

/// `two_stage_k` over an EXPLICIT table — the exact-find-then-default lookup
/// [`two_stage_k`] runs on [`TWO_STAGE_K`]. `pub` so the calibration harness
/// can pin the emitter↔resolver k-row binding against a SIMULATED paste
/// (production `TWO_STAGE_K` ships unflipped — task-15 controller notes: the
/// machine ships WITHOUT a flip, so the pasted row can only live in a test
/// table, never in this file).
pub fn two_stage_k_in(table: &[(&str, usize)], gate_key: &str) -> usize {
    table
        .iter()
        .find(|(k, _)| *k == gate_key)
        .or_else(|| table.iter().find(|(k, _)| *k == "default"))
        .map(|(_, k)| *k)
        .unwrap_or(8)
}

/// `two_stage_k(TWO_STAGE_K)` row for `gate_key`; the `default` row (8) when
/// the key (or its suffix tail) carries no pinned k. Never errors — an
/// unlisted key takes the default, exactly like an unlisted floor abstains.
pub fn two_stage_k(gate_key: &str) -> usize {
    two_stage_k_in(TWO_STAGE_K, gate_key)
}

/// The calibrated rule whose decision opened (or closed) the gate — the
/// audit line's `rule=i|ii` (§6, r28-m3: each rule carries its OWN decision
/// variable for `top`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwoStageRule {
    /// Rule (i): restricted-set fact-cosine floor (doc-level membership).
    RestrictedCosine,
    /// Rule (ii): member-chunk score floor (chunk-level membership,
    /// evidence-hash edge per r30-M3).
    ChunkHop,
}

impl TwoStageRule {
    /// The rule's composite floor-key segment and complement-rule tag.
    pub fn as_str(self) -> &'static str {
        match self {
            TwoStageRule::RestrictedCosine => "restricted-cosine",
            TwoStageRule::ChunkHop => "chunk-hop",
        }
    }
}

/// Everything the seven-field `wisdom_two_stage_audit` line needs (§6): the
/// caller formats `rule=<i|ii|none> open=<0|1> top=<score|none>
/// v1merged=<n> floor=<key> k=<k> stale_embed_key=<n>` from these fields.
#[derive(Debug, Clone, PartialEq)]
pub struct TwoStageOutcome {
    /// None = no two-stage rule opened (§6 m4: the v1 fallback / §4
    /// uncalibrated cells report `rule=none`).
    pub rule: Option<TwoStageRule>,
    /// Pinned (r30-m5): the FINAL entries list is non-empty.
    pub open: bool,
    /// The evaluated rule's OWN decision variable (r28-m3): rule (i) = best
    /// restricted FACT cosine, rule (ii) = best member-CHUNK score. `None` =
    /// the gate closed and no decision variable was produced.
    pub top: Option<f32>,
    /// Facts admitted by the §3 v1-merge.
    pub v1merged: usize,
    /// The floor key that drove this call (v1 key on fallback; composite
    /// `<floor_key_for(model, scheme)>:two-stage:<rule>` when two-stage ran).
    pub floor_key_used: String,
    /// The k used for stage 1 ([`two_stage_k`]'s default, 8).
    pub k: usize,
    /// Candidate docs (≥1 chunk) with a NULL/stale `embed_key` — reported
    /// even on a v1 fallback (r24 M2: the number is actionable after the fact).
    pub stale_embed_key: i64,
}

/// `(entries, outcome)` — Task 5's orchestrator destructures this.
pub type TwoStageResult = (Vec<WisdomItem>, TwoStageOutcome);

/// The whole two-stage evaluation (spec §3 v1-merge + §4 ONE fallback rule +
/// §6 coverage guard). `exclude` already contains the caller's excludes;
/// correction heads arrive via `corrections_absent_ids` — the corrections
/// ARRAY itself is computed by the CALLER, unchanged, so both paths emit
/// identical corrections (r29-m2). The skip set (`exclude` ∪ correction
/// heads) is applied to rule (i), rule (ii) AND the merge identically.
///
/// §4 matrix, implemented exactly:
/// 1. two-stage floor missing + v1 floor present → v1 fallback (`rule=none`).
/// 2. BOTH missing → uncalibrated: empty entries, no embeds (caller skips).
/// 3. two-stage present + v1 missing + coverage < 100% → uncalibrated
///    no-open (the one dark-adjacent cell, by design).
/// 4. two-stage present (+ v1 any) + coverage = 100% → two-stage runs.
/// 5. Below-floor stage 1 → closed, but the outcome still names the
///    evaluated rule (`rule=i|ii open=0` — never `rule=none` on a close).
///
/// Cell 1 (two-stage floor missing + v1 floor present) returns the CLOSED
/// shape (empty entries); the CALLER (Task 5) runs v1 and emits the v1
/// label.
#[allow(clippy::too_many_arguments)]
pub fn two_stage_entries(
    conn: &Connection,
    query_vec_raw: &[f32],
    query_vec_schemed: &[f32],
    scheme: Scheme,
    v1_floor: Option<f32>,
    two_stage_floor: Option<f32>,
    max: usize,
    exclude: &HashSet<String>,
    corrections_absent_ids: &HashSet<String>,
    now_ms: i64,
    profile_key: &str,
    kind: QueryKind,
) -> Result<TwoStageResult> {
    let max = max.min(MAX_ENTRIES);
    let k = two_stage_k(profile_key);
    // The delivery aperture rides with the kind (issue #282 §5 rev-15):
    // activity admits the human tier — `user_stated` AND `user_confirmed`;
    // message never filters. The v1-merge is dead under activity (its
    // `v1_floor` is `None` — no v1 `:activity` row ships), so the filter
    // below is inert there by construction.
    let provenance_filter: Option<&[&str]> = match kind {
        QueryKind::Message => None,
        QueryKind::Activity => Some(&["user_stated", "user_confirmed"][..]),
    };
    // One skip set for ALL selections ("one id, one list"):
    let mut skip: HashSet<String> = exclude.iter().cloned().collect();
    skip.extend(corrections_absent_ids.iter().cloned());

    let closed = |rule: Option<TwoStageRule>,
                  top: Option<f32>,
                  floor_key: String,
                  stale: i64,
                  merged: usize| {
        (
            Vec::<WisdomItem>::new(),
            TwoStageOutcome {
                rule,
                open: false,
                top,
                v1merged: merged,
                floor_key_used: floor_key,
                k,
                stale_embed_key: stale,
            },
        )
    };

    // §4 cells 1-2 resolved by `two_stage_floor`:
    //   None + v1 None  → uncalibrated (cell 2);
    //   None + v1 Some  → v1 fallback (cell 1);
    //   Some            → two-stage runs (cells 3/4, decided by coverage).
    let Some(ts_floor) = two_stage_floor else {
        if v1_floor.is_some() {
            return Ok(closed(None, None, profile_key.to_string(), 0, 0));
        }
        return Ok(closed(None, None, String::new(), 0, 0));
    };

    // §6 coverage guard BEFORE any stage-1 work. `Ok(None)` = probe miss
    // (pre-V28 / pre-V18 brain) ⇒ counts as coverage 0 with the candidate
    // count reported when it can be probed (r28-M3; never an error).
    let probe = crate::two_stage::coverage(conn, profile_key, now_ms)?;
    let (cand_count, covered) = probe.unwrap_or((0, 0));
    let stale = cand_count - covered;
    if covered < cand_count {
        // Coverage < 100%: v1 fallback when a v1 floor exists (§4's one
        // rule), uncalibrated no-open when it does not (cell 3).
        let floor_key = if v1_floor.is_some() {
            profile_key.to_string()
        } else {
            String::new()
        };
        return Ok(closed(None, None, floor_key, stale, 0));
    }

    // Two-stage runs (§4 cell 4). Stage 1 with the RAW vector (no prefix).
    let Some(cands) = crate::two_stage::candidate_set(conn, now_ms, Some(scheme))? else {
        // Schema skew surfacing between the coverage probe and here — the
        // never-dark invariant degrades to v1, never errors (r28-M3).
        let floor_key = if v1_floor.is_some() {
            profile_key.to_string()
        } else {
            String::new()
        };
        return Ok(closed(None, None, floor_key, stale, 0));
    };
    let hits = crate::two_stage::stage1_search(conn, query_vec_raw, &cands, k)?;

    // Rule selection: the caller resolves ONE composite floor per rule and
    // passes the calibrated rule's floor here; the key names which rule this
    // floor belongs to (Task 5/harness pass `floor_key_for(key, scheme) +
    // ":two-stage:" + rule`). The core reconstructs the rule tag from the
    // caller's floor key so `floor_key_used` round-trips.
    // For the shipped default path the caller resolves in pinned order
    // (restricted-cosine, then chunk-hop) and passes the FIRST hit's floor.
    // `floor_key_used` therefore needs the rule name — carried by the
    // caller's composite key convention. We derive it from which rule's
    // mechanics the caller wants by checking both composite keys: the one
    // matching `two_stage_floor` is the calibrated rule.
    // Site 5 of the floor-key resolution chain (spec rev-6 B1): the
    // attribution block derives its OWN scheme key — from the kind-carrying
    // base, so an activity floor attributes against the activity composites,
    // never the message ones. Without this, a calibrated activity floor (≠
    // the message value) attributes NOTHING and the tie-break default
    // RestrictedCosine would run against a chunk-hop-calibrated floor.
    let scheme_key = floor_key_for(&kind.floor_base_key(profile_key), scheme);
    let rule;
    let mut attributed = true;
    if two_stage_rule_floor(&format!("{scheme_key}:two-stage:restricted-cosine"))
        .is_some_and(|f| f == ts_floor)
    {
        rule = TwoStageRule::RestrictedCosine;
    } else if two_stage_rule_floor(&format!("{scheme_key}:two-stage:chunk-hop"))
        .is_some_and(|f| f == ts_floor)
    {
        rule = TwoStageRule::ChunkHop;
    } else {
        // Floor came from a key this core cannot attribute (harness passes
        // explicit floors during calibration sweeps): rule (i) is the
        // tie-break default (GLM r1 minor-5 — simpler path). The audit line
        // must NOT name a calibration key that did not drive the decision,
        // so `floor_key_used` below carries an explicit marker instead.
        attributed = false;
        rule = TwoStageRule::RestrictedCosine;
        // In tests this fires only for non-calibration-shaped floors: the
        // harness (compiles without cfg(test), so it resolves via gate_floor
        // on the production table only) passes explicit floors during
        // calibration sweeps and lands HERE — the rule-(i) tie-break.
        // (No debug_assert here: the harness legitimately reaches this arm.)
    };
    let floor_key_used = if attributed {
        format!("{scheme_key}:two-stage:{}", rule.as_str())
    } else {
        format!("{scheme_key}:two-stage:unattributed")
    };

    let temporal = temporal_columns(conn, Some(scheme))?;

    // ---- The evaluated rule's arm + its decision variable. ----
    let (rule_entries, top): (Vec<WisdomItem>, Option<f32>) = match rule {
        TwoStageRule::RestrictedCosine => {
            // Rule (i): score the RESTRICTED facts (rule-reachable set) with
            // the SCHEME-PREFIXED vector; open iff the best fact cosine ≥
            // the two-stage floor. The floor-drop (raw < floor) applies —
            // `top` is the best RESTRICTED fact cosine that CLEARED it.
            let comp = crate::two_stage::complement_entry_ids(
                conn,
                &cands,
                rule.as_str(),
                now_ms,
                Some(scheme),
            )?;
            let comp_set: HashSet<&str> = comp.iter().map(String::as_str).collect();
            let eligible = eligible_entry_ids(conn, &temporal, now_ms)?;
            let restricted: Vec<String> = eligible
                .into_iter()
                .filter(|id| !comp_set.contains(id.as_str()))
                .collect();
            let scored = gated_entries(
                conn,
                query_vec_schemed,
                Some(ts_floor),
                max,
                &skip,
                now_ms,
                &temporal,
                Some(&restricted),
                provenance_filter,
            )?;
            let best = scored
                .iter()
                .filter_map(|e| e.score)
                .fold(0.0_f32, f32::max);
            let top = if scored.is_empty() { None } else { Some(best) };
            (scored, top)
        }
        TwoStageRule::ChunkHop => {
            // Rule (ii): the top-1 MEMBER chunk above the chunk-hop floor
            // (r30-m1) opens ALL facts whose evidence hash resolves into
            // that chunk's doc (evidence-hash edge, r30-M3) with NO
            // fact-level floor (r25-M4, r26 m2: floor = None bypasses the
            // raw<floor drop). The chunk score is NEVER an entry score.
            let members: HashSet<i64> = crate::two_stage::member_chunk_ids(conn, &cands)?
                .into_iter()
                .collect();
            let hit = hits
                .iter()
                .find(|h| members.contains(&h.chunk_id) && h.score >= ts_floor);
            match hit {
                None => (Vec::new(), None),
                Some(best_member) => {
                    // Facts evidenced into the opened doc: the evidence-hash
                    // edge, same join shape as member_chunk_ids' hash leg.
                    let sql = "
                        SELECT DISTINCT le.entry_id FROM librarian_evidence le
                        JOIN json_each(le.evidence_json, '$.evidence') je
                        JOIN chunks c ON c.content_hash = json_extract(je.value, '$.content_hash')
                       WHERE c.doc_id = ?1";
                    let ids: Vec<String> = conn
                        .prepare(sql)?
                        .query_map([best_member.doc_id], |r| r.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let scored = gated_entries(
                        conn,
                        query_vec_schemed,
                        None, // r26 m2: NO fact-level floor under rule (ii)
                        max,
                        &skip,
                        now_ms,
                        &temporal,
                        Some(&ids),
                        provenance_filter,
                    )?;
                    (scored, Some(best_member.score))
                }
            }
        }
    };

    // ---- §3 v1-merge: the STRUCTURAL complement, scored via the v1 path
    // with the SCHEME-PREFIXED vector (r28-m2), floor = the v1 floor. ----
    let complement =
        crate::two_stage::complement_entry_ids(conn, &cands, rule.as_str(), now_ms, Some(scheme))?;
    let mut merged: Vec<(f32, WisdomItem)> = Vec::new();
    let mut v1merged = 0usize;
    if v1_floor.is_some() && max > 0 && !query_vec_schemed.is_empty() && !complement.is_empty() {
        let merge_scored = gated_entries(
            conn,
            query_vec_schemed,
            v1_floor,
            usize::MAX, // score the whole complement; the final merge truncates
            &skip,
            now_ms,
            &temporal,
            Some(&complement),
            provenance_filter,
        )?;
        // Counts complement rows ABOVE the v1 floor BEFORE the final
        // truncate(max) — i.e. the raw merge yield, not the shipped count.
        v1merged = merge_scored.len();
        // Weighted-cosine re-rank happens in the combined merge below —
        // gated_entries already sorted, so preserve its weighted order by
        // reading the weighted key back: recompute weighted = raw·tier is
        // NOT possible from WisdomItem (entity_id is dropped), so re-sort
        // with the SAME weighted comparator using each item's raw score.
        for item in merge_scored {
            let raw = item.score.unwrap_or(0.0);
            merged.push((raw, item));
        }
    }
    // Rule entries re-keyed to their weighted score for ONE combined rank.
    // tier_weight needs entity_id, which WisdomItem drops — so rank BOTH
    // groups by RAW cosine? No: the pinned order is the WEIGHTED cosine
    // (`gated_entries`' path). To reuse that exact ordering without
    // re-deriving entity ids, run the final merge through the same weighted
    // comparator by querying entity_id per id (cheap, id-filtered).
    let mut weighted_rule: Vec<(f32, WisdomItem)> = Vec::with_capacity(rule_entries.len());
    for item in rule_entries {
        let raw = item.score.unwrap_or(0.0);
        let w = raw * entity_tier_weight(conn, &item.id)?;
        weighted_rule.push((w, item));
    }
    for (raw, item) in merged.iter_mut() {
        let w = *raw * entity_tier_weight(conn, &item.id)?;
        *raw = w;
    }
    let mut combined = weighted_rule;
    combined.append(&mut merged);
    combined.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    combined.truncate(max);
    let entries: Vec<WisdomItem> = combined.into_iter().map(|(_, item)| item).collect();
    let open = !entries.is_empty(); // r30-m5: FINAL list non-empty
                                    // Rule closes below its floor while the merge opens: rule stays the
                                    // evaluated rule, top stays the RULE's variable (r27 m5, r28-m3).
    Ok((
        entries,
        TwoStageOutcome {
            rule: Some(rule),
            open,
            top,
            v1merged,
            floor_key_used,
            k,
            stale_embed_key: stale,
        },
    ))
}

/// `tier_weight` for one entry id (entity tier lookup). Cheap id-filtered
/// read; the two-stage merge needs the weighted order `gated_entries` uses,
/// and `WisdomItem` deliberately carries no entity id.
fn entity_tier_weight(conn: &Connection, id: &str) -> Result<f32> {
    let entity_id: Option<String> = conn
        .query_row(
            "SELECT entity_id FROM llm_wiki_entries WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(crate::wiki_graph::tier_weight(
        entity_id.as_deref().unwrap_or(""),
    ))
}

/// The two-stage orchestrator (issue #271 plan Task 5): the shared entry
/// point the CLI (`ct wisdom match --two-stage`) and the desktop driver
/// (Task 14/15) call. Resolves the §4 floors, applies the §6 coverage-first
/// gate, and runs `two_stage_entries`; embeds are the CALLER's (the embed
/// inputs are passed in), so this fn stays free of embedder I/O.
///
/// Caller contract (pinned order, plan Step 3):
///  1. The caller calls [`two_stage_pre_embed_check`] FIRST (the shared §3
///     embed-skip authority) and embeds ONLY the vectors its
///     [`TwoStagePreEmbed::embed_plan`] names — stage-1 RAW vector + schemed
///     vector (`Scheme::Raw` reuses one vector; `Instr1` embeds the prefixed
///     text). The §6 coverage gate therefore sees the call BEFORE any embed
///     was spent, on EVERY caller path.
///  2. This fn runs the coverage probe again internally (cheap, ONE
///     authority for the §4 cell decision) and falls back to v1 exactly
///     per §4 — the precheck's projection is advisory; the in-call decision
///     is authoritative.
///
/// Returns `(match_result, outcome)`: the match carries the FINAL entries
/// (v1 fallback ⇒ the caller's `v1_result` semantics — see
/// [`wisdom_match_two_stage_with_v1`]) and the label the caller prints.
pub struct TwoStageRun {
    /// The two-stage arm's own entries (may be empty on any fallback).
    pub result: WisdomMatch,
    /// The audit outcome for the seven-field stderr line.
    pub outcome: TwoStageOutcome,
    /// True when the two-stage arm actually RAN (label
    /// `semantic-v2-two-stage:{key}`); false on any §4 fallback/closed cell
    /// (the caller keeps the v1 label — CARRIED WARNING: cell 1 falls back to
    /// a FULL v1 run, which the caller performs via
    /// [`wisdom_match_two_stage_with_v1`]).
    pub ran: bool,
}

/// Which vectors the CALLER must produce before calling
/// [`wisdom_match_two_stage_with_v1`] — the shared §3 embed-skip authority
/// (fix-round Important #1/#2): every caller invokes
/// [`two_stage_pre_embed_check`] FIRST and embeds exactly what this plan
/// names, so the §6 coverage gate can veto BEFORE any embedder round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwoStageEmbedPlan {
    /// Uncalibrated / cell-3 no-open: NO vectors needed, embed nothing.
    None,
    /// The v1 fallback (or any v1-scored path): only the SCHEME-PREFIXED
    /// query vector.
    SchemedOnly,
    /// Two-stage will run: the stage-1 RAW vector plus the schemed vector
    /// (under `Scheme::Raw` the caller reuses the raw bytes for both —
    /// r26 m3, ONE embed per message).
    RawAndSchemed,
}

/// Everything [`two_stage_pre_embed_check`] answers BEFORE the caller embeds.
#[derive(Debug, Clone, PartialEq)]
pub struct TwoStagePreEmbed {
    /// The v1 floor (`gate_floor(scheme_key)`) is calibrated.
    pub v1_floor_present: bool,
    /// A composite two-stage floor (restricted-cosine OR chunk-hop, pinned
    /// order) is calibrated.
    pub ts_floor_present: bool,
    /// True when `two_stage::coverage` missed (pre-V28 shape etc.) — no
    /// coverage information; `stale_embed_key` is meaningless (0).
    pub coverage_probe_miss: bool,
    /// The §4 cell this call would land in: 1 = v1 fallback, 2 = both
    /// floors missing (uncalibrated), 3 = cell-3 uncalibrated no-open,
    /// 4 = two-stage runs. Projection only — the orchestrator re-derives
    /// the cell inside (ONE authority for decisions; the probe is cheap).
    pub projected_cell: u8,
    /// Vectors the caller must embed before the orchestrator call.
    pub embed_plan: TwoStageEmbedPlan,
    /// Candidate docs with a stale/NULL embed_key when the probe can answer,
    /// else 0. Mirrors the audit line's `stale_embed_key`.
    pub stale_embed_key: i64,
}

/// The PUB pre-embed precheck (fix-round Important #1/#2 — the shared §3
/// embed-skip authority): resolves the §4 floors and runs the §6 coverage
/// probe BEFORE any embed, returning which floors are present, the projected
/// §4 cell, whether embedding is needed at all (and which vectors), and the
/// stale-embed_key count when the probe can answer.
///
/// Caller contract (pinned order, plan Step 3.3 — "coverage check BEFORE any
/// embed"):
///  1. Call THIS fn first. Embed exactly [`TwoStagePreEmbed::embed_plan`]'s
///     vectors (`None` ⇒ zero embedder round-trips).
///  2. Then call [`wisdom_match_two_stage_with_v1`] with those vectors. The
///     orchestrator keeps its internal re-probe (cheap; one authority for
///     cell decisions) — the precheck is the CHEAP EARLY answer that makes
///     the coverage gate able to veto before any embed.
///
/// Task 14/15 callers use this instead of reimplementing floor/cell/coverage
/// logic.
pub fn two_stage_pre_embed_check(
    conn: &Connection,
    profile_key: &str,
    scheme: Scheme,
    now_ms: i64,
    kind: QueryKind,
) -> Result<TwoStagePreEmbed> {
    // Floor resolution — mirrors `wisdom_match_two_stage_with_v1` exactly
    // (site 2 of the resolution chain; the kind selects the `:activity`
    // base). `two_stage::coverage` stays on the UNSUFFIXED profile key
    // (coverage is keyed per model, never per query kind).
    let scheme_key = floor_key_for(&kind.floor_base_key(profile_key), scheme);
    let v1_floor_present = gate_floor(&scheme_key).is_some();
    let ts_floor_present =
        two_stage_rule_floor(&format!("{scheme_key}:two-stage:restricted-cosine"))
            .or_else(|| two_stage_rule_floor(&format!("{scheme_key}:two-stage:chunk-hop")))
            .is_some();

    // The §6 coverage probe FIRST — this whole fn exists so the gate can
    // veto before the caller spends an embed.
    let probe = crate::two_stage::coverage(conn, profile_key, now_ms)?;

    let (projected_cell, embed_plan, stale_embed_key) = match (v1_floor_present, ts_floor_present) {
        // §4 cell 2: both missing — uncalibrated, no embeds at all.
        (false, false) => (2, TwoStageEmbedPlan::None, 0),
        // §4 cell 1: v1-only fallback — needs exactly the schemed vector.
        // Probe miss also lands here whenever the v1 floor exists (no
        // coverage veto possible without the probe).
        (true, false) => (1, TwoStageEmbedPlan::SchemedOnly, 0),
        // Composite floor present: coverage decides cell 3 vs cell 4.
        (v1, true) => match probe {
            None => {
                // Probe miss with a composite floor: no coverage veto is
                // possible; the orchestrator treats a miss as coverage 0 ⇒
                // cell 3 when v1 is missing, cell-1-shaped fallback when v1
                // exists. Project the DARKER cell (3 / no embeds) so the
                // caller never embeds for a call that cannot open.
                if v1 {
                    (1, TwoStageEmbedPlan::SchemedOnly, 0)
                } else {
                    (3, TwoStageEmbedPlan::None, 0)
                }
            }
            Some((n, cov)) => {
                let stale = n - cov;
                if cov < n {
                    // §4 cell 3: uncalibrated no-open (or v1 fallback when
                    // the v1 floor exists — either way the two-stage arm
                    // never opens and the caller needs no two-stage embeds;
                    // a v1-present caller still needs the schemed vector
                    // for the fallback run).
                    if v1 {
                        (3, TwoStageEmbedPlan::SchemedOnly, stale)
                    } else {
                        (3, TwoStageEmbedPlan::None, stale)
                    }
                } else {
                    // §4 cell 4: two-stage runs — stage-1 raw + schemed.
                    (4, TwoStageEmbedPlan::RawAndSchemed, stale)
                }
            }
        },
    };

    Ok(TwoStagePreEmbed {
        v1_floor_present,
        ts_floor_present,
        coverage_probe_miss: probe.is_none(),
        projected_cell,
        embed_plan,
        stale_embed_key,
    })
}

/// Full two-stage run INCLUDING the §4 cell-1 v1 fallback: on every fallback
/// cell the v1 path executes against the same connection (the caller passes
/// the v1 query vector it already embedded — or an empty slice when embeds
/// were skipped) and the returned [`TwoStageRun::result`] carries the v1
/// entries with the v1 gate label. The two-stage arm's own (empty) result is
/// discarded; only its audit outcome survives.
#[allow(clippy::too_many_arguments)]
pub fn wisdom_match_two_stage_with_v1(
    conn: &Connection,
    query_vec_raw: &[f32],
    query_vec_schemed: &[f32],
    scheme: Scheme,
    max: usize,
    exclude: &[String],
    now_ms: i64,
    profile_key: &str,
    kind: QueryKind,
) -> Result<TwoStageRun> {
    // Site 3 of the resolution chain (spec rev-8 M1 — the OPPOSITE of moot):
    // the base key carries the kind's `:activity` suffix, which (a) makes
    // `v1_floor` `None` under activity (no v1 `:activity` row ships ⇒ the
    // §3 v1-merge is dead, exactly as the spec pins) and (b) builds the
    // ran-path label `semantic-v2-two-stage:{scheme_key}` with the `:activity`
    // suffix and NO composite segment (rev-6 M1). Resolving `v1_floor` from
    // the message key here is FORBIDDEN — it would run the merge on the
    // message floor inside the activity attempt.
    let v1_key = kind.floor_base_key(profile_key);
    // Floor resolution (plan Step 3.2): v1 floor under the resolved scheme;
    // the composite two-stage floors in PINNED rule order. The composite
    // lookup goes through `two_stage_rule_floor` so test builds see the
    // test-only slice — mirroring the core's attribution exactly.
    let scheme_key = floor_key_for(&v1_key, scheme);
    let v1_floor = gate_floor(&scheme_key);
    let ts_floor = two_stage_rule_floor(&format!("{scheme_key}:two-stage:restricted-cosine"))
        .or_else(|| two_stage_rule_floor(&format!("{scheme_key}:two-stage:chunk-hop")));

    // Probe BEFORE any decision so `stale_embed_key` is reportable on every
    // path (r24 M2). `Ok(None)` = probe miss (pre-V28 shape) ⇒ v1 fallback
    // with the count reported when the probe can still answer it (r28-M3).
    // The core re-probes inside `two_stage_entries` — the probe is cheap
    // (indexed COUNT over candidate docs) and one authority keeps the §4
    // cells from drifting; `stale` here is only a cross-check.
    let _probe = crate::two_stage::coverage(conn, profile_key, now_ms)?;

    // §4 matrix, resolved ONCE here and mirrored by `two_stage_entries`:
    //   cell 1: ts floor missing + v1 present  → v1 fallback
    //   cell 2: BOTH missing                   → uncalibrated (no embeds —
    //           the caller checked `two_stage_embed_keys` before embedding)
    //   cell 3: ts present + v1 missing + cov<100% → uncalibrated no-open
    //   cell 4: ts present + cov = 100%        → two-stage runs
    // The cell decision ITSELF is re-derived inside the core from the same
    // floors; here we only route (and the core re-probes coverage).
    let (ts_entries, outcome) = two_stage_entries(
        conn,
        query_vec_raw,
        query_vec_schemed,
        scheme,
        v1_floor,
        ts_floor,
        max,
        &exclude.iter().cloned().collect(),
        &HashSet::new(),
        now_ms,
        profile_key,
        kind,
    )?;
    let ran = outcome.rule.is_some();
    debug_assert!(
        ran || ts_entries.is_empty(),
        "fallback cells never carry two-stage entries"
    );

    // §4 cell-1 (and any other non-running cell): run FULL v1 and return ITS
    // entries under the v1 label. Corrections are computed inside the v1
    // path, so both paths emit identical corrections (r29-m2). The v1 vector
    // is the SCHEMED one — `wisdom_match_in_scheme`'s own contract.
    if !ran {
        // `v1_key` already carries the activity suffix (above), so this call
        // resolves NO floor (rev-9/rev-10 M1): `wisdom_match_impl` takes its
        // `None` arm (`:374`), `gated_entries` is NEVER entered inside the
        // activity attempt — no v1 scan, no per-row stderr warnings. The
        // activity caller DISCARDS this result and re-runs the message path.
        let v1_result = wisdom_match_in_scheme(
            conn,
            query_vec_schemed,
            &v1_key,
            scheme,
            max,
            exclude,
            now_ms,
        )?;
        return Ok(TwoStageRun {
            result: v1_result,
            outcome,
            ran: false,
        });
    }

    // Two-stage ran: the arm's entries ARE the final entries. Corrections
    // flow through the v1 resolver unchanged (the entries themselves carry
    // the skip set already).
    let corrections = find_corrections_public(conn, exclude, now_ms, scheme)?;
    Ok(TwoStageRun {
        result: WisdomMatch {
            schema: SCHEMA_VERSION,
            gate: format!("{TWO_STAGE_LABEL_PREFIX}{scheme_key}"),
            entries: ts_entries,
            corrections,
        },
        outcome,
        ran: true,
    })
}

/// Corrections for the two-stage ran-path: the same resolver the v1 path
/// uses, under the same scheme (r29-m2 — identical corrections both paths).
/// `pub` for the calibration harness (issue #282 review: the paired arms
/// must derive their correction-head set from THIS production resolver, so
/// the baseline and both two-stage arms can never disagree about which
/// heads are skipped).
pub fn find_corrections_public(
    conn: &Connection,
    exclude: &[String],
    now_ms: i64,
    scheme: Scheme,
) -> Result<Vec<WisdomItem>> {
    let temporal = temporal_columns(conn, Some(scheme))?;
    find_corrections(conn, exclude, now_ms, &temporal)
}

fn find_corrections(
    conn: &Connection,
    exclude: &[String],
    now_ms: i64,
    temporal: &Temporal,
) -> Result<Vec<WisdomItem>> {
    let exclude_set: HashSet<&str> = exclude.iter().map(String::as_str).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<WisdomItem> = Vec::new();
    let mut by_head: HashMap<String, usize> = HashMap::new();
    for old in exclude {
        if !seen.insert(old.as_str()) {
            continue;
        }
        let Some(head) = resolve_head(conn, old, now_ms, temporal)? else {
            continue;
        };
        if exclude_set.contains(head.as_str()) {
            continue; // the caller already holds the replacement
        }
        if let Some(&i) = by_head.get(&head) {
            out[i].supersedes.push(old.clone());
            continue;
        }
        let Some(mut item) = load_item(conn, &head)? else {
            continue;
        };
        item.supersedes.push(old.clone());
        by_head.insert(head, out.len());
        out.push(item);
    }
    Ok(out)
}

/// Live head of `start`'s `superseded_by` chain, or None when `start` is
/// missing or not superseded, the chain hits a deleted or expired row, a
/// cycle, or more than `MAX_CHAIN_DEPTH` hops.
fn resolve_head(
    conn: &Connection,
    start: &str,
    now_ms: i64,
    temporal: &Temporal,
) -> Result<Option<String>> {
    let first: Option<Option<String>> = conn
        .query_row(
            "SELECT superseded_by FROM llm_wiki_entries WHERE id = ?1",
            params![start],
            |r| r.get(0),
        )
        .optional()?;
    let Some(Some(mut next)) = first else {
        return Ok(None);
    };
    let valid_to_col = if temporal.has_valid_to {
        "valid_to"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT superseded_by, deleted_at IS NOT NULL, {valid_to_col}
         FROM llm_wiki_entries WHERE id = ?1"
    );
    let mut visited: HashSet<String> = HashSet::from([start.to_string()]);
    for _ in 0..MAX_CHAIN_DEPTH {
        if !visited.insert(next.clone()) {
            return Ok(None); // cycle
        }
        let row: Option<(Option<String>, bool, Option<i64>)> = conn
            .query_row(&sql, params![next], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        let Some((superseded_by, deleted, valid_to)) = row else {
            return Ok(None);
        };
        if deleted {
            return Ok(None);
        }
        match superseded_by {
            Some(n) => next = n,
            None => {
                let live = valid_to.is_none_or(|v| v > now_ms);
                return Ok(live.then_some(next));
            }
        }
    }
    Ok(None) // depth exceeded
}

fn load_item(conn: &Connection, id: &str) -> Result<Option<WisdomItem>> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT title, body, source_type FROM llm_wiki_entries
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.and_then(|(title, body, source_type)| {
        Some(WisdomItem {
            id: id.to_string(),
            title: title?,
            text: body?,
            score: None,
            supersedes: Vec::new(),
            provenance: provenance_for(source_type.as_deref()),
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::embedder::ExternalEmbedProfile;

    const NOW: i64 = 1_000_000;
    const KEY: &str = "test:model";
    /// Real qwen key: the only model with BOTH per-scheme floors registered
    /// (raw 0.70, instr1 0.64), so `wisdom_match`'s scheme-derived floor
    /// selection is exercisable without a stub.
    const KEY_EXT: &str = "external:qwen/qwen3-embedding-4b";

    fn blob(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Unit vector whose cosine with the query `[1, 0]` is `c`.
    fn at(c: f32) -> Vec<f32> {
        vec![c, (1.0 - c * c).max(0.0).sqrt()]
    }

    const Q: [f32; 2] = [1.0, 0.0];

    fn seed(conn: &Connection, id: &str, entity_id: &str, v: &[f32]) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?5, NULL)",
            params![id, entity_id, format!("T {id}"), format!("B {id}"), blob(v)],
        )
        .unwrap();
    }

    fn supersede(conn: &Connection, old: &str, new: &str) {
        conn.execute(
            "UPDATE llm_wiki_entries SET superseded_by = ?2, superseded_at = 50, valid_to = 50
             WHERE id = ?1",
            params![old, new],
        )
        .unwrap();
    }

    /// INSERT with the eligibility-predicate columns explicit; [`seed`] is the
    /// all-eligible special case (title implies body text, everything NULL).
    fn seed_with(
        conn: &Connection,
        id: &str,
        title: Option<&str>,
        deleted_at: Option<i64>,
        superseded_by: Option<&str>,
        superseded_at: Option<i64>,
        valid_to: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding,
                superseded_by, superseded_at, valid_to
             ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, ?5, ?6, NULL, ?7, ?8, ?9)",
            params![
                id,
                format!("ent-{id}"),
                title,
                title.map(|t| format!("B {t}")),
                deleted_at,
                blob(&[0.0, 1.0]),
                superseded_by,
                superseded_at,
                valid_to,
            ],
        )
        .unwrap();
    }

    fn seed_deleted(conn: &Connection, id: &str) {
        seed_with(
            conn,
            id,
            Some(&format!("T {id}")),
            Some(1),
            None,
            None,
            None,
        );
    }

    /// The plan's NULL-title arm, adapted to the LIVE schema (DEVIATION,
    /// recorded): `title` carries a NOT NULL constraint, so a literal NULL
    /// cannot be seeded; the not-really-a-fact degenerate is exercised via
    /// the empty-string form instead. It stays ELIGIBLE under both
    /// consumers' shared predicate — the parity assertion is what this row
    /// serves, not an exclusion.
    fn seed_null_title(conn: &Connection, id: &str) {
        seed_with(conn, id, Some(""), None, None, None, None);
    }

    /// `superseded_at` in the PAST (50 < NOW), mirroring [`supersede`]: the
    /// superseded entry has already lost effect, so it is no longer eligible.
    fn seed_superseded(conn: &Connection, id: &str, new: &str) {
        seed_with(
            conn,
            id,
            Some(&format!("T {id}")),
            None,
            Some(new),
            Some(50),
            None,
        );
    }

    /// `valid_to` in the PAST: the entry's validity window has closed.
    fn seed_expired(conn: &Connection, id: &str) {
        seed_with(
            conn,
            id,
            Some(&format!("T {id}")),
            None,
            None,
            None,
            Some(50),
        );
    }

    fn run(conn: &Connection, floor: Option<f32>, max: usize, exclude: &[&str]) -> WisdomMatch {
        let exclude: Vec<String> = exclude.iter().map(|s| s.to_string()).collect();
        wisdom_match_with_floor(conn, &Q, KEY, Scheme::Raw, floor, max, &exclude, NOW).unwrap()
    }

    fn ids(items: &[WisdomItem]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn floor_keeps_at_and_above_drops_below() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "exact", "ent", &Q);
        seed(&conn, "mid", "ent", &at(0.6));
        let m = run(&conn, Some(1.0), 5, &[]);
        assert_eq!(ids(&m.entries), vec!["exact"]);
        let m = run(&conn, Some(0.59), 5, &[]);
        assert_eq!(ids(&m.entries), vec!["exact", "mid"]);
        let m = run(&conn, Some(0.61), 5, &[]);
        assert_eq!(ids(&m.entries), vec!["exact"]);
        assert_eq!(m.gate, "semantic-v1:test:model");
        assert_eq!(m.schema, 1);
    }

    #[test]
    fn gate_uses_raw_cosine_not_tier_weight() {
        let conn = open_in_memory().unwrap();
        // 0.45 * 1.5 = 0.675 would pass a 0.5 floor if the weight leaked in.
        seed(&conn, "weighted", "tier_fact", &at(0.45));
        assert!(run(&conn, Some(0.5), 5, &[]).entries.is_empty());
    }

    #[test]
    fn tier_weight_orders_above_floor_and_id_breaks_ties() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "plain", "ent", &at(0.9));
        seed(&conn, "fact", "tier_fact", &at(0.8)); // 1.2 weighted
        seed(&conn, "b_tie", "ent", &at(0.7));
        seed(&conn, "a_tie", "ent", &at(0.7));
        let m = run(&conn, Some(0.5), 10, &[]);
        assert_eq!(ids(&m.entries), vec!["fact", "plain", "a_tie", "b_tie"]);
        assert!(
            (m.entries[0].score.unwrap() - 0.8).abs() < 1e-4,
            "score is raw cosine"
        );
        assert_eq!(
            ids(&run(&conn, Some(0.5), 2, &[]).entries),
            vec!["fact", "plain"]
        );
        assert!(run(&conn, Some(0.5), 0, &[]).entries.is_empty());
        assert_eq!(run(&conn, Some(0.5), 99, &[]).entries.len(), 4); // clamped, all fit
    }

    #[test]
    fn excluded_ids_never_match() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "a", "ent", &Q);
        seed(&conn, "b", "ent", &Q);
        assert_eq!(ids(&run(&conn, Some(0.5), 5, &["a"]).entries), vec!["b"]);
    }

    #[test]
    fn current_only_filter() {
        let conn = open_in_memory().unwrap();
        for id in ["live", "old", "expired", "future", "gone"] {
            seed(&conn, id, "ent", &Q);
        }
        supersede(&conn, "old", "live");
        conn.execute(
            "UPDATE llm_wiki_entries SET valid_to = ?1 WHERE id = 'expired'",
            params![NOW],
        )
        .unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET valid_to = ?1 WHERE id = 'future'",
            params![NOW + 1],
        )
        .unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 5 WHERE id = 'gone'",
            [],
        )
        .unwrap();
        assert_eq!(
            ids(&run(&conn, Some(0.5), 10, &[]).entries),
            vec!["future", "live"]
        );
    }

    #[test]
    fn dimension_mismatch_skipped() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "three_d", "ent", &[1.0, 0.0, 0.0]);
        assert!(run(&conn, Some(0.1), 5, &[]).entries.is_empty());
    }

    #[test]
    fn uncalibrated_abstains_but_corrections_flow() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "old", "ent", &Q);
        seed(&conn, "new", "ent", &Q);
        supersede(&conn, "old", "new");
        let m = run(&conn, None, 5, &["old"]);
        assert_eq!(m.gate, GATE_UNCALIBRATED);
        assert!(m.entries.is_empty());
        assert_eq!(ids(&m.corrections), vec!["new"]);
        assert_eq!(m.corrections[0].supersedes, vec!["old"]);
        assert_eq!(m.corrections[0].score, None);
    }

    #[test]
    fn correction_chain_resolution() {
        let conn = open_in_memory().unwrap();
        for id in [
            "a1", "a2", "a3", "d1", "d2", "c1", "c2", "x1", "x2", "plain",
        ] {
            seed(&conn, id, "ent", &at(0.1));
        }
        supersede(&conn, "a1", "a2"); // multi-hop: a1 -> a2 -> a3
        supersede(&conn, "a2", "a3");
        supersede(&conn, "d1", "d2"); // ends deleted
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 5 WHERE id = 'd2'",
            [],
        )
        .unwrap();
        supersede(&conn, "c1", "c2"); // cycle
        supersede(&conn, "c2", "c1");
        supersede(&conn, "x1", "x2"); // head already excluded
        let m = run(
            &conn,
            None,
            0,
            &["a1", "d1", "c1", "x1", "x2", "plain", "unknown"],
        );
        assert_eq!(ids(&m.corrections), vec!["a3"]);
        assert_eq!(m.corrections[0].supersedes, vec!["a1"]);
    }

    #[test]
    fn two_excludes_to_one_head_merge() {
        let conn = open_in_memory().unwrap();
        for id in ["o1", "o2", "head"] {
            seed(&conn, id, "ent", &at(0.1));
        }
        supersede(&conn, "o1", "head");
        supersede(&conn, "o2", "head");
        let m = run(&conn, None, 0, &["o2", "o1", "o2"]);
        assert_eq!(ids(&m.corrections), vec!["head"]);
        assert_eq!(m.corrections[0].supersedes, vec!["o2", "o1"]);
    }

    #[test]
    fn chain_longer_than_max_depth_yields_nothing() {
        let conn = open_in_memory().unwrap();
        let n = MAX_CHAIN_DEPTH + 2;
        for i in 0..n {
            seed(&conn, &format!("f{i}"), "ent", &at(0.1));
        }
        for i in 0..n - 1 {
            supersede(&conn, &format!("f{i}"), &format!("f{}", i + 1));
        }
        assert!(run(&conn, None, 0, &["f0"]).corrections.is_empty());
        // within the bound it resolves
        let m = run(&conn, None, 0, &["f2"]);
        assert_eq!(ids(&m.corrections), vec![format!("f{}", n - 1).as_str()]);
    }

    #[test]
    fn correction_head_is_not_duplicated_into_entries() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "old", "ent", &Q);
        seed(&conn, "new", "ent", &Q); // also clears the floor
        seed(&conn, "next", "ent", &at(0.9));
        supersede(&conn, "old", "new");
        let m = run(&conn, Some(0.5), 1, &["old"]);
        assert_eq!(ids(&m.corrections), vec!["new"]);
        assert_eq!(ids(&m.entries), vec!["next"]); // backfilled
    }

    #[test]
    fn pre_v24_table_matches_without_corrections() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (
                id TEXT PRIMARY KEY, entity_id TEXT NOT NULL, title TEXT, body TEXT,
                source_type TEXT, embedding_blob BLOB, deleted_at INTEGER)",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries VALUES ('a', 'ent', 'T', 'B', 'user_stated', ?1, NULL)",
            params![blob(&Q)],
        )
        .unwrap();
        let m = run(&conn, Some(0.5), 5, &["a", "b"]);
        assert!(m.corrections.is_empty());
        assert!(m.entries.is_empty()); // "a" excluded
        let m = run(&conn, Some(0.5), 5, &[]);
        assert_eq!(ids(&m.entries), vec!["a"]);
        assert_eq!(m.entries[0].provenance.as_deref(), Some("user_stated"));
    }

    #[test]
    fn empty_query_vec_yields_no_entries() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "a", "ent", &Q);
        let m =
            wisdom_match_with_floor(&conn, &[], KEY, Scheme::Raw, Some(0.1), 5, &[], NOW).unwrap();
        assert!(m.entries.is_empty());
    }

    #[test]
    fn provenance_vocabulary() {
        for v in PROVENANCE_VOCAB {
            assert_eq!(provenance_for(Some(v)).as_deref(), Some(*v));
        }
        assert_eq!(provenance_for(Some("agent_magic")), None);
        assert_eq!(provenance_for(None), None);
    }

    #[test]
    fn gate_keys() {
        let local = EmbedProfile::Local {
            model: "Nomic-Embed-Code".into(),
        };
        assert_eq!(gate_model_key(&local, None), "local:nomic-embed-code");
        assert_eq!(gate_model_key(&local, Some("constant8")), "stub:constant8");
        assert_eq!(gate_model_key(&local, Some("")), "local:nomic-embed-code");
        let cloud = EmbedProfile::Cloud {
            provider: CloudProvider::OpenAi,
            model: "text-embedding-3-small".into(),
            api_key: String::new(),
        };
        assert_eq!(
            gate_model_key(&cloud, None),
            "cloud:open_ai:text-embedding-3-small"
        );
        let ext = EmbedProfile::External {
            profile: ExternalEmbedProfile {
                base_url: "https://x".into(),
                model: "Qwen3-Embed".into(),
                api_key: None,
            },
        };
        assert_eq!(gate_model_key(&ext, None), "external:qwen3-embed");
        assert_eq!(gate_floor("stub:constant8"), Some(0.5));
        assert_eq!(gate_floor("external:qwen/qwen3-embedding-4b"), Some(0.70));
        assert_eq!(gate_floor("local:nomic-embed-code"), None);
    }

    #[test]
    fn gate_keys_per_scheme() {
        let ext = EmbedProfile::External {
            profile: ExternalEmbedProfile {
                base_url: "https://x".into(),
                model: "qwen/qwen3-embedding-4b".into(),
                api_key: None,
            },
        };
        assert_eq!(
            gate_model_key_for_scheme(&ext, None, Scheme::Raw),
            "external:qwen/qwen3-embedding-4b"
        );
        assert_eq!(
            gate_model_key_for_scheme(&ext, None, Scheme::Instr1),
            "external:qwen/qwen3-embedding-4b:instr1"
        );
        // The stub overrides the profile whatever the scheme.
        assert_eq!(
            gate_model_key_for_scheme(&ext, Some("constant8"), Scheme::Instr1),
            "stub:constant8:instr1"
        );
        // The stub gate survives the cutover: its floor is registered under
        // both schemes, so stub-backed runs never silently abstain post-flip.
        for scheme in [Scheme::Raw, Scheme::Instr1] {
            assert_eq!(
                gate_floor(&gate_model_key_for_scheme(&ext, Some("constant8"), scheme)),
                Some(0.5),
                "{scheme:?}"
            );
        }
        // Both calibrated floors resolve through the scheme-aware key.
        assert_eq!(
            gate_floor(&gate_model_key_for_scheme(&ext, None, Scheme::Raw)),
            Some(0.70)
        );
        assert_eq!(
            gate_floor(&gate_model_key_for_scheme(&ext, None, Scheme::Instr1)),
            Some(0.64)
        );
    }

    #[test]
    fn fact_ids_and_truncation() {
        assert!(valid_fact_id("fact_0123456789abcdef01234567"));
        assert!(valid_fact_id("-leading-dash"));
        for bad in ["", "a b", "a/b", &"x".repeat(129)] {
            assert!(!valid_fact_id(bad), "{bad:?}");
        }
        let long = "é".repeat(MAX_TEXT_CHARS + 5);
        assert_eq!(truncate_text(&long).chars().count(), MAX_TEXT_CHARS);
        assert_eq!(truncate_text("short"), "short");
    }

    // ---- Read-scheme coupling (spec §Scheme architecture; tests b + d) ----

    /// `llm_wiki_meta` carries the active read scheme in migrated DBs; set it
    /// through the vocabulary constant so the tests fail with it if the key
    /// ever drifts.
    fn set_active_scheme(conn: &Connection, value: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![crate::embed_scheme::ACTIVE_SCHEME_META_KEY, value],
        )
        .unwrap();
    }

    /// Test (b), gate leg: a dual-stamped DB — `raw` and `instr1` rows in the
    /// same table — must surface only the ACTIVE scheme's rows to the gate,
    /// under both meta values, with identical vectors (the filter, not the
    /// score, decides).
    #[test]
    fn gate_sees_only_active_scheme_rows_dual_stamped() {
        let conn = open_in_memory().unwrap();
        // `seed` lands under the default 'raw' stamp; flip the second row to
        // `instr1` so the table is dual-stamped with identical vectors.
        seed(&conn, "raw_row", "ent", &Q);
        seed(&conn, "instr_row", "ent", &Q);
        conn.execute(
            "UPDATE llm_wiki_entries SET embed_scheme = 'instr1' WHERE id = 'instr_row'",
            [],
        )
        .unwrap();

        set_active_scheme(&conn, "raw");
        let m = wisdom_match(&conn, &Q, KEY_EXT, 5, &[], NOW).unwrap();
        assert_eq!(ids(&m.entries), vec!["raw_row"]);

        set_active_scheme(&conn, "instr1");
        let m = wisdom_match(&conn, &Q, KEY_EXT, 5, &[], NOW).unwrap();
        assert_eq!(ids(&m.entries), vec!["instr_row"]);
    }

    /// Test (d), coupling: raw → floor 0.70 (raw qwen key) + raw rows +
    /// unprefixed query; instr1 → floor 0.64 (`:instr1` key) + instr1 rows +
    /// prefixed query; an unknown meta value is a hard error (fail-closed).
    /// The floor is asserted structurally: under raw, a 0.65-cosine row
    /// (above 0.64, below 0.70) opens the gate only when the raw floor is the
    /// one selected; flipping the meta to instr1 must let the same row
    /// through. The prefix mode rides `query_text_for_scheme`, the exact
    /// function the `ct wisdom match` embed call uses.
    #[test]
    fn read_scheme_couples_floor_rows_and_prefix() {
        use crate::embed_scheme::query_text_for_scheme;

        let conn = open_in_memory().unwrap();
        // Row sitting strictly between the two floors.
        seed(&conn, "between", "ent", &at(0.65));

        set_active_scheme(&conn, "raw");
        // raw floor is 0.70 → the 0.65 row is gated out.
        let m = wisdom_match(&conn, &Q, KEY_EXT, 5, &[], NOW).unwrap();
        assert!(m.entries.is_empty());
        // …and the calibration key that produced that floor is the raw one.
        assert_eq!(gate_floor(KEY_EXT), Some(0.70));
        assert_eq!(query_text_for_scheme("hello", Scheme::Raw), "hello");

        set_active_scheme(&conn, "instr1");
        // Under instr1 the floor is 0.64 but the row's stamp is `raw`, so it
        // is STILL gated out — the filter, not the floor, decides (raw →
        // 0.70/raw rows/unprefixed and instr1 → 0.64/instr1 rows/prefixed
        // must not mix).
        let m = wisdom_match(&conn, &Q, KEY_EXT, 5, &[], NOW).unwrap();
        assert!(m.entries.is_empty());
        // Flip the row's stamp to instr1 and NOW the same vector clears the
        // 0.64 floor: floor and filter moved together with the meta value.
        conn.execute(
            "UPDATE llm_wiki_entries SET embed_scheme = 'instr1' WHERE id = 'between'",
            [],
        )
        .unwrap();
        let m = wisdom_match(&conn, &Q, KEY_EXT, 5, &[], NOW).unwrap();
        assert_eq!(ids(&m.entries), vec!["between"]);
        assert_eq!(
            gate_floor(&floor_key_for(KEY_EXT, Scheme::Instr1)),
            Some(0.64)
        );
        assert_eq!(
            query_text_for_scheme("hello", Scheme::Instr1),
            format!("{}hello", crate::embed_scheme::QUERY_INSTRUCTION_PREFIX)
        );
    }

    /// Single-resolution legs: a caller-resolved scheme (`_in_scheme`) or a
    /// caller-declared one (`_with_floor`) drives the row filter — never the
    /// meta value as it stands when the SELECT runs. A cutover landing
    /// between the caller's resolution and the gate cannot pair the caller's
    /// query/floor with the other scheme's rows.
    #[test]
    fn caller_resolved_scheme_drives_the_filter_not_the_meta() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "raw_row", "ent", &Q);
        seed(&conn, "instr_row", "ent", &Q);
        conn.execute(
            "UPDATE llm_wiki_entries SET embed_scheme = 'instr1' WHERE id = 'instr_row'",
            [],
        )
        .unwrap();
        // The caller resolved raw; the meta has since flipped to instr1.
        set_active_scheme(&conn, "instr1");
        let m = wisdom_match_in_scheme(&conn, &Q, KEY_EXT, Scheme::Raw, 5, &[], NOW).unwrap();
        assert_eq!(ids(&m.entries), vec!["raw_row"]);
        assert_eq!(m.gate, format!("semantic-v1:{KEY_EXT}"));
        let m =
            wisdom_match_with_floor(&conn, &Q, KEY, Scheme::Raw, Some(0.5), 5, &[], NOW).unwrap();
        assert_eq!(ids(&m.entries), vec!["raw_row"]);
        // And the reverse: declared instr1 under a raw meta.
        set_active_scheme(&conn, "raw");
        let m = wisdom_match_with_floor(&conn, &Q, KEY, Scheme::Instr1, Some(0.5), 5, &[], NOW)
            .unwrap();
        assert_eq!(ids(&m.entries), vec!["instr_row"]);
    }

    /// Declaring `instr1` against a pre-V27 table (rows are de-facto raw) is
    /// a hard error, never a silent cross-scheme score.
    #[test]
    fn declared_instr1_on_pre_v27_table_fails_closed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (
                id TEXT PRIMARY KEY, entity_id TEXT NOT NULL, title TEXT, body TEXT,
                source_type TEXT, embedding_blob BLOB, deleted_at INTEGER)",
        )
        .unwrap();
        let err = wisdom_match_with_floor(&conn, &Q, KEY, Scheme::Instr1, Some(0.5), 5, &[], NOW)
            .unwrap_err();
        assert!(err.to_string().contains("fail-closed"), "{err}");
    }

    /// Test (d), fail-closed leg: an unknown `wisdom_active_scheme` value is a
    /// hard error on the gate path — never a fallback to another scheme's
    /// floor or filter.
    #[test]
    fn unknown_active_scheme_is_hard_error_on_gate_path() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "raw_row", "ent", &Q);
        set_active_scheme(&conn, "bogus-scheme");
        let err = wisdom_match(&conn, &Q, KEY, 5, &[], NOW).unwrap_err();
        assert!(
            err.to_string().contains("bogus-scheme"),
            "error should name the scheme: {err}"
        );
    }

    /// Pre-V27 shape (no `embed_scheme` column): the filter degrades off and
    /// rows are read verbatim, mirroring the temporal-column degradation.
    #[test]
    fn pre_v27_table_reads_without_scheme_filter() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (
                id TEXT PRIMARY KEY, entity_id TEXT NOT NULL, title TEXT, body TEXT,
                source_type TEXT, embedding_blob BLOB, deleted_at INTEGER)",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries VALUES ('a', 'ent', 'T', 'B', 'user_stated', ?1, NULL)",
            params![blob(&Q)],
        )
        .unwrap();
        let m = run(&conn, Some(0.5), 5, &[]);
        assert_eq!(ids(&m.entries), vec!["a"]);
    }

    /// THE shared eligibility predicate (spec §3.2): `eligible_entry_ids` and
    /// `gated_entries` are two consumers of ONE fragment, so the v1 gate and
    /// the stage-1 SQL can never disagree about which facts are eligible.
    /// One row per predicate arm is live here; everything else must drop from
    /// BOTH consumers identically.
    #[test]
    fn eligibility_predicate_matches_gated_entries_exactly() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "live", "ent", &Q);
        // Second live row so the soft-delete UPDATE below exercises a real
        // exclusion (the plan sketch mutated an id it never seeded).
        seed(&conn, "live2", "ent", &at(0.5));
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'live2'",
            [],
        )
        .unwrap();
        seed_deleted(&conn, "dead");
        seed_null_title(&conn, "notitle");
        seed_superseded(&conn, "old", "live");
        seed_expired(&conn, "expired");

        let temporal = temporal_columns(&conn, None).unwrap();
        let pred_ids: HashSet<String> = eligible_entry_ids(&conn, &temporal, NOW)
            .unwrap()
            .into_iter()
            .collect();
        let gated = gated_entries(
            &conn,
            &Q,
            Some(0.0),
            10,
            &HashSet::new(),
            NOW,
            &temporal,
            None,
            None,
        )
        .unwrap();
        let gated_ids: HashSet<String> = gated.iter().map(|e| e.id.clone()).collect();
        assert_eq!(
            gated_ids, pred_ids,
            "one predicate, two consumers — but they disagree"
        );
        // Sanity: the exclusions behave identically in both consumers.
        // DEVIATION (recorded at seed_null_title): the live schema pins
        // title NOT NULL, so the "notitle" arm seeds an empty title that is
        // ELIGIBLE under the shared predicate; "dead"/"old"/"expired"/"live2"
        // are the excluded arms.
        assert_eq!(
            pred_ids,
            HashSet::from(["live".to_string(), "notitle".to_string()])
        );
    }

    /// Parity guard above the `MAX_EXCLUDES` cliff (spec §3.1 r30-M2): the
    /// per-call skip set lives in Rust on BOTH paths, so with MORE than
    /// `MAX_EXCLUDES` excludes the v1 gate and the stage-1 complement
    /// computation (`eligible_entry_ids` + Rust filter) must skip exactly the
    /// same ids — an oversize exclude list can never desync the stages.
    #[test]
    fn stage1_predicate_parity_above_1024_excludes() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "a", "ent", &Q);
        seed(&conn, "b", "ent", &at(0.8));
        let excludes: Vec<String> = (0..MAX_EXCLUDES + 6)
            .map(|i| format!("x{i}"))
            .chain(["a".into()])
            .collect();
        let temporal = temporal_columns(&conn, None).unwrap();
        let skip: HashSet<String> = excludes.iter().cloned().collect();
        let after_skip: Vec<String> = eligible_entry_ids(&conn, &temporal, NOW)
            .unwrap()
            .into_iter()
            .filter(|id| !skip.contains(id))
            .collect();
        assert_eq!(after_skip, vec!["b".to_string()]);
        // v1 path with the same oversize skip set:
        let gated =
            gated_entries(&conn, &Q, Some(0.0), 10, &skip, NOW, &temporal, None, None).unwrap();
        assert_eq!(
            gated.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
            vec!["b".to_string()]
        );
    }

    // ===================== Task 4: two-stage decision core =====================
    //
    // Fixtures reuse Task 3's `two_stage::two_stage_test_support::fixture()`
    // (one fixture, one edge model). Its layout, pinned there:
    //   doc1 (stub key, chunk c1 `h-c1`, vector u1) — candidate via evidence
    //        hash hop; f1's evidence names `h-c1` ⇒ f1 rule-(ii) reachable.
    //   doc2 (stub key, chunk c2, vector u1) — candidate via proposal chain
    //        (p1 evidence leg); ZERO member chunks ⇒ rule-(ii)-dark.
    //   doc3 (`old-model` key, chunk c3, no embeddings row) — NOT candidate.
    //   doc4 (NULL key, no chunks) — not in any population (r26-M2).
    //   entries: f1 (librarian_inferred, blob u4, evidence h-c1, proposal p1),
    //            f2 (user_stated, blob u4, no edges), f3 (librarian_inferred,
    //            blob u4, dangling evidence hash, proposal p1).
    // The stub floor under both keys is 0.5 (WISDOM_GATE_FLOORS).

    use crate::two_stage::{blob as ts_blob, fixture, unit_vec, NOW_MS as TS_NOW};

    const TS_KEY: &str = "stub:constant8";

    /// Two-stage call through [`two_stage_entries`] with the fixture's shapes:
    /// 4-dim raw + schemed vectors, raw scheme, no excludes/corrections.
    fn ts_run(
        conn: &Connection,
        query_raw: &[f32],
        query_schemed: &[f32],
        v1_floor: Option<f32>,
        ts_floor: Option<f32>,
        max: usize,
    ) -> crate::wisdom_match::TwoStageResult {
        two_stage_entries(
            conn,
            query_raw,
            query_schemed,
            Scheme::Raw,
            v1_floor,
            ts_floor,
            max,
            &HashSet::new(),
            &HashSet::new(),
            TS_NOW,
            TS_KEY,
            QueryKind::Message,
        )
        .unwrap()
    }

    /// §4 matrix cell 1: two-stage floor MISSING + v1 floor present ⇒ v1
    /// fallback — v1 entries, and NO two-stage rule was evaluated (§6 m4:
    /// `rule=none` is reserved for exactly these non-two-stage opens).
    #[test]
    fn missing_two_stage_floor_falls_back_to_v1() {
        let conn = fixture();
        // u1 matches c1/c2 and — through the doc hop — f1's blob is u4, which
        // is ORTHOGONAL to u1: with only the v1 path scored, no two-stage
        // fact can pass. The v1 floor admits nothing either (u4 ⊥ u1 and
        // u4 ⊥ the query), so assert on the fallback SHAPE: rule None, empty
        // two-stage bookkeeping — and, against a REAL v1 open, v1 entries.
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(1), Some(0.5), None, 8);
        assert!(
            outcome.rule.is_none(),
            "missing two-stage floor ⇒ no two-stage rule evaluated"
        );
        assert!(!outcome.open, "nothing cleared any floor ⇒ closed");
        assert!(entries.is_empty());
        // The outcome carries the v1 fallback's floor key: the current v1 key.
        assert_eq!(outcome.floor_key_used, TS_KEY);
        assert_eq!(outcome.v1merged, 0);
    }

    /// §4 cell 2: BOTH floors missing ⇒ uncalibrated (empty entries, no rule);
    /// the CALLER asserts the `uncalibrated` label — here we pin the shape the
    /// caller branches on: `floor_key_used` empty, rule None, open false.
    #[test]
    fn both_floors_missing_is_uncalibrated_no_embed() {
        let conn = fixture();
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(1), None, None, 8);
        assert!(outcome.rule.is_none());
        assert!(!outcome.open);
        assert!(entries.is_empty());
        assert_eq!(outcome.floor_key_used, "", "uncalibrated ⇒ no floor key");
        assert_eq!(outcome.v1merged, 0);
    }

    /// §4 cell 1 + §6 guard: one stale doc in the candidate set (doc2 keeps
    /// its `stub:constant8` key; doc1's is stale here) ⇒ coverage < 100% ⇒
    /// v1 fallback, and the audit outcome reports HOW stale the set is.
    #[test]
    fn coverage_below_100_falls_back_with_stale_count() {
        let conn = fixture();
        conn.execute(
            "UPDATE documents SET embed_key = 'old-model' WHERE id = 1",
            [],
        )
        .unwrap();
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(1), Some(0.5), Some(0.5), 8);
        assert!(outcome.rule.is_none(), "low coverage ⇒ no rule evaluated");
        assert!(!outcome.open);
        assert!(entries.is_empty(), "fixture facts score 0 against u1");
        assert_eq!(outcome.stale_embed_key, 1, "one stale candidate doc");
        assert_eq!(outcome.floor_key_used, TS_KEY);
    }

    /// §4 cell 3 (the dark-adjacent cell, BY DESIGN): two-stage floor present
    /// + v1 floor MISSING + coverage < 100% ⇒ uncalibrated no-open.
    #[test]
    fn fourth_cell_two_stage_floor_only_plus_low_coverage_is_uncalibrated() {
        let conn = fixture();
        conn.execute(
            "UPDATE documents SET embed_key = 'old-model' WHERE id = 1",
            [],
        )
        .unwrap();
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(1), None, Some(0.5), 8);
        assert!(outcome.rule.is_none(), "uncalibrated ⇒ rule None (§6 m4)");
        assert!(!outcome.open);
        assert!(entries.is_empty());
        assert_eq!(outcome.stale_embed_key, 1);
    }

    /// Task 5: the orchestrator's ran-path label. In test builds the stub's
    /// composite floors ARE resolvable (`two_stage_rule_floor` sees the
    /// test-only slice), so with coverage 100% two-stage RUNS and the caller
    /// prints `semantic-v2-two-stage:{scheme_key}` — this pins the label the
    /// CLI emits when `outcome.rule.is_some()` (the CLI integration tests
    /// compile without cfg(test) and can only reach the cell-1 fallback).
    #[test]
    fn orchestrator_emits_two_stage_label_when_it_runs() {
        let conn = fixture();
        let run = wisdom_match_two_stage_with_v1(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            8,
            &[],
            TS_NOW,
            TS_KEY,
            QueryKind::Message,
        )
        .unwrap();
        assert!(
            run.ran,
            "stub composite floor + full coverage ⇒ two-stage runs"
        );
        assert_eq!(
            run.outcome.rule,
            Some(TwoStageRule::RestrictedCosine),
            "pinned order: restricted-cosine floor wins attribution"
        );
        assert_eq!(
            run.result.gate,
            format!("{TWO_STAGE_LABEL_PREFIX}{TS_KEY}"),
            "ran ⇒ semantic-v2-two-stage:{TS_KEY}"
        );
    }

    /// Task 5: BOTH floors missing ⇒ the uncalibrated no-embed cell — empty
    /// result, `uncalibrated` gate, rule=none, floor_key_used empty (the
    /// caller's audit line prints `floor=`). Uses the never-calibrated
    /// external key (no v1 floor, no composite floor anywhere) instead of
    /// mutating the const floor table.
    #[test]
    fn orchestrator_uncalibrated_cell_reports_rule_none_and_empty_floor_key() {
        let conn = fixture();
        let run = wisdom_match_two_stage_with_v1(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            8,
            &[],
            TS_NOW,
            "external:never-calibrated-model",
            QueryKind::Message,
        )
        .unwrap();
        assert!(!run.ran);
        assert_eq!(run.outcome.rule, None);
        assert_eq!(run.outcome.floor_key_used, "");
        assert!(run.result.entries.is_empty());
        assert_eq!(run.result.gate, GATE_UNCALIBRATED);
    }

    /// Task-5 fix round (reviewer Important #1/#2): the PUB pre-embed
    /// precheck — the shared §3 embed-skip authority. Every caller (CLI,
    /// Task-14/15 harness) invokes it BEFORE any embed_one and branches its
    /// embed logic on the projected §4 cell + the coverage probe, so §6's
    /// "coverage check BEFORE any embed" holds on ALL caller paths.
    /// Precheck on the fixture, BOTH floors present (post-flip 2026-10-18):
    /// cell-3 projected (coverage veto), schemed-only embeds, both candidate
    /// docs stale (fixture coverage 0/2 — its docs carry a non-matching
    /// qwen embed_key). Uses the real qwen key — pre-flip this world was
    /// v1-only (cell 1); the #265 flip added the composite chunk-hop floor,
    /// so the same precheck now projects the coverage-veto cell.
    #[test]
    fn precheck_both_floors_stale_coverage_projects_cell3_veto() {
        let conn = fixture();
        let pre = two_stage_pre_embed_check(
            &conn,
            "external:qwen/qwen3-embedding-4b",
            Scheme::Raw,
            TS_NOW,
            QueryKind::Message,
        )
        .unwrap();
        assert!(pre.v1_floor_present, "qwen raw key has a v1 floor (0.70)");
        assert!(
            pre.ts_floor_present,
            "composite chunk-hop floor flipped in 2026-10-18 (issue #265)"
        );
        assert!(!pre.coverage_probe_miss);
        assert_eq!(
            pre.projected_cell, 3,
            "both floors + partial fixture coverage ⇒ §4 cell 3 (coverage veto; \
             the fixture's candidate docs carry no qwen embed_key)"
        );
        assert_eq!(
            pre.embed_plan,
            TwoStageEmbedPlan::SchemedOnly,
            "cell 3 with the v1 floor present still needs the schemed vector \
             for the v1-fallback run"
        );
        assert_eq!(
            pre.stale_embed_key, 2,
            "both fixture candidate docs are stale against the qwen key"
        );
    }

    /// Precheck, BOTH floors missing: cell 2, no embeds at all — the shared
    /// authority the caller branches on for the §3 uncalibrated skip.
    #[test]
    fn precheck_both_floors_missing_projects_cell2_no_embeds() {
        let conn = fixture();
        let pre = two_stage_pre_embed_check(
            &conn,
            "external:never-calibrated-model",
            Scheme::Raw,
            TS_NOW,
            QueryKind::Message,
        )
        .unwrap();
        assert!(!pre.v1_floor_present);
        assert!(!pre.ts_floor_present);
        assert_eq!(pre.projected_cell, 2, "both missing ⇒ §4 cell 2");
        assert_eq!(pre.embed_plan, TwoStageEmbedPlan::None);
    }

    /// Precheck, composite floor only + stale candidate (coverage < 100%):
    /// §4 cell 3 (the dark-adjacent cell) projected BEFORE any embed — this
    /// is the ordering Important #1 pins. With the probe able to answer, the
    /// stale count rides along.
    #[test]
    fn precheck_cell3_low_coverage_projects_before_any_embed() {
        let conn = fixture();
        conn.execute(
            "UPDATE documents SET embed_key = 'old-model' WHERE id = 1",
            [],
        )
        .unwrap();
        let pre = two_stage_pre_embed_check(&conn, TS_KEY, Scheme::Raw, TS_NOW, QueryKind::Message)
            .unwrap();
        assert!(pre.v1_floor_present);
        assert!(
            pre.ts_floor_present,
            "test builds see the composite slice ⇒ composite floor present"
        );
        assert!(!pre.coverage_probe_miss);
        assert_eq!(pre.projected_cell, 3, "floors present + cov<100% ⇒ cell 3");
        assert_eq!(
            pre.stale_embed_key, 1,
            "doc1 stale ⇒ one stale candidate doc"
        );
        // Cell 3 with the v1 floor PRESENT ⇒ the orchestrator runs FULL v1
        // as the fallback ⇒ the schemed vector IS needed (a v1-missing
        // caller would get None here — pinned by the both-missing test).
        assert_eq!(pre.embed_plan, TwoStageEmbedPlan::SchemedOnly);
    }

    /// Precheck, all floors present + full coverage: §4 cell 4 — embedding
    /// proceeds; under Raw the schemed vector REUSES the raw one (r26 m3),
    /// so the plan is raw+schemed with ONE embed, not two.
    #[test]
    fn precheck_cell4_full_coverage_plans_stage1_embed_raw_reuse() {
        let conn = fixture();
        let pre = two_stage_pre_embed_check(&conn, TS_KEY, Scheme::Raw, TS_NOW, QueryKind::Message)
            .unwrap();
        assert!(pre.v1_floor_present);
        assert!(pre.ts_floor_present);
        assert_eq!(pre.projected_cell, 4);
        assert_eq!(
            pre.embed_plan,
            TwoStageEmbedPlan::RawAndSchemed,
            "cell 4 ⇒ stage-1 raw + schemed (Raw reuses under embed, still both vecs)"
        );
        assert_eq!(pre.stale_embed_key, 0);
    }

    /// Probe miss (pre-V28 shape): the precheck reports the miss, projects
    /// the §4 fallback (no coverage veto possible), and `stale_embed_key`
    /// degrades to 0 — the probe cannot answer (r28-M3 semantics the audit
    /// line also carries).
    #[test]
    fn precheck_probe_miss_reports_miss_and_zero_stale() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE documents (id INTEGER PRIMARY KEY, path TEXT);")
            .unwrap();
        let pre = two_stage_pre_embed_check(&conn, TS_KEY, Scheme::Raw, TS_NOW, QueryKind::Message)
            .unwrap();
        assert!(pre.coverage_probe_miss);
        assert_eq!(
            pre.projected_cell, 1,
            "no coverage info ⇒ same as v1-only here"
        );
        assert_eq!(pre.stale_embed_key, 0, "probe can't answer ⇒ 0");
        assert_eq!(
            pre.embed_plan,
            TwoStageEmbedPlan::SchemedOnly,
            "v1 floor exists ⇒ the fallback still needs the schemed vector"
        );
    }

    /// Rule (i): with coverage 100% and the restricted-cosine floor
    /// calibrated, the best restricted-set FACT cosine (against the
    /// SCHEME-PREFIXED vector) opens the gate; `top` is that fact cosine
    /// (r28-m3) — NOT the stage-1 chunk score (here u1, also 1.0, but the
    /// distinct-vector case in `audit_outcome_fields_pinned` separates them).
    #[test]
    fn rule_i_opens_on_restricted_fact_cosine() {
        let conn = fixture();
        // Align the fact vector: f1/f2/f3 carry u4; a u4 query scores them 1.0
        // (raw ≥ 0.5), and the stage-1 chunks (u1) score 0 — proving `top`
        // comes from the FACT cosine, not the chunk path.
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(4), Some(0.5), Some(0.5), 8);
        assert_eq!(outcome.rule, Some(TwoStageRule::RestrictedCosine));
        assert!(outcome.open, "best restricted fact cosine 1.0 ≥ floor 0.5");
        let got = (outcome.top.unwrap() - 1.0).abs() < 1e-6;
        assert!(
            got,
            "top = best restricted FACT cosine, got {:?}",
            outcome.top
        );
        assert!(!entries.is_empty());
        assert_eq!(
            outcome.floor_key_used,
            format!("{TS_KEY}:two-stage:restricted-cosine")
        );
        assert_eq!(outcome.stale_embed_key, 0);
        // f2 is complement (no edges) ⇒ merge path; f1 is rule-(i) reachable.
        assert!(
            entries.iter().any(|e| e.id == "f1"),
            "rule-(i)-reachable fact scored"
        );
        assert_eq!(outcome.v1merged, 1, "f2 admitted by the v1-merge");
    }

    /// r25-M4 / r26 m2: under rule (ii) the chunk-score floor is the ONLY
    /// abstention test — facts of the opened doc are injected even when their
    /// raw cosine is BELOW the v1 floor (the `raw < floor` drop is bypassed).
    #[test]
    fn rule_ii_bypasses_the_fact_floor() {
        let conn = fixture();
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'f1'",
            rusqlite::params![ts_blob(&unit_vec(2))],
        )
        .unwrap();
        // Query u1: chunk c1 (a MEMBER chunk, h-c1 in f1's evidence) scores
        // 1.0 ≥ chunk-hop floor; f1's fact vector u2 is ORTHOGONAL to u1 —
        // far below ANY floor — yet rule (ii) must inject it.
        let (entries, outcome) =
            ts_run(&conn, &unit_vec(1), &unit_vec(1), Some(0.9), Some(0.45), 8);
        assert_eq!(outcome.rule, Some(TwoStageRule::ChunkHop));
        assert!(outcome.open);
        let f1 = entries.iter().find(|e| e.id == "f1").expect("f1 injected");
        let raw = f1.score.unwrap();
        assert!(
            raw < 0.1,
            "f1 raw cosine must be far below the v1 floor 0.9, got {raw}"
        );
        assert_eq!(
            outcome.v1merged, 0,
            "u1 ⊥ u4 ⇒ the merge admits nothing (f2 below the v1 floor)"
        );
    }

    /// r30-M3: rule (ii)'s doc→facts hop is the EVIDENCE-HASH edge, not the
    /// proposal union. A probe matching trigger doc B must NOT open doc A's
    /// facts under rule (ii). Fixture mapping for this test: f1's evidence is
    /// repointed at doc2's chunk hash ⇒ doc2's c2 becomes the (only) MEMBER
    /// chunk (c1 loses membership — no evidence names it), and f1's blob
    /// moves to u3 so no cosine leg scores it. The opened doc is doc2; f3 is
    /// carried by doc2 ONLY through the proposal leg — under a (wrong) union
    /// hop it would open with no fact floor; under the evidence-hash edge it
    /// must NOT appear.
    #[test]
    fn rule_ii_uses_evidence_hash_edge_not_proposal_union() {
        let conn = fixture();
        conn.execute_batch(
            "UPDATE chunks SET content_hash = 'h-c2' WHERE id = 2;
             UPDATE librarian_evidence SET evidence_json = '{\"evidence\":[{\"content_hash\":\"h-c2\"}]}' WHERE entry_id = 'f1';",
        )
        .unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'f1'",
            rusqlite::params![ts_blob(&unit_vec(3))],
        )
        .unwrap();
        let mut exclude: HashSet<String> = HashSet::new();
        exclude.insert("f1".to_string()); // keep the opened doc's own fact out of ENTRIES
        let (entries, outcome) = two_stage_entries(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            Some(0.5),
            Some(0.45),
            8,
            &exclude,
            &HashSet::new(),
            TS_NOW,
            TS_KEY,
            QueryKind::Message,
        )
        .unwrap();
        assert_eq!(outcome.rule, Some(TwoStageRule::ChunkHop));
        assert!(
            !entries.iter().any(|e| e.id == "f3"),
            "proposal-union fan-out must NOT open under rule (ii)"
        );
        // `top` is the best MEMBER chunk score: c2 at 1.0 (c1 is not a member).
        assert!((outcome.top.unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(outcome.v1merged, 0, "u1 ⊥ u4 ⇒ the merge admits nothing");
    }

    /// r26-M1: an ALL-`user_stated` brain has NO candidate docs, but with a
    /// v1 floor present the §3 v1-merge scores the complement facts and the
    /// gate STAYS OPENABLE — `v1merged ≥ 1`.
    #[test]
    fn v1_merge_keeps_source_less_brain_openable() {
        let conn = open_in_memory().unwrap();
        // user_stated fact, blob u4; no documents/chunks/evidence at all.
        conn.execute(
            "INSERT INTO llm_wiki_entries
               (id, entity_id, title, body, source_type, created_at, updated_at, embedding_blob)
             VALUES ('s1','e','T','B','user_stated',0,0,?1)",
            rusqlite::params![ts_blob(&unit_vec(4))],
        )
        .unwrap();
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(4), Some(0.5), Some(0.5), 8);
        // Empty candidate set = vacuous coverage ⇒ two-stage still runs.
        assert!(
            entries.iter().any(|e| e.id == "s1"),
            "merge must open the gate"
        );
        assert!(outcome.open);
        assert_eq!(outcome.v1merged, 1);
        // The stage-1 arm found nothing above ITS floor (no chunks at all).
        assert_eq!(outcome.rule, Some(TwoStageRule::RestrictedCosine));
        assert_eq!(
            outcome.top, None,
            "no restricted fact above floor ⇒ top None"
        );
    }

    /// r27-M1: a ZERO-CHUNK doc's facts are in the structural complement by
    /// definition (sourced, but no stage-1-scorable chunk) — a provenance
    /// merge would carry them; the structural one MUST too. Fixture: a doc
    /// with an evidence edge but zero chunks, carrying a fact.
    #[test]
    fn zero_chunk_doc_facts_open_via_merge() {
        let conn = open_in_memory().unwrap();
        // FK order matters: the entry BEFORE its librarian_evidence row.
        conn.execute_batch(
            "INSERT INTO documents (path, hash, status, tier, embed_key)
               VALUES ('/v/z.md','hz','indexed','user_doc','stub:constant8');
             INSERT INTO llm_wiki_entries
               (id, entity_id, title, body, source_type, created_at, updated_at)
             VALUES ('fz','e','T','B','librarian_inferred',0,0);
             INSERT INTO curated_proposals (id, kind, model, status, created_at)
               VALUES ('pz','new_entity','test','approved',0);
             INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
               VALUES ('pz', 1, 'evidence');
             INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at)
               VALUES ('fz','pz','{\"evidence\":[{\"content_hash\":\"h-z\"}]}',0)",
        )
        .unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fz'",
            rusqlite::params![ts_blob(&unit_vec(4))],
        )
        .unwrap();
        // u4 query scores fz at 1.0 — but fz has NO chunk, so stage 1 can
        // never score it; only the merge can carry it.
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(4), Some(0.5), Some(0.5), 8);
        assert!(
            entries.iter().any(|e| e.id == "fz"),
            "zero-chunk doc's fact must open via the merge"
        );
        assert!(outcome.open);
        assert_eq!(outcome.v1merged, 1);
        // Coverage population: the zero-chunk doc is NOT counted (r26-M2).
        assert_eq!(outcome.stale_embed_key, 0);
    }

    /// r29-m2: the corrections arrays are identical under two-stage and v1
    /// (the caller computes them UNCHANGED and passes the absent ids in), and
    /// no correction head appears among the entries under either path.
    #[test]
    fn corrections_identical_and_heads_excluded() {
        let conn = fixture();
        // A superseded pair INDEPENDENT of the candidate edges: old → head.
        conn.execute_batch(
            "INSERT INTO llm_wiki_entries
               (id, entity_id, title, body, source_type, created_at, updated_at,
                superseded_by, superseded_at, embedding_blob)
             VALUES ('f-old','e9','T old','B','user_stated',0,0,'f-head',50,NULL);
             INSERT INTO llm_wiki_entries
               (id, entity_id, title, body, source_type, created_at, updated_at, embedding_blob)
             VALUES ('f-head','e9','T head','B','user_stated',0,0,NULL)",
        )
        .unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'f-head'",
            rusqlite::params![ts_blob(&unit_vec(4))],
        )
        .unwrap();
        let exclude: Vec<String> = vec!["f-old".to_string()];
        // v1 path (the parity anchor):
        let m1 = crate::wisdom_match::wisdom_match_with_floor(
            &conn,
            &unit_vec(4),
            TS_KEY,
            Scheme::Raw,
            Some(0.5),
            8,
            &exclude,
            TS_NOW,
        )
        .unwrap();
        // Two-stage path with the same caller-side corrections: the ABSENT id
        // is `f-old` (what the caller passes), and — per the interface pin —
        // `exclude` already contains the correction heads the caller wants
        // skipped ("one id, one list").
        let mut excl_set: HashSet<String> = exclude.iter().cloned().collect();
        excl_set.insert("f-head".to_string());
        let (entries, outcome) = two_stage_entries(
            &conn,
            &unit_vec(1),
            &unit_vec(4),
            Scheme::Raw,
            Some(0.5),
            Some(0.5),
            8,
            &excl_set,
            &HashSet::from(["f-old".to_string()]),
            TS_NOW,
            TS_KEY,
            QueryKind::Message,
        )
        .unwrap();
        // Parity: v1's corrections shape == the two-stage caller's contract —
        // the head resolves, the old id appears in `supersedes`, and the HEAD
        // id never sneaks into the merged entries.
        assert_eq!(m1.corrections.len(), 1);
        assert_eq!(m1.corrections[0].id, "f-head");
        assert_eq!(m1.corrections[0].supersedes, vec!["f-old".to_string()]);
        assert!(
            !entries.iter().any(|e| e.id == "f-head"),
            "correction head must never appear in entries (one id, one list)"
        );
        assert!(
            !entries.iter().any(|e| e.id == "f-old"),
            "excluded id must never appear in entries"
        );
        // f2 is the only complement fact the merge can admit (f-old is
        // excluded, f-head is a correction head, f1/f3 are rule-(i) reachable).
        assert_eq!(outcome.v1merged, 1, "exactly f2 via the merge");
    }

    /// §6 audit-field semantics pinned (r26 m4, r27 m5, r28-m3, r30-m5):
    /// rule = the evaluated rule even on a two-stage CLOSE; open = FINAL
    /// entries non-empty; top = the rule's OWN decision variable; v1merged
    /// counts merge admits; floor/k/stale carry the audit identity.
    #[test]
    fn audit_outcome_fields_pinned() {
        let conn = fixture();
        // (a) Below-floor stage 1 ⇒ CLOSED, but the audit still reports
        // the EVALUATED rule (never rule=none on a two-stage close); top is
        // None because no fact cleared the floor. Query u2 ⊥ all blobs (u4)
        // ⇒ merge admits nothing either.
        let (entries, outcome) = ts_run(&conn, &unit_vec(1), &unit_vec(2), Some(0.5), Some(0.5), 8);
        assert_eq!(outcome.rule, Some(TwoStageRule::RestrictedCosine));
        assert!(!outcome.open, "below floor ⇒ closed");
        assert!(entries.is_empty());
        assert_eq!(outcome.top, None, "no fact cleared the floor ⇒ top None");
        assert_eq!(outcome.v1merged, 0, "u2 ⊥ u4 ⇒ merge admits nothing");
        assert_eq!(
            outcome.floor_key_used,
            format!("{TS_KEY}:two-stage:restricted-cosine")
        );
        assert_eq!(outcome.k, 8, "TWO_STAGE_K default");
        assert_eq!(outcome.stale_embed_key, 0);
        // (b) Mixed open: rule (i)'s floor 0.95 admits the u4 fact (1.0 ≥
        // 0.95) — open = the FINAL state (same list here), top = the RULE's
        // variable = the best RESTRICTED fact cosine 1.0. The chunk probes
        // (u1) score 0 against the u4 query — a different scale from
        // `top` = 1.0, proving the fact-cosine source.
        let (entries, outcome) =
            ts_run(&conn, &unit_vec(1), &unit_vec(4), Some(0.9), Some(0.95), 8);
        assert_eq!(outcome.rule, Some(TwoStageRule::RestrictedCosine));
        assert!(outcome.open);
        assert!(
            (outcome.top.unwrap() - 1.0).abs() < 1e-6,
            "top = best restricted fact cosine (chunks scored 0)"
        );
        assert_eq!(
            outcome.v1merged, 1,
            "f2 via merge; f1 is rule-(i) reachable"
        );
        // f3 is ALSO rule-(i)-reachable (proposal leg → doc2 has a scorable
        // chunk) and clears the 0.95 floor: 3 entries, ONE ranked list.
        assert_eq!(entries.len(), 3, "f1 + f3 (rule) + f2 (merge)");
        // (c) k mirrors gate_floor's lookup shape: any key → default, and
        // the k table is NOT a floor key.
        assert_eq!(two_stage_k(TS_KEY), 8);
        assert_eq!(two_stage_k("external:qwen/qwen3-embedding-4b:instr1"), 8);
        assert_eq!(two_stage_k("anything-else"), 8);
        assert!(gate_floor("default").is_none(), "k is NOT a floor key");
    }

    // ===================== Issue #282: activity query mode =====================

    /// rev-6 B1 ATTRIBUTION test: with a stub ACTIVITY composite value
    /// DIFFERENT from the message stub's 0.45, the active path must
    /// attribute ChunkHop via the activity composite row and report
    /// `floor_key_used` byte-exactly `…:activity:two-stage:chunk-hop`.
    /// Without the site-5 QueryKind threading, the attribution block would
    /// look up the MESSAGE composite (0.45 ≠ 0.40), attribute nothing, and
    /// fall back to RestrictedCosine on a chunk-hop-calibrated floor.
    #[test]
    fn activity_composite_attributes_chunk_hop_with_its_own_floor_value() {
        let conn = fixture();
        // u1 vs u4: chunk c1 (member of doc1, whose evidence names f1) is a
        // 1.0 hit under the raw vector; f1 is rule-(ii) reachable.
        let (entries, outcome) = two_stage_entries(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            None,       // v1_floor: NO v1 `:activity` row ever ships
            Some(0.40), // the activity composite floor (test table)
            8,
            &HashSet::new(),
            &HashSet::new(),
            TS_NOW,
            TS_KEY,
            QueryKind::Activity,
        )
        .unwrap();
        assert_eq!(outcome.rule, Some(TwoStageRule::ChunkHop));
        assert_eq!(
            outcome.floor_key_used,
            format!("{TS_KEY}:activity:two-stage:chunk-hop")
        );
        // The fixture's only rule-(ii)-reachable fact is f1
        // (`librarian_inferred`), so the `user_stated` aperture filters the
        // delivery to empty — which is exactly the §5 contract on this
        // brain (the aperture test below pins both sides of it).
        assert!(
            entries.is_empty(),
            "aperture: f1 (librarian_inferred) must not be delivered"
        );
    }

    /// rev-6 M1 LABEL test: the ran-path label is
    /// `{TWO_STAGE_LABEL_PREFIX}{scheme_key}` with the kind-carrying scheme
    /// key — `…:activity`, NO composite segment; the MESSAGE label is
    /// unchanged.
    #[test]
    fn activity_ran_label_is_scheme_key_plus_activity_no_composite_segment() {
        let conn = fixture();
        let run = wisdom_match_two_stage_with_v1(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            8,
            &[],
            TS_NOW,
            TS_KEY,
            QueryKind::Activity,
        )
        .unwrap();
        assert!(run.ran, "stub activity composite + full coverage ⇒ ran");
        assert_eq!(
            run.result.gate,
            format!("{TWO_STAGE_LABEL_PREFIX}{TS_KEY}:activity")
        );
        // The message label is unchanged (regression guard):
        let msg = wisdom_match_two_stage_with_v1(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            8,
            &[],
            TS_NOW,
            TS_KEY,
            QueryKind::Message,
        )
        .unwrap();
        assert_eq!(msg.result.gate, format!("{TWO_STAGE_LABEL_PREFIX}{TS_KEY}"));
    }

    /// rev-8 M1 / rev-9 M1 / rev-10 M1: under the activity kind the
    /// orchestrator's `!ran` v1 pass resolves NO floor (no v1 `:activity`
    /// row ships), so the attempt's discarded result carries the
    /// `GATE_UNCALIBRATED` + empty-entries signature — `gated_entries` is
    /// never entered inside the attempt (no v1 scan, no per-row stderr).
    /// The proxy is DISCRIMINATING: an un-threaded `QueryKind` would resolve
    /// the stub v1 row and label `semantic-v1:stub:constant8`.
    #[test]
    fn activity_attempt_degraded_run_carries_none_arm_signature() {
        let conn = fixture();
        // Coverage veto (stale embed keys) ⇒ the core cannot run ⇒ `!ran`:
        conn.execute(
            "UPDATE documents SET embed_key = 'old-model' WHERE id = 1",
            [],
        )
        .unwrap();
        let run = wisdom_match_two_stage_with_v1(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            8,
            &[],
            TS_NOW,
            TS_KEY,
            QueryKind::Activity,
        )
        .unwrap();
        assert!(!run.ran);
        assert_eq!(run.result.gate, GATE_UNCALIBRATED);
        assert!(run.result.entries.is_empty());
        assert!(run.outcome.rule.is_none());
    }

    /// rev-13 M1 APERTURE (lib, active path only): with the activity
    /// composite forcing a cell-4 run, the delivered entries contain NO
    /// non-`user_stated` fact even though rule (ii) opens doc1 (whose fact
    /// f1 is `librarian_inferred`); corrections are NOT aperture-filtered
    /// (rev-13 M1: corrections bypass the aperture); `open` reflects the
    /// post-aperture list. The message call on the same vectors still
    /// delivers f1 (message mode is untouched).
    #[test]
    fn activity_aperture_delivers_user_stated_only_on_the_active_path() {
        let conn = fixture();
        // f1 (librarian_inferred) is the only rule-(ii)-reachable fact; f2
        // (user_stated) is NOT rule-reachable, so the activity-active run
        // opens on f1's doc and the aperture filters f1 away entirely.
        let (entries, outcome) = two_stage_entries(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            None,
            Some(0.40),
            8,
            &HashSet::new(),
            &HashSet::new(),
            TS_NOW,
            TS_KEY,
            QueryKind::Activity,
        )
        .unwrap();
        assert_eq!(outcome.rule, Some(TwoStageRule::ChunkHop));
        assert!(
            !entries
                .iter()
                .any(|e| e.provenance.as_deref() != Some("user_stated")),
            "aperture: every delivered entry is user_stated"
        );
        // The message path on the same vectors is UNFILTERED (f1 delivered):
        // The MESSAGE control uses the message composite value (0.45 — the
        // attribution table is per-kind, rev-6 B1) and NO aperture: f1 is
        // delivered, proving message mode is untouched by the aperture.
        let (msg_entries, msg_outcome) = two_stage_entries(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            None,
            Some(0.45),
            8,
            &HashSet::new(),
            &HashSet::new(),
            TS_NOW,
            TS_KEY,
            QueryKind::Message,
        )
        .unwrap();
        assert_eq!(msg_outcome.rule, Some(TwoStageRule::ChunkHop));
        assert!(
            msg_entries.iter().any(|e| e.id == "f1"),
            "message mode unchanged: f1 (librarian_inferred) delivered"
        );
    }

    // ===================== Task 8: aperture widening + Path B =====================

    /// Task 8 helper: a rule-(ii)-reachable entry (its evidence names the
    /// fixture's `h-c1` chunk hash) with the stored `source_type` explicit,
    /// so the delivery aperture is the ONLY filter standing between it and
    /// the delivered list.
    fn seed_aperture_entry(conn: &Connection, id: &str, source_type: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries
               (id, entity_id, title, body, source_type, source_ref,
                created_at, updated_at, embedding_blob)
             VALUES (?1, ?2, 'T', 'B', ?3, 'x', 0, 0, ?4)",
            params![id, format!("e-{id}"), source_type, ts_blob(&unit_vec(4))],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at)
             VALUES (?1, 'p1', '{\"evidence\":[{\"content_hash\":\"h-c1\"}]}', 0)",
            params![id],
        )
        .unwrap();
    }

    /// Task 8 (§5 rev-15): the activity aperture admits the human-tier SET —
    /// `user_stated` AND `user_confirmed`; `immutable_document` and a
    /// stored type outside the provenance vocabulary (NULL provenance) do
    /// not. Delivered labels stay `provenance_for(stored)` verbatim.
    #[test]
    fn activity_admits_confirmed_and_stated() {
        let conn = fixture();
        seed_aperture_entry(&conn, "f-cf", "user_confirmed");
        seed_aperture_entry(&conn, "f-st", "user_stated");
        seed_aperture_entry(&conn, "f-im", "immutable_document");
        seed_aperture_entry(&conn, "f-np", "raw_note");
        let (entries, outcome) = two_stage_entries(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            None,
            Some(0.40), // the activity composite floor (test table)
            8,
            &HashSet::new(),
            &HashSet::new(),
            TS_NOW,
            TS_KEY,
            QueryKind::Activity,
        )
        .unwrap();
        assert_eq!(outcome.rule, Some(TwoStageRule::ChunkHop));
        // All four carry cosine 0.0 (u4 blobs vs the u1 query), so the
        // delivered order is the id tie-break over the admitted set only.
        let got: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            got,
            vec!["f-cf", "f-st"],
            "user_confirmed joins user_stated; immutable_document and NULL provenance stay out"
        );
        assert_eq!(entries[0].provenance.as_deref(), Some("user_confirmed"));
        assert_eq!(entries[1].provenance.as_deref(), Some("user_stated"));
    }

    /// Task 8 non-shadowing pin (§5 fairness rule): the aperture filter runs
    /// BEFORE the arm-internal truncate, so with max=1 a high-ranking
    /// non-admitted fact is dropped rather than consuming the slot and
    /// shadowing the admitted fact ranked below it.
    #[test]
    fn non_admitted_fact_never_shadows_admitted_below() {
        let conn = fixture();
        seed_aperture_entry(&conn, "f-im-high", "immutable_document");
        seed_aperture_entry(&conn, "f-st-low", "user_stated");
        // The immutable fact outranks: cosine 1.0 vs 0.0 against the query.
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'f-im-high'",
            params![ts_blob(&unit_vec(1))],
        )
        .unwrap();
        let (entries, _outcome) = two_stage_entries(
            &conn,
            &unit_vec(1),
            &unit_vec(1),
            Scheme::Raw,
            None,
            Some(0.40),
            1, // ONE slot: a post-truncate filter would deliver nothing
            &HashSet::new(),
            &HashSet::new(),
            TS_NOW,
            TS_KEY,
            QueryKind::Activity,
        )
        .unwrap();
        let got: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            got,
            vec!["f-st-low"],
            "the non-admitted high-ranker must be filtered pre-truncate, never shadow"
        );
    }

    /// rev-8 M2 / spec §4: `QueryKind::Activity` + explicit `--single-stage`
    /// selection never reaches a v1-activity arm in THIS PR — the CLI falls
    /// back to the message call (its own pin). Here we pin the CORE side:
    /// the activity v1 floor key resolves to None even though the message
    /// stub v1 floor exists.
    #[test]
    fn activity_v1_floor_key_resolves_none_despite_message_stub_floor() {
        assert!(gate_floor(&format!("{TS_KEY}:activity")).is_none());
        assert!(gate_floor(TS_KEY).is_some(), "message stub v1 floor exists");
    }

    /// rev-11 m2 RECORDED DEVIATION guard: the no-flag + message-floor-absent
    /// embed-failure divergence is unreachable — the message v1 row always
    /// exists in production and under the stub. Pinned here so a future table
    /// edit that removes it must revisit the spec §Error-handling deviation.
    #[test]
    fn message_v1_stub_floor_exists_deviation_guard() {
        assert!(gate_floor(TS_KEY).is_some());
    }
}
