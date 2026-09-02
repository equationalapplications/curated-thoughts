//! One-shot tier backfill for `llm_wiki_entries` (spec §3.3).
//!
//! Classifies existing entries only where provenance is certain: deposit-origin
//! entries take the configured deposit default, everything else stays NULL.
//!
//! The marker **parameterizes** the run; it does not gate it. A gate that
//! refuses on sight of a marker has an unrecoverable failure mode — a marker
//! present without its UPDATEs permanently suppresses a backfill that never
//! ran, and the NULL-only scope cannot repair it because those rows are still
//! NULL and now unreachable. Here a lost marker degrades to current-config
//! behaviour (and still cannot retier anything), and a spurious marker merely
//! supplies a default. No state is unrecoverable.

use anyhow::{Context as _, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub const MARKER_KEY: &str = "tier_backfill_v1";
pub const MARKER_VERSION: u32 = 1;

/// Deposit-origin detection: evidence pointing under the agent deposit prefix
/// (`safe_path.rs` AGENTS_DEPOSIT_DIR). Anything else is not certain provenance.
const DEPOSIT_EVIDENCE_PREFIX: &str = "immutable-source-files/agents/";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackfillMarker {
    pub version: u32,
    /// Write-once: when the cohort was established.
    pub first_applied_at: i64,
    /// Last run that actually wrote rows.
    pub last_applied_at: i64,
    pub runs: u32,
    /// **Write-once.** Load-bearing, not bookkeeping: if a rerun refreshed this
    /// from current config, the cohort value would follow config drift and the
    /// marker would decay into the current-config behaviour it exists to prevent.
    pub deposit_default_used: String,
    /// Cumulative across runs. A floor, not a total, if the marker was deleted.
    pub rows_classified: i64,
    pub schema_version: i64,
}

pub fn read_marker(conn: &Connection) -> Result<Option<BackfillMarker>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [MARKER_KEY],
            |r| r.get(0),
        )
        .ok();
    match raw {
        Some(s) => Ok(Some(
            serde_json::from_str(&s).context("parse tier_backfill_v1 marker")?,
        )),
        None => Ok(None),
    }
}

