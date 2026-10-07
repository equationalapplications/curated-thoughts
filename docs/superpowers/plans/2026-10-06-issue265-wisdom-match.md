# `ct wisdom match` (issue #265) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `ct wisdom match`, a read-only, relevance-gated match of curated wisdom
facts against a message, with corrections for superseded facts. It is the contract
curated-thoughts-integrations (CTI) live delivery consumes.

**Architecture:** A new library module `src-tauri/src/wisdom_match.rs` holds the pure
logic. It gates on raw cosine against a per-model floor, filters to current facts, and
resolves corrections from `superseded_by` chains. A `Match` variant in the existing
`WisdomCmd` group of `tools/src/bin/ct.rs` calls it through
`queries::wisdom_match_cmd`. A new `calibrate_wisdom_gate` tool sets the floor for
`nomic-embed-code` from a fixture probe set, and a `slow-tests` regression test guards
the result.

**Tech Stack:** Rust (stable), rusqlite, serde/serde_json, clap 4 derive, flate2, sha2
(all existing workspace deps). Tests: `cargo test` (unit + `tools/tests` CLI
integration), `--features slow-tests` for the bench.

**Spec:** [`docs/superpowers/specs/2026-10-06-issue265-wisdom-match-design.md`](../specs/2026-10-06-issue265-wisdom-match-design.md)
(+ `2026-10-06-issue265-wisdom-match-investigation.md`). The plan argues from the spec;
executors read both. Consumer contract: CTI
`docs/superpowers/specs/2026-10-06-intuitive-wisdom-live-delivery-design.md`
§"CT prerequisite".

**Validation status:** the Task 1 code was written against `origin/main` @ `9c2281b`
during planning but **not compiled**; planning stopped before the build (owner
direction). Treat the first `cargo test` in Task 1 as the real check.

## Global Constraints

- Contract (verbatim): `ct wisdom match --json [--max N] [--exclude=<id>]... -- <text>`; stdout `{"schema":1,"gate":…,"entries":[item],"corrections":[item]}`, item = `{id,title,text,score,supersedes,provenance}`.
- Exit codes: 0 success (including zero matches and `uncalibrated`); 1 for every error, including usage errors (`ct` `main` maps clap errors to 1). `wisdom match` never returns 2.
- `<text>` only after `--` (clap `last = true`); truncated to 2000 chars. `--max` default 2, clamped to `0..=10`. `--exclude=<id>` must match `^[A-Za-z0-9._:-]{1,128}$`; more than 1024 → error.
- Gate compares **raw** cosine to `WISDOM_GATE_FLOORS[key]`; tier weight only orders. Unknown key → `gate:"uncalibrated"`, no entries.
- `provenance` ∈ {`librarian_inferred`, `user_stated`, `user_confirmed`, `immutable_document`} or `null`.
- One id, one list: a correction head never appears in `entries`.
- Read-only: `open_ro`, SELECT only, bound parameters only; exclude ids never in SQL text.
- Never touch the live brain (CT INTENT workflow 5). Calibration builds its own scratch brain; it needs real embeddings (Ollama `nomic-embed-code`), so it runs on the Linux reference machine — **not** the owner's Mac (no Ollama there).
- `ct recall`, MCP `wiki_search`/`wiki_context` unchanged.
- Commit messages: conventional (`feat:`/`test:`/`docs:`) for semantic-release; end with `Refs #265` and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't hand-edit `CHANGELOG.md` (release automation owns it).
- Before each commit: `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` clean.

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/wisdom_match.rs` (new) | gate key, floor table, matching, corrections, provenance, id validation, truncation + unit tests |
| `src-tauri/src/lib.rs` (modify) | `pub mod wisdom_match;` |
| `tools/src/queries.rs` (modify) | `wisdom_match_cmd` |
| `tools/src/bin/ct.rs` (modify) | `WisdomCmd::Match`, `parse_fact_id`, dispatch |
| `tools/tests/ct_wisdom_match.rs` (new) | CLI contract tests |
| `README.md` (modify) | one line in the `ct` command list |
| `src-tauri/tests/fixtures/wisdom_gate/{facts.jsonl,probes.jsonl,README.md}` (new) | calibration fixtures |
| `tools/src/bin/calibrate_wisdom_gate.rs` (new) + `tools/Cargo.toml` `[[bin]]` | calibration sweep + freeze |
| `src-tauri/tests/fixtures/wisdom_gate/{vectors.json.gz,expected.json}` (generated) | frozen vectors + chosen floor |
| `src-tauri/tests/wisdom_gate_bench.rs` (new) | `slow-tests` regression guard |
| `docs/benchmarks/YYYY-MM-DD-wisdom-gate-nomic-embed-code.md` (new) | calibration snapshot |

---

### Task 1: `wisdom_match` core module

**Files:**
- Create: `src-tauri/src/wisdom_match.rs`
- Modify: `src-tauri/src/lib.rs` (after `pub mod wiki_graph;`, line ~39)

**Interfaces:**
- Consumes: `crate::search::{bytes_to_f32, cosine_similarity}`, `crate::wiki_graph::tier_weight`, `crate::db::ddl_compat::existing_columns`, `crate::embedder::{CloudProvider, EmbedProfile, ExternalEmbedProfile}`, `crate::db::connection::open_in_memory` (tests).
- Produces (used by Tasks 2, 5, 6):
  - consts `SCHEMA_VERSION: u32`, `GATE_UNCALIBRATED: &str`, `MAX_CHAIN_DEPTH`, `MAX_ENTRIES`, `MAX_EXCLUDES`, `MAX_TEXT_CHARS: usize`, `PROVENANCE_VOCAB`, `WISDOM_GATE_FLOORS: &[(&str, f32)]`
  - `struct WisdomItem { id, title, text: String, score: Option<f32>, supersedes: Vec<String>, provenance: Option<String> }` (Serialize)
  - `struct WisdomMatch { schema: u32, gate: String, entries, corrections: Vec<WisdomItem> }` (Serialize)
  - `fn gate_floor(&str) -> Option<f32>`
  - `fn gate_model_key(&EmbedProfile, stub: Option<&str>) -> String`
  - `fn valid_fact_id(&str) -> bool`, `fn provenance_for(Option<&str>) -> Option<String>`, `fn truncate_text(&str) -> &str`
  - `fn wisdom_match(&Connection, &[f32], gate_key: &str, max: usize, exclude: &[String], now_ms: i64) -> Result<WisdomMatch>`
  - `fn wisdom_match_with_floor(&Connection, &[f32], gate_key: &str, floor: Option<f32>, max: usize, exclude: &[String], now_ms: i64) -> Result<WisdomMatch>`

- [ ] **Step 1: Register the module.** In `src-tauri/src/lib.rs`, add `pub mod wisdom_match;` directly below `pub mod wiki_graph;`.

- [ ] **Step 2: Write the module with its tests** (tests first in spirit: the `#[cfg(test)] mod tests` block encodes every spec "Unit" bullet). Full file:

