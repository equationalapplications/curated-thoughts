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
}

fn temporal_columns(conn: &Connection) -> Result<Temporal> {
    let cols = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    Ok(Temporal {
        has_superseded_by: cols.iter().any(|c| c == "superseded_by"),
        has_valid_to: cols.iter().any(|c| c == "valid_to"),
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
    wisdom_match_with_floor(
        conn,
        query_vec,
        gate_key,
        gate_floor(gate_key),
        max,
        exclude,
        now_ms,
    )
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
        assert_eq!(gate_floor("local:nomic-embed-code"), None);
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
}
