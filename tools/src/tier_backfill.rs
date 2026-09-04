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

/// Matches a path column against the deposit prefix in **both** shapes the
/// database actually holds.
///
/// `documents.path` is written by the ingest walker, which canonicalizes to an
/// absolute path (`lib.rs` reconcile: `std::fs::canonicalize(entry.path())`),
/// while the legacy `source_ref` producer and the fixtures store a
/// vault-relative path. An anchored `LIKE 'immutable-source-files/agents/%'`
/// therefore matched nothing on a real brain while every relative-path unit
/// test passed — so the anchored form is deliberately paired with a
/// `'%/' || prefix` form here. Windows separators are normalized to `/` so a
/// `C:\Vault\immutable-source-files\agents\note.md` still matches, mirroring
/// `safe_path::is_deposit_path`.
///
/// `{col}` is interpolated, never user input; the prefix stays a bound `?1`.
fn deposit_path_predicate(col: &str) -> String {
    format!(
        "({col} LIKE ?1 || '%' OR {col} LIKE '%/' || ?1 || '%' OR \
         REPLACE({col}, '\\', '/') LIKE ?1 || '%' OR \
         REPLACE({col}, '\\', '/') LIKE '%/' || ?1 || '%')"
    )
}

/// The shared `WHERE` body naming rows eligible for classification: a live,
/// unclassified entry whose provenance is certainly a deposit. `plan_backfill`
/// and `apply_backfill` must never drift, so they read the same predicate.
fn eligible_rows_predicate() -> String {
    format!(
        "tier IS NULL
           AND deleted_at IS NULL
           AND (
             -- Legacy form: source_ref is a plain vault-relative path
             (source_ref NOT LIKE '{{%' AND {legacy})
             OR
             -- Current form: source_ref is the JSON blob; the deposit path is
             -- recovered via chunks.content_hash -> documents.path. Gate on
             -- leading-byte '{{' so legacy rows never hit json_extract (which
             -- raises on non-JSON input).
             (substr(source_ref, 1, 1) = '{{' AND json_valid(source_ref) AND EXISTS (
               SELECT 1 FROM json_each(json_extract(source_ref, '$.evidence')) AS ev
               JOIN chunks c ON c.content_hash = json_extract(ev.value, '$.content_hash')
               JOIN documents d ON d.id = c.doc_id
               WHERE {doc}
             ))
           )",
        legacy = deposit_path_predicate("source_ref"),
        doc = deposit_path_predicate("d.path"),
    )
}

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