```rust
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
    wisdom_match_with_floor(conn, query_vec, gate_key, gate_floor(gate_key), max, exclude, now_ms)
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
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
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
    let valid_to_col = if temporal.has_valid_to { "valid_to" } else { "NULL" };
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
            .query_row(&sql, params![next], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
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
        assert!((m.entries[0].score.unwrap() - 0.8).abs() < 1e-4, "score is raw cosine");
        assert_eq!(ids(&run(&conn, Some(0.5), 2, &[]).entries), vec!["fact", "plain"]);
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
        conn.execute("UPDATE llm_wiki_entries SET valid_to = ?1 WHERE id = 'expired'", params![NOW])
            .unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET valid_to = ?1 WHERE id = 'future'",
            params![NOW + 1],
        )
        .unwrap();
        conn.execute("UPDATE llm_wiki_entries SET deleted_at = 5 WHERE id = 'gone'", [])
            .unwrap();
        assert_eq!(ids(&run(&conn, Some(0.5), 10, &[]).entries), vec!["future", "live"]);
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
        for id in ["a1", "a2", "a3", "d1", "d2", "c1", "c2", "x1", "x2", "plain"] {
            seed(&conn, id, "ent", &at(0.1));
        }
        supersede(&conn, "a1", "a2"); // multi-hop: a1 -> a2 -> a3
        supersede(&conn, "a2", "a3");
        supersede(&conn, "d1", "d2"); // ends deleted
        conn.execute("UPDATE llm_wiki_entries SET deleted_at = 5 WHERE id = 'd2'", [])
            .unwrap();
        supersede(&conn, "c1", "c2"); // cycle
        supersede(&conn, "c2", "c1");
        supersede(&conn, "x1", "x2"); // head already excluded
        let m = run(&conn, None, 0, &["a1", "d1", "c1", "x1", "x2", "plain", "unknown"]);
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
        assert_eq!(gate_model_key(&cloud, None), "cloud:open_ai:text-embedding-3-small");
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
```

- [ ] **Step 3: Run the tests, confirm they fail without the implementation.** Temporarily replace the body of `wisdom_match_with_floor` with `todo!()`:

Run: `cd src-tauri && cargo test --lib wisdom_match`
Expected: the tests panic with `not yet implemented`. Restore the body.

