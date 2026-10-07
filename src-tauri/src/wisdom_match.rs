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

use crate::embed_scheme::{floor_key_for, read_scheme, Scheme};
use crate::embedder::{CloudProvider, EmbedProfile};
use crate::search::{bytes_to_f32, cosine_similarity};
use crate::wiki_graph::tier_weight;

/// Output schema version (`"schema"` in the JSON contract).
pub const SCHEMA_VERSION: u32 = 1;
/// `gate` value when the embed model has no calibrated floor.
pub const GATE_UNCALIBRATED: &str = "uncalibrated";
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

/// Abstention floors on RAW cosine, one per embedding model (CT INTENT rule
/// 7). Values come only from a `calibrate_wisdom_gate` run recorded under
/// docs/benchmarks/. A model not listed here abstains.
pub const WISDOM_GATE_FLOORS: &[(&str, f32)] = &[
    // Test-only key: reachable only with CURATED_EMBED_STUB=constant8.
    ("stub:constant8", 0.5),
    // Calibrated 2026-10-07 (OpenRouter, GLM-5.3-flash probes): hit@2 0.44, FP 0.05
    // — docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md
    ("external:qwen/qwen3-embedding-4b", 0.70),
    // instr1 scheme (both-side instruction prefix, spec-rev2 cell E): hit@2 0.57 at
    // this floor — docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md.
    // Key form: <raw gate key>:instr1 (see embed_scheme::floor_key_for).
    ("external:qwen/qwen3-embedding-4b:instr1", 0.64),
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

struct Temporal {
    has_superseded_by: bool,
    has_valid_to: bool,
    /// Active read scheme for the SELECT filter — `None` on the pre-V26 shape
    /// (no `embed_scheme` column). Resolved once per call, next to the column
    /// probes, so every scheme-derived tuple member of the call path shares
    /// one `read_scheme` resolution.
    scheme: Option<Scheme>,
}

fn temporal_columns(conn: &Connection) -> Result<Temporal> {
    let cols = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    let scheme = if cols.iter().any(|c| c == "embed_scheme") {
        // Fail-closed on an unknown `wisdom_active_scheme` value: never fall
        // back to `raw`. This is the ONE read_scheme resolution per call —
        // the SELECT filter here, and the floor + query prefix on the
        // production path, all derive from it.
        Some(read_scheme(conn)?)
    } else {
        // Pre-V26 shape: rows are de-facto raw, the filter is omitted,
        // mirroring how the temporal filters degrade on old tables.
        None
    };
    Ok(Temporal {
        has_superseded_by: cols.iter().any(|c| c == "superseded_by"),
        has_valid_to: cols.iter().any(|c| c == "valid_to"),
        scheme,
    })
}

/// Match with the calibrated floor for `gate_key` (none → abstain).
pub fn wisdom_match(
    conn: &Connection,
    query_vec: &[f32],
    gate_key: &str,
    max: usize,
    exclude: &[String],
    now_ms: i64,
) -> Result<WisdomMatch> {
    // The floor is the read scheme's tuple member for THIS model key: same
    // `read_scheme(conn)` resolution the SELECT filter uses inside
    // `wisdom_match_with_floor`, never an independently chosen scheme. An
    // unregistered (model, scheme) key means abstain (floor None).
    let floor = match temporal_columns(conn)?.scheme {
        Some(scheme) => gate_floor(&floor_key_for(gate_key, scheme)),
        None => gate_floor(gate_key),
    };
    wisdom_match_with_floor(conn, query_vec, gate_key, floor, max, exclude, now_ms)
}

/// `wisdom_match` with an explicit floor (tests and calibration). `floor =
/// None` abstains exactly like an uncalibrated model. An empty `query_vec`
/// yields no entries (corrections still flow).
pub fn wisdom_match_with_floor(
    conn: &Connection,
    query_vec: &[f32],
    gate_key: &str,
    floor: Option<f32>,
    max: usize,
    exclude: &[String],
    now_ms: i64,
) -> Result<WisdomMatch> {
    let max = max.min(MAX_ENTRIES);
    let temporal = temporal_columns(conn)?;
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
            gated_entries(conn, query_vec, floor, max, &skip, now_ms, &temporal)?
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

fn gated_entries(
    conn: &Connection,
    query_vec: &[f32],
    floor: f32,
    max: usize,
    skip: &HashSet<String>,
    now_ms: i64,
    temporal: &Temporal,
) -> Result<Vec<WisdomItem>> {
    let mut sql = String::from(
        "SELECT id, entity_id, title, body, source_type, embedding_blob
         FROM llm_wiki_entries
         WHERE deleted_at IS NULL AND embedding_blob IS NOT NULL",
    );
    // READ-scheme SELECT filter (spec §Scheme architecture): rows stamped
    // under another scheme are never candidates. `as_str()` is the stored
    // representation bound verbatim — no inline literal. `None` (pre-V26
    // shape, resolved in temporal_columns) omits the filter.
    if let Some(s) = temporal.scheme {
        sql.push_str(&format!(" AND embed_scheme = '{}'", s.as_str()));
    }
    if temporal.has_superseded_by {
        sql.push_str(" AND superseded_by IS NULL");
    }
    if temporal.has_valid_to {
        sql.push_str(" AND (valid_to IS NULL OR valid_to > ?1)");
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
    let rows: Vec<rusqlite::Result<CandidateRow>> = if temporal.has_valid_to {
        stmt.query_map(params![now_ms], mapper)?.collect()
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
        let raw = cosine_similarity(query_vec, &bytes_to_f32(&blob));
        if raw < floor {
            continue; // the gate compares RAW cosine — never the weighted score
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

    fn run(conn: &Connection, floor: Option<f32>, max: usize, exclude: &[&str]) -> WisdomMatch {
        let exclude: Vec<String> = exclude.iter().map(|s| s.to_string()).collect();
        wisdom_match_with_floor(conn, &Q, KEY, floor, max, &exclude, NOW).unwrap()
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
        let m = wisdom_match_with_floor(&conn, &[], KEY, Some(0.1), 5, &[], NOW).unwrap();
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

    /// Pre-V26 shape (no `embed_scheme` column): the filter degrades off and
    /// rows are read verbatim, mirroring the temporal-column degradation.
    #[test]
    fn pre_v26_table_reads_without_scheme_filter() {
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
}