/// The `(entry_id, tier)` pairs an apply would write. Read-only.
pub fn plan_backfill(conn: &Connection, tier: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM llm_wiki_entries
          WHERE tier IS NULL
            AND deleted_at IS NULL
            AND evidence LIKE ?1 || '%'
          ORDER BY id",
    )?;
    let rows = stmt.query_map([DEPOSIT_EVIDENCE_PREFIX], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for id in rows {
        out.push((id?, tier.to_string()));
    }
    Ok(out)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Classify eligible entries and record the marker **in the same transaction**,
/// so the pair is all-or-nothing.
///
/// `config_default` is used only when no marker exists. Once a cohort exists,
/// its recorded `deposit_default_used` wins, so a config flip cannot split it.
pub fn apply_backfill(conn: &mut Connection, config_default: &str) -> Result<BackfillMarker> {
    let existing = read_marker(conn)?;
    let tier = existing
        .as_ref()
        .map(|m| m.deposit_default_used.clone())
        .unwrap_or_else(|| config_default.to_string());

    let schema_version: i64 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |r| r.get(0))
        .unwrap_or(0);

    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE llm_wiki_entries
            SET tier = ?1
          WHERE tier IS NULL
            AND deleted_at IS NULL
            AND evidence LIKE ?2 || '%'",
        rusqlite::params![tier, DEPOSIT_EVIDENCE_PREFIX],
    )? as i64;

    // A run that changes no data leaves no trace, matching the dry-run-default
    // posture. `last_applied_at` therefore means "last run that wrote rows".
    if changed == 0 {
        if let Some(marker) = existing {
            tx.commit()?;
            return Ok(marker);
        }
    }

    let now = now_unix();
    let marker = match existing {
        Some(prev) => BackfillMarker {
            version: prev.version,
            first_applied_at: prev.first_applied_at,
            last_applied_at: now,
            runs: prev.runs + 1,
            deposit_default_used: prev.deposit_default_used,
            rows_classified: prev.rows_classified + changed,
            schema_version,
        },
        None => BackfillMarker {
            version: MARKER_VERSION,
            first_applied_at: now,
            last_applied_at: now,
            runs: 1,
            deposit_default_used: tier,
            rows_classified: changed,
            schema_version,
        },
    };

    tx.execute(
        "INSERT INTO llm_wiki_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![MARKER_KEY, serde_json::to_string(&marker)?],
    )?;
    tx.commit()?;
    Ok(marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// A brain with the tables this backfill touches and two deposit-origin
    /// entries plus one non-deposit entry.
    fn seeded_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (
                 id TEXT PRIMARY KEY, entity_id TEXT NOT NULL, title TEXT,
                 evidence TEXT, deleted_at INTEGER,
                 tier TEXT NULL CHECK (tier IN ('fact','wisdom') OR tier IS NULL));
             CREATE TABLE llm_wiki_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version (version) VALUES (16);
             INSERT INTO llm_wiki_entries (id, entity_id, title, evidence) VALUES
                 ('d1','ent_1','D1','immutable-source-files/agents/note.md'),
                 ('d2','ent_1','D2','immutable-source-files/agents/other.md'),
                 ('x1','ent_1','X1','immutable-source-files/spec.md');",
        )
        .unwrap();
        let _ = &mut conn;
        conn
    }

    #[test]
    fn plan_lists_only_deposit_origin_entries() {
        let conn = seeded_conn();
        let plan = plan_backfill(&conn, "wisdom").unwrap();
        let ids: Vec<&str> = plan.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["d1", "d2"]);
        assert!(plan.iter().all(|(_, t)| t == "wisdom"));
    }

    #[test]
    fn plan_mutates_nothing() {
        let conn = seeded_conn();
        plan_backfill(&conn, "wisdom").unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries WHERE tier IS NOT NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(read_marker(&conn).unwrap().is_none());
    }

    #[test]
    fn apply_classifies_and_writes_the_marker() {
        let mut conn = seeded_conn();
        let marker = apply_backfill(&mut conn, "wisdom").unwrap();
        assert_eq!(marker.rows_classified, 2);
        assert_eq!(marker.runs, 1);
        assert_eq!(marker.deposit_default_used, "wisdom");

        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tier.as_deref(), Some("wisdom"));
        let untouched: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='x1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(untouched, None);
    }

    #[test]
    fn rerun_pins_the_cohort_tier_even_after_config_drift() {
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();

        // A deposit whose provenance only became visible after run 1.
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, evidence)
             VALUES ('d3','ent_1','D3','immutable-source-files/agents/late.md')",
            [],
        )
        .unwrap();

        // Config flipped since run 1 — the cohort must not split.
        let marker = apply_backfill(&mut conn, "fact").unwrap();
        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tier.as_deref(), Some("wisdom"), "rerun must use deposit_default_used, not current config");

        assert_eq!(marker.deposit_default_used, "wisdom", "write-once");
        assert_eq!(marker.rows_classified, 3, "accumulates");
        assert_eq!(marker.runs, 2, "increments");
    }

    #[test]
    fn rerun_never_retiers_an_already_classified_row() {
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();
        apply_backfill(&mut conn, "fact").unwrap();
        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tier.as_deref(), Some("wisdom"));
    }

    #[test]
    fn zero_row_rerun_leaves_the_marker_byte_identical() {
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();
        let before: String = conn
            .query_row("SELECT value FROM llm_wiki_meta WHERE key=?1", [MARKER_KEY], |r| r.get(0))
            .unwrap();
        apply_backfill(&mut conn, "wisdom").unwrap();
        let after: String = conn
            .query_row("SELECT value FROM llm_wiki_meta WHERE key=?1", [MARKER_KEY], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn deleting_the_marker_and_reapplying_is_a_no_op_on_classified_rows() {
        // The recovery property: no state is unrecoverable, because the marker
        // parameterizes the run rather than gating it (spec §3.3).
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();
        conn.execute("DELETE FROM llm_wiki_meta WHERE key=?1", [MARKER_KEY]).unwrap();
        apply_backfill(&mut conn, "fact").unwrap();
        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tier.as_deref(), Some("wisdom"), "NULL-only scope protects classified rows");
    }
}