- [ ] **Step 4: Run — PASS**

Run: `cd src-tauri && cargo test --lib wisdom_match`
Expected: 16 tests pass. If `Option::is_none_or` is unavailable on the toolchain, use `valid_to.map_or(true, |v| v > now_ms)`. If a `seed` INSERT fails on a NOT NULL column, copy the column list from `wiki_graph::unit_tests::seed_entry` (`src-tauri/src/wiki_graph.rs:1013`), which is known to work against `open_in_memory`.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add src-tauri/src/wisdom_match.rs src-tauri/src/lib.rs
git commit -m "feat(wisdom): wisdom_match core — raw-cosine gate per model, current-only, corrections

Refs #265

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `ct wisdom match` CLI

**Files:**
- Modify: `tools/src/queries.rs` (append `wisdom_match_cmd`)
- Modify: `tools/src/bin/ct.rs` (`WisdomCmd` enum ~line 245, `parse_fact_id` helper, dispatch ~line 543)
- Create: `tools/tests/ct_wisdom_match.rs`
- Modify: `README.md` (the `ct` command block, ~line 229-240)

**Interfaces:**
- Consumes: Task 1's `wisdom_match`, `gate_model_key`, `gate_floor`, `truncate_text`, `valid_fact_id`, `MAX_EXCLUDES`; `crate::write::{resolve, open_ro}`, `crate::paths::print_json`, `retrieval::load_embed_profile`, `embed_one` (already imported in `queries.rs:26-33`).
- Produces: `pub fn wisdom_match_cmd(text: &str, max: usize, exclude: &[String], json_mode: bool) -> Result<i32>` (reached as `cli_common::wisdom_match_cmd` via the `pub use crate::queries::*` re-export).

- [ ] **Step 1: Write the failing CLI tests** — `tools/tests/ct_wisdom_match.rs`:

```rust
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

#[test]
fn match_returns_contract_shape() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let v = json_of(&run_ct(&["wisdom", "match", "--json", "--", "how do I deploy"]));
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
            "wisdom", "match", "--json", "--max", "0", "--exclude=fact_old", "--", "q",
        ]));
        assert_eq!(v["entries"], serde_json::json!([]));
        assert_eq!(v["corrections"][0]["id"], "fact_new");
        assert_eq!(v["corrections"][0]["supersedes"], serde_json::json!(["fact_old"]));
        assert!(v["corrections"][0]["score"].is_null());
        // excluding the head too: nothing to correct, nothing to match
        let v = json_of(&run_ct(&[
            "wisdom", "match", "--json", "--exclude=fact_old", "--exclude=fact_new", "--", "q",
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
        json_of(&run_ct(&["wisdom", "match", "--json", "--", "-weird --text"]));
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
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test -p curated-thoughts-tools --test ct_wisdom_match`
(If the package name differs, take it from `tools/Cargo.toml` `[package] name`.)
Expected: the tests fail; clap rejects `match` as an unrecognized subcommand, so exit 1 where 0 is expected.

- [ ] **Step 3: Implement `wisdom_match_cmd`** — append to `tools/src/queries.rs`:

```rust
// ---------------------------------------------------------------------------
// `ct wisdom match` — issue #265 (consumer: CTI live wisdom delivery).
// ---------------------------------------------------------------------------

/// Relevance-gated, read-only wisdom match (spec 2026-10-06-issue265).
/// Exit 0 on success including zero matches and an uncalibrated gate;
/// every error propagates (exit 1). Never returns EXIT_NO_RESULTS.
pub fn wisdom_match_cmd(text: &str, max: usize, exclude: &[String], json_mode: bool) -> Result<i32> {
    use tauri_app_lib::wisdom_match as wm;

    let brain = resolve()?;
    let conn = open_ro(&brain)?;
    let profile = retrieval::load_embed_profile(&brain.paths.config_path)
        .context("loading embed profile from vault config.json")?;
    let stub = std::env::var("CURATED_EMBED_STUB").ok();
    let key = wm::gate_model_key(&profile, stub.as_deref());
    let text = wm::truncate_text(text);
    // Embed only when entries can actually be produced: an uncalibrated
    // gate, --max 0, or an empty text never needs the embedding backend.
    let query_vec = if text.trim().is_empty() || max == 0 || wm::gate_floor(&key).is_none() {
        Vec::new()
    } else {
        embed_one(&profile, text.to_string()).context("failed to embed query")?
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let result = wm::wisdom_match(&conn, &query_vec, &key, max, exclude, now_ms)?;
    if json_mode {
        print_json(&result);
    } else {
        println!("gate: {}", result.gate);
        for c in &result.corrections {
            println!("correction {} (supersedes {}): {}", c.id, c.supersedes.join(", "), c.title);
        }
        for e in &result.entries {
            println!("{:.4} {}: {}", e.score.unwrap_or(0.0), e.id, e.title);
        }
    }
    Ok(0)
}
```