/// Marker read over a `Transaction`. Used by `apply_backfill` so the read
/// happens inside the same IMMEDIATE transaction that may write the marker —
/// a read on the outer connection would race a concurrent apply.
fn read_marker_tx(tx: &rusqlite::Transaction<'_>) -> Result<Option<BackfillMarker>> {
    let raw: Option<String> = tx
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
///
/// `source_ref` is polymorphic (see `src-tauri/src/db/commit.rs:136-180`):
///   * **Legacy**: a bare vault-relative path (pre-c30f141 producer).
///   * **Current**: a JSON blob `{"proposal_id":..., "evidence":[{"content_hash":...}]}`
///     where the path lives on `documents.path` joined through `chunks.content_hash`.
///
/// The predicate matches either form so a fresh / recently-rebuilt brain (which
/// contains only the JSON form) classifies correctly. See also
/// `source_docs_from_ref` at `src-tauri/src/db/entities.rs:195-238` for the same
/// join shape used at read time.
pub fn plan_backfill(conn: &Connection, tier: &str) -> Result<Vec<(String, String)>> {
    let sql = format!(
        "SELECT id FROM llm_wiki_entries WHERE {} ORDER BY id",
        eligible_rows_predicate()
    );
    let mut stmt = conn.prepare(&sql)?;
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
///
/// The transaction is opened with `IMMEDIATE` semantics **before** the marker
/// is read. A deferred transaction plus an out-of-transaction read lets two
/// concurrent applies both observe "no marker", then race to write one; the
/// second writer would clobber the first's cohort record with its own
/// `config_default`, so later eligible rows join the cohort at the wrong tier.
/// `IMMEDIATE` acquires the reserved lock at BEGIN, so the second apply blocks
/// until the first commits, then sees the new marker and reuses its
/// `deposit_default_used` (the write-once field) instead of refreshing it
/// from the loser's view of config.
pub fn apply_backfill(conn: &mut Connection, config_default: &str) -> Result<BackfillMarker> {
    use rusqlite::TransactionBehavior;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing = read_marker_tx(&tx)?;
    let tier = existing
        .as_ref()
        .map(|m| m.deposit_default_used.clone())
        .unwrap_or_else(|| config_default.to_string());

    let schema_version: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // The JSON branch below excludes rows whose source_ref will not parse.
    // That exclusion is correct, but silent — issue #162's corruption was
    // invisible partly because this path skipped 140 rows without saying so.
    let excluded: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM llm_wiki_entries
              WHERE source_ref IS NOT NULL
                AND substr(source_ref, 1, 1) = '{'
                AND NOT json_valid(source_ref)
                AND deleted_at IS NULL
                AND tier IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if excluded > 0 {
        eprintln!(
            "[tier_backfill] skipping {excluded} row(s) this run could classify: \
             malformed JSON source_ref (deposit-origin tier cannot be classified \
             for these; see issue #162)"
        );
    }

    // The predicate binds the prefix as ?1, so the tier value takes ?2 and the
    // parameter order is (prefix, tier) — the reverse of the reading order.
    let update_sql = format!(
        "UPDATE llm_wiki_entries SET tier = ?2 WHERE {}",
        eligible_rows_predicate()
    );
    let changed = tx.execute(
        &update_sql,
        rusqlite::params![DEPOSIT_EVIDENCE_PREFIX, tier],
    )? as i64;

    // A run that changes no data leaves no trace, matching the dry-run-default
    // posture. `last_applied_at` therefore means "last run that wrote rows".
    // The recovery direction (marker was deleted, possibly with no new rows
    // to classify) still writes a fresh ledger so an operator never loses the
    // floor they relied on — see spec §3.3.
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
            // Recovery direction: no prior cohort, so the marker's
            // `deposit_default_used` is the **current** config default
            // (the prior cohort is gone — there is no write-once value to
            // preserve). Spec §3.3 zero-row recovery.
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

    /// A brain mirroring the live V16 shape: `source_ref` is polymorphic (legacy
    /// plain-path vs. current JSON blob), and the deposit-origin path lives on
    /// `documents.path` joined through `chunks.content_hash`.
    ///
    /// Coverage:
    ///   * `d1`, `d2` — legacy plain-path deposit-origin entries (match the
    ///     `(source_ref NOT LIKE '{%' AND source_ref LIKE 'immutable-source-files/agents/%')` branch)
    ///   * `j1`, `j2` — current JSON-blob deposit-origin entries (match the
    ///     `EXISTS (... json_each -> chunks -> documents.path LIKE ...)` branch)
    ///   * `x1`, `jx1` — non-deposit-origin entries (spec origin); must NOT classify
    ///   * `dsoft` — path *almost* matches the prefix (`agents-but-not-really/...`);
    ///     the `LIKE 'prefix/%'` check must reject it
    ///   * `j3` — NULL source_ref; must NOT classify (purely ungrounded)
    fn seeded_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"CREATE TABLE llm_wiki_entries (
                 id TEXT PRIMARY KEY, entity_id TEXT NOT NULL, title TEXT,
                 source_ref TEXT, deleted_at INTEGER,
                 tier TEXT NULL CHECK (tier IN ('fact','wisdom') OR tier IS NULL));
             CREATE TABLE llm_wiki_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             CREATE TABLE documents (
                 id TEXT PRIMARY KEY, path TEXT NOT NULL);
             CREATE TABLE chunks (
                 content_hash TEXT PRIMARY KEY, doc_id TEXT NOT NULL);
             INSERT INTO schema_version (version) VALUES (16);
             INSERT INTO documents (id, path) VALUES
                 ('doc_d1','immutable-source-files/agents/note.md'),
                 ('doc_d2','immutable-source-files/agents/other.md'),
                 ('doc_x1','immutable-source-files/spec.md'),
                 ('doc_d3','immutable-source-files/agents/late.md'),
                 ('doc_dsoft','immutable-source-files/agents-but-not-really/note.md');
             INSERT INTO chunks (content_hash, doc_id) VALUES
                 ('h_d1','doc_d1'),
                 ('h_d2','doc_d2'),
                 ('h_x1','doc_x1'),
                 ('h_d3','doc_d3'),
                 ('h_dsoft','doc_dsoft');
             -- Legacy plain-path source_ref (pre-c30f141 form)
             INSERT INTO llm_wiki_entries (id, entity_id, title, source_ref) VALUES
                 ('d1','ent_1','D1','immutable-source-files/agents/note.md'),
                 ('d2','ent_1','D2','immutable-source-files/agents/other.md'),
                 ('x1','ent_1','X1','immutable-source-files/spec.md'),
                 ('dsoft','ent_1','Dsoft','immutable-source-files/agents-but-not-really/note.md');
             -- Current JSON-blob source_ref (c30f141+ producer contract)
             INSERT INTO llm_wiki_entries (id, entity_id, title, source_ref) VALUES
                 ('j1','ent_1','J1','{"proposal_id":"p1","evidence":[{"content_hash":"h_d1"}]}'),
                 ('j2','ent_1','J2','{"proposal_id":"p2","evidence":[{"content_hash":"h_d2"}]}'),
                 ('jx1','ent_1','JX1','{"proposal_id":"p3","evidence":[{"content_hash":"h_x1"}]}'),
                 ('j3','ent_1','J3',NULL);"#,
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
        // Both forms: legacy plain-path (d1, d2) and current JSON blob (j1, j2).
        // Excluded: x1, jx1 (spec origin), dsoft (prefix-but-not-segment),
        // j3 (NULL source_ref, purely ungrounded).
        assert_eq!(ids, vec!["d1", "d2", "j1", "j2"]);
        assert!(plan.iter().all(|(_, t)| t == "wisdom"));
    }

    #[test]
    fn plan_mutates_nothing() {
        let conn = seeded_conn();
        plan_backfill(&conn, "wisdom").unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE tier IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
        assert!(read_marker(&conn).unwrap().is_none());
    }

    #[test]
    fn apply_classifies_and_writes_the_marker() {
        let mut conn = seeded_conn();
        let marker = apply_backfill(&mut conn, "wisdom").unwrap();
        assert_eq!(marker.rows_classified, 4, "d1, d2, j1, j2");
        assert_eq!(marker.runs, 1);
        assert_eq!(marker.deposit_default_used, "wisdom");

        // Sample one of each form to confirm both branches wrote.
        let legacy_tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(legacy_tier.as_deref(), Some("wisdom"));
        let json_tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='j1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(json_tier.as_deref(), Some("wisdom"));

        // Non-deposit-origin entries stay NULL.
        let untouched_spec: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='x1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            untouched_spec, None,
            "spec-origin legacy row must stay NULL"
        );
        let untouched_json_spec: Option<String> = conn
            .query_row(
                "SELECT tier FROM llm_wiki_entries WHERE id='jx1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            untouched_json_spec, None,
            "spec-origin JSON row must stay NULL"
        );
        let untouched_soft: Option<String> = conn
            .query_row(
                "SELECT tier FROM llm_wiki_entries WHERE id='dsoft'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            untouched_soft, None,
            "prefix-but-not-segment must stay NULL"
        );
        let untouched_null: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='j3'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(untouched_null, None, "NULL source_ref must stay NULL");
    }

    #[test]
    fn rerun_pins_the_cohort_tier_even_after_config_drift() {
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();

        // A late deposit-origin entry whose provenance only became visible after
        // run 1 — exercises the JSON-blob branch on rerun (rather than the
        // already-covered legacy plain-path branch).
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, source_ref)
             VALUES ('j3_late','ent_1','J3Late',
                     '{\"proposal_id\":\"p_late\",\"evidence\":[{\"content_hash\":\"h_d3\"}]}')",
            [],
        )
        .unwrap();

        // Config flipped since run 1 — the cohort must not split.
        let marker = apply_backfill(&mut conn, "fact").unwrap();
        let tier: Option<String> = conn
            .query_row(
                "SELECT tier FROM llm_wiki_entries WHERE id='j3_late'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            tier.as_deref(),
            Some("wisdom"),
            "rerun must use deposit_default_used, not current config"
        );

        assert_eq!(marker.deposit_default_used, "wisdom", "write-once");
        assert_eq!(marker.rows_classified, 5, "accumulates (4 + 1 late JSON)");
        assert_eq!(marker.runs, 2, "increments");
    }

    #[test]
    fn rerun_never_retiers_an_already_classified_row() {
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();
        apply_backfill(&mut conn, "fact").unwrap();
        // Sample one legacy and one JSON-blob row to lock both branches.
        let legacy_tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(legacy_tier.as_deref(), Some("wisdom"));
        let json_tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='j1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(json_tier.as_deref(), Some("wisdom"));
    }

    #[test]
    fn zero_row_rerun_leaves_the_marker_byte_identical() {
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();
        let before: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key=?1",
                [MARKER_KEY],
                |r| r.get(0),
            )
            .unwrap();
        apply_backfill(&mut conn, "wisdom").unwrap();
        let after: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key=?1",
                [MARKER_KEY],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn deleting_the_marker_and_reapplying_is_a_no_op_on_classified_rows() {
        // The recovery property: no state is unrecoverable, because the marker
        // parameterizes the run rather than gating it (spec §3.3).
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();
        conn.execute("DELETE FROM llm_wiki_meta WHERE key=?1", [MARKER_KEY])
            .unwrap();
        apply_backfill(&mut conn, "fact").unwrap();
        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            tier.as_deref(),
            Some("wisdom"),
            "NULL-only scope protects classified rows"
        );
    }

    /// The production shape. `documents.path` is written by the ingest walker,
    /// which canonicalizes to an **absolute** path, so the anchored
    /// `LIKE 'immutable-source-files/agents/%'` predicate this tool shipped
    /// with matched nothing on a real brain — while every relative-path
    /// fixture above passed. Both shapes must classify.
    #[test]
    fn classifies_deposit_rows_whose_document_path_is_absolute() {
        let mut conn = seeded_conn();
        conn.execute_batch(
            r#"INSERT INTO documents (id, path) VALUES
                   ('doc_abs','/Users/x/Vault/immutable-source-files/agents/abs.md'),
                   ('doc_abs_sib','/Users/x/Vault/immutable-source-files/agents-but-not-really/abs.md');
               INSERT INTO chunks (content_hash, doc_id) VALUES
                   ('h_abs','doc_abs'),
                   ('h_abs_sib','doc_abs_sib');
               INSERT INTO llm_wiki_entries (id, entity_id, title, source_ref) VALUES
                   ('j_abs','ent_1','JAbs','{"proposal_id":"p_abs","evidence":[{"content_hash":"h_abs"}]}'),
                   ('j_abs_sib','ent_1','JAbsSib','{"proposal_id":"p_sib","evidence":[{"content_hash":"h_abs_sib"}]}'),
                   ('d_abs','ent_1','DAbs','/Users/x/Vault/immutable-source-files/agents/legacy-abs.md');"#,
        )
        .unwrap();

        let plan = plan_backfill(&conn, "wisdom").unwrap();
        let ids: Vec<&str> = plan.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            ids.contains(&"j_abs"),
            "an absolute documents.path must classify via the JSON branch, got {ids:?}"
        );
        assert!(
            ids.contains(&"d_abs"),
            "an absolute legacy source_ref must classify too, got {ids:?}"
        );
        assert!(
            !ids.contains(&"j_abs_sib"),
            "the trailing separator must still reject an absolute sibling directory"
        );

        // Relative fixtures keep classifying — the fix is additive.
        assert!(ids.contains(&"d1") && ids.contains(&"j1"));

        apply_backfill(&mut conn, "wisdom").unwrap();
        for id in ["j_abs", "d_abs"] {
            let tier: Option<String> = conn
                .query_row("SELECT tier FROM llm_wiki_entries WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(tier.as_deref(), Some("wisdom"), "{id} must be classified");
        }
        let sib: Option<String> = conn
            .query_row(
                "SELECT tier FROM llm_wiki_entries WHERE id='j_abs_sib'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sib, None, "sibling directory must stay NULL");
    }

    #[test]
    fn windows_separator_paths_classify_via_normalized_branch() {
        // `documents.path` on a Windows host looks like
        // `C:\Vault\immutable-source-files\agents\note.md` — backslashes
        // throughout, no forward slash before the deposit prefix. The SQL
        // predicate normalizes backslashes to forward slashes so this
        // shape classifies too, mirroring `safe_path::is_deposit_path`.
        let mut conn = seeded_conn();
        conn.execute_batch(
            r#"INSERT INTO documents (id, path) VALUES
                   ('doc_win','C:\Vault\immutable-source-files\agents\note.md'),
                   ('doc_win_sib','C:\Vault\immutable-source-files\agents-but-not-really\note.md');
               INSERT INTO chunks (content_hash, doc_id) VALUES
                   ('h_win','doc_win'),
                   ('h_win_sib','doc_win_sib');
               INSERT INTO llm_wiki_entries (id, entity_id, title, source_ref) VALUES
                   ('j_win','ent_1','JWin','{"proposal_id":"p_win","evidence":[{"content_hash":"h_win"}]}'),
                   ('j_win_sib','ent_1','JWinSib','{"proposal_id":"p_win_sib","evidence":[{"content_hash":"h_win_sib"}]}');"#,
        )
        .unwrap();

        let plan = plan_backfill(&conn, "wisdom").unwrap();
        let ids: Vec<&str> = plan.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            ids.contains(&"j_win"),
            "Windows-shaped absolute path must classify via the JSON branch, got {ids:?}"
        );
        assert!(
            !ids.contains(&"j_win_sib"),
            "Windows-shaped sibling directory must still be rejected, got {ids:?}"
        );

        apply_backfill(&mut conn, "wisdom").unwrap();
        let tier: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='j_win'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tier.as_deref(), Some("wisdom"));
    }

    #[test]
    fn deleted_marker_is_recreated_by_a_zero_row_rerun() {
        // Spec §3.3 zero-row recovery: when the marker is absent, a rerun
        // writes a fresh ledger regardless of `changed`, so an operator
        // never loses the floor they relied on. The fresh marker's
        // `deposit_default_used` is the current config default (the prior
        // cohort is gone), `first_applied_at` is the run's wall clock, and
        // `rows_classified` is the count of rows that run newly classified.
        let mut conn = seeded_conn();
        apply_backfill(&mut conn, "wisdom").unwrap();
        // All four deposit-origin rows are already tiered.
        conn.execute("DELETE FROM llm_wiki_meta WHERE key = ?1", [MARKER_KEY])
            .unwrap();
        let marker = apply_backfill(&mut conn, "fact").unwrap();
        assert_eq!(
            marker.rows_classified, 0,
            "zero-row recovery: nothing newly classified"
        );
        assert_eq!(
            marker.deposit_default_used, "fact",
            "recovery direction uses current config, not the deleted cohort's"
        );
        assert_eq!(marker.runs, 1, "fresh marker, not an increment");
        // Already-classified rows stay classified (NULL-only scope is unchanged).
        let d1: Option<String> = conn
            .query_row("SELECT tier FROM llm_wiki_entries WHERE id='d1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(d1.as_deref(), Some("wisdom"));
    }

    #[test]
    fn malformed_json_source_ref_is_excluded() {
        // A source_ref that starts with '{' but is not valid JSON must not cause
        // json_extract to raise "malformed JSON" — it must be silently excluded.
        let mut conn = seeded_conn();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, source_ref)
             VALUES ('bad_json','ent_1','BadJson','{not valid json')",
            [],
        )
        .unwrap();

        // plan_backfill must not raise.
        let plan = plan_backfill(&conn, "wisdom").unwrap();
        let ids: Vec<&str> = plan.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            !ids.contains(&"bad_json"),
            "malformed-JSON source_ref must not appear in plan"
        );

        // apply_backfill must not raise.
        let marker = apply_backfill(&mut conn, "wisdom").unwrap();
        assert_eq!(
            marker.rows_classified, 4,
            "malformed-JSON row is not counted"
        );
        let tier: Option<String> = conn
            .query_row(
                "SELECT tier FROM llm_wiki_entries WHERE id='bad_json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tier, None, "malformed-JSON source_ref stays NULL");
    }
}