- [ ] **Step 4: Add the subcommand** in `tools/src/bin/ct.rs`. Add the variant as the last one in `enum WisdomCmd`:

```rust
    /// Relevance-gated, read-only match of wisdom facts against a message
    /// (issue #265; consumer: CTI live delivery). The text goes after `--`.
    Match {
        #[arg(long)]
        json: bool,
        /// Most relevance-gated entries to return (clamped to 0..=10; corrections are extra).
        #[arg(long, default_value_t = 2)]
        max: usize,
        /// Fact id already in the caller's context (repeatable; use the --exclude=<id> form).
        #[arg(long = "exclude", value_parser = parse_fact_id, action = clap::ArgAction::Append)]
        exclude: Vec<String>,
        /// The message to match. Only accepted after `--`.
        #[arg(last = true, required = true)]
        text: Vec<String>,
    },
```

Add the helper next to `require_yes`:

```rust
/// clap value parser for `--exclude`: `^[A-Za-z0-9._:-]{1,128}$`.
fn parse_fact_id(s: &str) -> Result<String, String> {
    if tauri_app_lib::wisdom_match::valid_fact_id(s) {
        Ok(s.to_string())
    } else {
        Err(format!("invalid fact id {s:?} (expected ^[A-Za-z0-9._:-]{{1,128}}$)"))
    }
}
```

In `run`, inside `Cmd::Wisdom { cmd } => match cmd {`, after the `WisdomCmd::Pending` arm:

```rust
            WisdomCmd::Match {
                json,
                max,
                exclude,
                text,
            } => {
                if exclude.len() > tauri_app_lib::wisdom_match::MAX_EXCLUDES {
                    bail!(
                        "at most {} --exclude values",
                        tauri_app_lib::wisdom_match::MAX_EXCLUDES
                    );
                }
                cli_common::wisdom_match_cmd(&text.join(" "), max, &exclude, json)
            }
```

- [ ] **Step 5: Run — PASS**

Run: `cargo test -p curated-thoughts-tools --test ct_wisdom_match && cargo test -p curated-thoughts-tools --test ct_search_recall_code`
Expected: all pass. The second suite proves `ct recall`/`search`/`code` are unchanged.

- [ ] **Step 6: README** — in the `ct` command block, under the `ct recall` line, add:

```
ct wisdom match --json -- <message>    # relevance-gated wisdom facts for a message (CTI live delivery)
```

- [ ] **Step 7: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add tools/src/queries.rs tools/src/bin/ct.rs tools/tests/ct_wisdom_match.rs README.md
git commit -m "feat(ct): ct wisdom match — read-only relevance-gated match + corrections

Refs #265

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Calibration fixtures

The facts and probes must come from **different** models (CT INTENT workflow 4,
"cross-model paraphrase probes"). The executor (Claude) writes the facts, and
**GLM-5.3-FLASH** writes the probes through the owner's ZAI plan (`ZAI_API_KEY`,
Anthropic-compatible endpoint `https://api.z.ai/api/anthropic`).

**Files:**
- Create: `src-tauri/tests/fixtures/wisdom_gate/facts.jsonl`, `probes.jsonl`, `README.md`

- [ ] **Step 1: Write `facts.jsonl`** with 100 lines, one fact per line:
`{"id":"fact_wg_000","title":"…","body":"…","source_type":"librarian_inferred"}`.
Ids run `fact_wg_000`…`fact_wg_099`. Content: concrete software-development procedures
and gotchas, 1-3 sentences each, spread across 10 topics × 10 facts: git, CI, Rust
build, Python packaging, Docker, SQL/SQLite, HTTP APIs, testing, release/versioning,
shell. Each fact must be specific enough that only one fact answers a given situation.
Use `source_type` `user_stated` for 10 of them and `librarian_inferred` for the rest.

- [ ] **Step 2: Generate probes with GLM** (script stays in the scratchpad, not
committed):

```python
import json, os, urllib.request
FACTS = [json.loads(l) for l in open("src-tauri/tests/fixtures/wisdom_gate/facts.jsonl")]
URL = "https://api.z.ai/api/anthropic/v1/messages"
HDR = {"x-api-key": os.environ["ZAI_API_KEY"], "anthropic-version": "2023-06-01",
       "content-type": "application/json"}

def ask(prompt):
    body = json.dumps({"model": "GLM-5.3-FLASH", "max_tokens": 4000,
                       "messages": [{"role": "user", "content": prompt}]}).encode()
    req = urllib.request.Request(URL, body, HDR)
    out = json.load(urllib.request.urlopen(req, timeout=120))
    return "".join(b.get("text", "") for b in out["content"])

probes = []
for f in FACTS:
    text = ask(
        "A developer is chatting with a coding agent. Write ONE message (1-2 sentences) "
        "the developer might send in a situation where the following fact would help, "
        "WITHOUT quoting it and avoiding its distinctive keywords where natural. "
        "Reply with the message only.\n\nFACT: " + f["title"] + " — " + f["body"])
    probes.append({"text": text.strip(), "expect": [f["id"]]})
topics = sorted({f["title"] for f in FACTS})
for batch in range(4):
    text = ask(
        "Write 25 short developer messages to a coding agent, one per line, no numbering. "
        + ("They must share vocabulary with these topics but ask something NONE of them "
           "answers: " + "; ".join(topics[batch*25:(batch+1)*25]) if batch < 2 else
           "They must be about everyday development topics unrelated to git, CI, Rust, "
           "Python packaging, Docker, SQL, HTTP APIs, testing, releases or shell."))
    probes += [{"text": l.strip(), "expect": []} for l in text.splitlines() if l.strip()][:25]
with open("src-tauri/tests/fixtures/wisdom_gate/probes.jsonl", "w") as fh:
    for p in probes:
        fh.write(json.dumps(p, ensure_ascii=False) + "\n")
```

- [ ] **Step 3: Validate the fixtures:**

```python
import json
F = [json.loads(l) for l in open("src-tauri/tests/fixtures/wisdom_gate/facts.jsonl")]
P = [json.loads(l) for l in open("src-tauri/tests/fixtures/wisdom_gate/probes.jsonl")]
ids = {f["id"] for f in F}
assert len(F) >= 100 and len(ids) == len(F)
assert all(set(f) == {"id", "title", "body", "source_type"} for f in F)
rel = [p for p in P if p["expect"]]; irr = [p for p in P if not p["expect"]]
assert len(P) >= 200 and len(rel) >= 80 and len(irr) >= 80, (len(rel), len(irr))
assert all(set(p["expect"]) <= ids for p in P)
print("ok", len(F), len(rel), len(irr))
```

Expected: `ok 100 100 100`. Then hand-review 20 random probes. Drop any relevant
probe that quotes its fact verbatim, and any "irrelevant" probe that one of the facts
actually answers. Rerun the validator.

- [ ] **Step 4: `README.md`** in the fixture dir: purpose, generator model (facts:
Claude; probes: GLM-5.3-FLASH, with date), the prompt text from Step 2, counts, and
"never edit by hand after calibration — regenerate and recalibrate".

- [ ] **Step 5: Commit**

```bash
git add src-tauri/tests/fixtures/wisdom_gate/
git commit -m "test(wisdom): calibration fixtures — 100 facts, cross-model probes

Refs #265

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `calibrate_wisdom_gate` tool

**Files:**
- Create: `tools/src/bin/calibrate_wisdom_gate.rs`
- Modify: `tools/Cargo.toml` (add a `[[bin]]` entry in the same style as the others)

**Interfaces:**
- Consumes: Task 1's `wisdom_match_with_floor`, `gate_model_key`; `tauri_app_lib::embedder::{embed_batch, EmbedProfile}`, `tauri_app_lib::embed_sweep::embed_text_for_entry`, `tauri_app_lib::db::connection::open_app_db`.
- Produces: stdout table + summary JSON; with `--freeze DIR`, writes `DIR/vectors.json.gz` and `DIR/expected.json`. Task 5 reads both. `expected.json` = `{"model_key","floor","hit_at_2","fp_rate","n_relevant","n_irrelevant","facts_sha256","probes_sha256"}`; `vectors.json.gz` = `{"model_key","facts":[{id,title,body,source_type,vector}],"probes":[{text,expect,vector}]}`.

- [ ] **Step 1: `tools/Cargo.toml`:**

```toml
[[bin]]
name = "calibrate_wisdom_gate"
path = "src/bin/calibrate_wisdom_gate.rs"
```

- [ ] **Step 2: Write the tool:**

```rust
//! calibrate_wisdom_gate — sets the `ct wisdom match` abstention floor for
//! one embed model (issue #265; CT INTENT rule 7 + workflow 4).
//!
//! Builds a SCRATCH brain in a temp dir from fixture facts, embeds facts and
//! probes with the REAL profile, sweeps floors 0.20..=0.90 through
//! `wisdom_match_with_floor` itself, and picks the floor that maximises
//! hit@2 subject to FP rate <= 0.05 (ties -> higher floor). Never touches the
//! live brain. Refuses to run with CURATED_EMBED_STUB set.

use anyhow::{bail, Context, Result};
use clap::Parser;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

use tauri_app_lib::embed_sweep::embed_text_for_entry;
use tauri_app_lib::embedder::{embed_batch, EmbedProfile};
use tauri_app_lib::wisdom_match::{gate_model_key, wisdom_match_with_floor};

const FP_BOUND: f64 = 0.05;
const BATCH: usize = 32;
const NOW_MS: i64 = 4_102_444_800_000; // 2100-01-01: nothing in the fixture expires

#[derive(Parser)]
struct Args {
    #[arg(long)]
    facts: PathBuf,
    #[arg(long)]
    probes: PathBuf,
    /// EmbedProfile JSON (as in vault config.json `embed_profile`).
    #[arg(long, default_value = r#"{"type":"local","model":"nomic-embed-code"}"#)]
    profile: String,
    /// Write vectors.json.gz + expected.json here (the regression fixture).
    #[arg(long)]
    freeze: Option<PathBuf>,
}

#[derive(Deserialize, Serialize, Clone)]
struct Fact {
    id: String,
    title: String,
    body: String,
    source_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vector: Vec<f32>,
}

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

fn embed_all(profile: &EmbedProfile, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    let mut out = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(BATCH) {
        let got = embed_batch(profile, chunk.to_vec())?;
        if got.len() != chunk.len() {
            bail!("embed backend returned {} vectors for {}", got.len(), chunk.len());
        }
        out.extend(got);
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args = Args::parse();
    if std::env::var_os("CURATED_EMBED_STUB").is_some() {
        bail!("refusing to calibrate with CURATED_EMBED_STUB set — real embeddings only");
    }
    let profile: EmbedProfile = serde_json::from_str(&args.profile).context("--profile")?;
    let key = gate_model_key(&profile, None);
    let (mut facts, facts_sha) = read_jsonl::<Fact>(&args.facts)?;
    let (mut probes, probes_sha) = read_jsonl::<Probe>(&args.probes)?;
    let n_rel = probes.iter().filter(|p| !p.expect.is_empty()).count();
    let n_irr = probes.len() - n_rel;
    if facts.len() < 100 || probes.len() < 200 || n_rel < 80 || n_irr < 80 {
        bail!("fixture too small: facts {}, relevant {n_rel}, irrelevant {n_irr}", facts.len());
    }

    let fact_vecs = embed_all(
        &profile,
        facts.iter().map(|f| embed_text_for_entry(&f.title, &f.body)).collect(),
    )?;
    let probe_vecs = embed_all(&profile, probes.iter().map(|p| p.text.clone()).collect())?;
    for (f, v) in facts.iter_mut().zip(fact_vecs) {
        f.vector = v;
    }
    for (p, v) in probes.iter_mut().zip(probe_vecs) {
        p.vector = v;
    }

    let tmp = tempfile::tempdir()?;
    let conn = tauri_app_lib::db::connection::open_app_db(&tmp.path().join("brain.db"), None)?;
    for f in &facts {
        let blob: Vec<u8> = f.vector.iter().flat_map(|x| x.to_le_bytes()).collect();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent_calibration', ?2, ?3, '[]', 'inferred', ?4,
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?5, NULL)",
            params![f.id, f.title, f.body, f.source_type, blob],
        )?;
    }

    let mut best: Option<(u32, f64, f64)> = None; // (floor_pct, hit, fp)
    println!("floor  hit@2   fp_rate");
    for pct in 20u32..=90 {
        let floor = pct as f32 / 100.0;
        let (mut hits, mut fps) = (0usize, 0usize);
        for p in &probes {
            let m = wisdom_match_with_floor(&conn, &p.vector, &key, Some(floor), 2, &[], NOW_MS)?;
            if p.expect.is_empty() {
                fps += usize::from(!m.entries.is_empty());
            } else {
                hits += usize::from(m.entries.iter().any(|e| p.expect.contains(&e.id)));
            }
        }
        let hit = hits as f64 / n_rel as f64;
        let fp = fps as f64 / n_irr as f64;
        println!("{floor:.2}   {hit:.3}   {fp:.3}");
        if fp <= FP_BOUND && best.is_none_or(|(_, h, _)| hit >= h) {
            best = Some((pct, hit, fp)); // >= keeps the HIGHER floor on ties
        }
    }
    let Some((pct, hit, fp)) = best else {
        bail!("no floor meets FP <= {FP_BOUND}; model stays uncalibrated");
    };
    let expected = serde_json::json!({
        "model_key": key, "floor": pct as f64 / 100.0, "hit_at_2": hit, "fp_rate": fp,
        "n_relevant": n_rel, "n_irrelevant": n_irr,
        "facts_sha256": facts_sha, "probes_sha256": probes_sha,
    });
    println!("{}", serde_json::to_string_pretty(&expected)?);

    if let Some(dir) = args.freeze {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("expected.json"), serde_json::to_vec_pretty(&expected)?)?;
        let frozen = serde_json::json!({"model_key": key, "facts": facts, "probes": probes});
        let file = std::fs::File::create(dir.join("vectors.json.gz"))?;
        let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        gz.write_all(&serde_json::to_vec(&frozen)?)?;
        gz.finish()?;
    }
    Ok(())
}
```

- [ ] **Step 3: Build and check the guards** (runs anywhere, no Ollama needed):

Run: `cargo build -p curated-thoughts-tools --bin calibrate_wisdom_gate && CURATED_EMBED_STUB=constant8 ./target/debug/calibrate_wisdom_gate --facts src-tauri/tests/fixtures/wisdom_gate/facts.jsonl --probes src-tauri/tests/fixtures/wisdom_gate/probes.jsonl; echo exit=$?`
Expected: `error: refusing to calibrate with CURATED_EMBED_STUB set …`, `exit=1`. If `hex` is not a direct dependency of `tools`, add `hex = "0.4"` (already used by `src-tauri`). If `open_app_db` is not reachable, use `rusqlite::Connection::open` + `tauri_app_lib::db::connection::migrate_open_db(&conn, None)`.

- [ ] **Step 4: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add tools/src/bin/calibrate_wisdom_gate.rs tools/Cargo.toml Cargo.lock
git commit -m "feat(tools): calibrate_wisdom_gate — floor sweep on a scratch brain with real embeddings

Refs #265

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Calibrate `nomic-embed-code`, freeze, regression guard

Runs on the **Linux reference machine** with Ollama serving `nomic-embed-code`
(production default). Not on the owner's Mac.

**Files:**
- Generated: `src-tauri/tests/fixtures/wisdom_gate/vectors.json.gz`, `expected.json`
- Modify: `src-tauri/src/wisdom_match.rs` (`WISDOM_GATE_FLOORS`)
- Create: `src-tauri/tests/wisdom_gate_bench.rs`, `docs/benchmarks/<date>-wisdom-gate-nomic-embed-code.md`

- [ ] **Step 1: Run calibration**

```bash
ollama pull nomic-embed-code
./target/release/calibrate_wisdom_gate \
  --facts src-tauri/tests/fixtures/wisdom_gate/facts.jsonl \
  --probes src-tauri/tests/fixtures/wisdom_gate/probes.jsonl \
  --freeze src-tauri/tests/fixtures/wisdom_gate | tee /tmp/wisdom-gate-run.txt
```

(build first with `cargo build --release -p curated-thoughts-tools --bin calibrate_wisdom_gate`).
Expected: a 71-row table and a summary JSON. If it exits "no floor meets FP <= 0.05",
STOP and report. The model stays uncalibrated and the owner decides (better probes vs
a different model); do not loosen `FP_BOUND`.

- [ ] **Step 2: Set the floor.** In `WISDOM_GATE_FLOORS`, add after the stub entry
(use the exact `floor` from `expected.json`):

```rust
    // Calibrated <date> on <machine>: hit@2 <h>, FP <fp> — docs/benchmarks/<date>-wisdom-gate-nomic-embed-code.md
    ("local:nomic-embed-code", <floor>),
```

and update the `gate_keys` unit test: replace
`assert_eq!(gate_floor("local:nomic-embed-code"), None);` with
`assert_eq!(gate_floor("local:nomic-embed-code"), Some(<floor>));` and
`assert_eq!(gate_floor("local:other-model"), None);`.

- [ ] **Step 3: Regression test** — `src-tauri/tests/wisdom_gate_bench.rs`:

```rust
//! Frozen-vector regression guard for the `ct wisdom match` floor (issue #265).
//! Recomputes hit@2 / FP through `wisdom_match_with_floor` from the vectors
//! the calibration run froze. Guards the CODE PATH, not the model.
#![cfg(feature = "slow-tests")]

use rusqlite::params;
use std::io::Read;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wisdom_gate");

#[test]
fn wisdom_gate_floor_still_holds() {
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{DIR}/expected.json")).unwrap()).unwrap();
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(format!("{DIR}/vectors.json.gz")).unwrap())
        .read_to_end(&mut raw)
        .unwrap();
    let frozen: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let key = expected["model_key"].as_str().unwrap();
    let floor = expected["floor"].as_f64().unwrap() as f32;
    assert_eq!(
        tauri_app_lib::wisdom_match::gate_floor(key),
        Some(floor),
        "WISDOM_GATE_FLOORS must match the calibration snapshot"
    );

    let conn = tauri_app_lib::db::connection::open_in_memory().unwrap();
    let vec_of = |v: &serde_json::Value| -> Vec<f32> {
        v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect()
    };
    for f in frozen["facts"].as_array().unwrap() {
        let blob: Vec<u8> = vec_of(&f["vector"]).iter().flat_map(|x| x.to_le_bytes()).collect();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent_calibration', ?2, ?3, '[]', 'inferred', ?4,
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?5, NULL)",
            params![
                f["id"].as_str().unwrap(),
                f["title"].as_str().unwrap(),
                f["body"].as_str().unwrap(),
                f["source_type"].as_str().unwrap(),
                blob
            ],
        )
        .unwrap();
    }
    let (mut hits, mut rel, mut fps, mut irr) = (0usize, 0usize, 0usize, 0usize);
    for p in frozen["probes"].as_array().unwrap() {
        let expect: Vec<&str> = p["expect"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        let m = tauri_app_lib::wisdom_match::wisdom_match_with_floor(
            &conn, &vec_of(&p["vector"]), key, Some(floor), 2, &[], 4_102_444_800_000,
        )
        .unwrap();
        if expect.is_empty() {
            irr += 1;
            fps += usize::from(!m.entries.is_empty());
        } else {
            rel += 1;
            hits += usize::from(m.entries.iter().any(|e| expect.contains(&e.id.as_str())));
        }
    }
    let hit = hits as f64 / rel as f64;
    let fp = fps as f64 / irr as f64;
    println!("wisdom gate {key} @ {floor}: hit@2 {hit:.3}, FP {fp:.3}");
    assert!(fp <= 0.05, "FP {fp} > 0.05");
    assert!(
        hit >= expected["hit_at_2"].as_f64().unwrap() - 0.02,
        "hit@2 {hit} regressed below snapshot"
    );
}
```

Run: `cd src-tauri && cargo test --features "test-utils,slow-tests" --test wisdom_gate_bench -- --nocapture`
Expected: PASS, printing the same hit@2 and FP as the calibration run.

- [ ] **Step 4: Latency.** Against the scratch fixture brain is not representative.
Instead, run 50 calls of `ct wisdom match --json -- "<probe text>"` against a scratch
copy of a real brain (`CURATED_BRAIN_DIR` = a copied directory, never the live one).
Record p50 and p95. Target: p95 ≤ 1.5 s warm.

- [ ] **Step 5: Benchmark snapshot** `docs/benchmarks/<date>-wisdom-gate-nomic-embed-code.md`,
following the README's convention: what ran (command lines), model and Ollama version,
machine, fixture sha256s (from `expected.json`), the full sweep table from
`/tmp/wisdom-gate-run.txt`, the chosen floor, hit@2, FP, and the latency p50/p95.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add src-tauri/src/wisdom_match.rs src-tauri/tests/wisdom_gate_bench.rs \
        src-tauri/tests/fixtures/wisdom_gate/ docs/benchmarks/
git commit -m "feat(wisdom): calibrate nomic-embed-code floor + frozen-vector regression guard

Refs #265

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Follow-up issue, review, PR

- [ ] **Step 1: Follow-up issue (ask the owner first; it's outward-facing).** Draft:
"Active Librarian: apply supersession deposits through core-llm-wiki `supersede` so
`llm_wiki_entries.superseded_by` is populated. Today nothing writes it from the deposit
path. `ct wisdom match` corrections (#265) stay empty until it does. CT INTENT rule 4."
On approval: `gh issue create -R equationalapplications/curated-thoughts …`. Link it
from the spec's "Dependency" paragraph.
- [ ] **Step 2:** superpowers:verification-before-completion — `cargo test --workspace`, the slow bench, fmt, clippy. Paste outputs.
- [ ] **Step 3:** Dual review (GLM + Opus) to convergence per CT INTENT workflow 3; open questions park the PR.
- [ ] **Step 4:** Push, open the PR `feat: ct wisdom match (#265)` linking the spec, investigation, plan, benchmark snapshot and the CTI spec; flip the spec `**Status:**` to implemented with the PR number in a follow-up commit pushed in the same flow. After release, tell CTI the minimum CT version (CTI README + its plan Task 9 unblocks).
