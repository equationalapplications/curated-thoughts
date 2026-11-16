//! `ct wisdom areas rename` — the owner-run area rename (spec §12, rev 12).
//!
//! Owner rulings 2026-11-16 (Discord msg `1571895753129988189`, binding):
//! the hub is named DIRECTLY by its configured `wisdom.areas_paths` key —
//! NO `--path` resolution on a write; the audit is ONE `curated_agent_log`
//! row INSIDE the rename transaction (NO vault file).
//!
//! One transaction (§12.2): UPDATE `llm_wiki_entries SET area = :new,
//! updated_at = :now WHERE area = :old AND entity_id IN (:cluster_ids)` —
//! the merge-group expansion (gate-5 MAJOR-1: a single-entity_id binding
//! strands pre-merge loser-keyed facts) — then a re-SELECT of the same set
//! builds a full-row `wiki_fact_outbox_payload` per affected entry and
//! `push_entries_outbox` emits one outbox Update per row. The audit row
//! (`ct-cli` / `wisdom_areas_rename` / `write`, entity_id = the hub id,
//! summary = JSON `{by, old, new, rows}`) is written in the SAME
//! transaction: fail-closed — a rollback leaves NO audit row (§12.5 rev 12).
//!
//! Refusals, never degrade (§12.1): unknown hub key (error lists ALL
//! configured keys), unresolvable/dead hub, same-survivor collision,
//! `new` not defined by the hub's areas file, `old` matching zero rows
//! (documented choice: refuse — a zero-row "success" would write an audit
//! row describing nothing and could mask a wrong hub selection).

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;

use crate::config::hub_resolver::{configured_survivors, detect_collision, live_survivor, load_areas_file};
use crate::config::WisdomConfig;
use crate::db::commit::{
    log_agent_access_checked_with_summary, push_entries_outbox, wiki_fact_outbox_payload,
};
use crate::db::outbox_format::OutboxOperation;
use crate::db::entities::{cluster_ids, in_placeholders};
use crate::db::proposals_review::os_account;

/// What one rename invocation did (or would do, on a dry run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreasRenameReport {
    /// The hub id exactly as configured (and passed via `--hub`).
    pub hub: String,
    pub old: String,
    pub new: String,
    /// Rows matching `old` across the hub's merge group.
    pub rows: usize,
    /// `false` on a dry run (nothing written); `true` when committed.
    pub executed: bool,
}

const AUDIT_CLIENT: &str = "ct-cli";
const AUDIT_TOOL: &str = "wisdom_areas_rename";

/// Rename `old` → `new` across `hub_id`'s merge group. With `execute =
/// false` this is a DRY RUN (§12.1 default): the preview is returned and
/// NOTHING is written. Refuses (never degrades) on every misconfiguration.
pub fn areas_rename(
    conn: &Connection,
    cfg: &WisdomConfig,
    hub_id: &str,
    old: &str,
    new: &str,
    execute: bool,
) -> Result<AreasRenameReport> {
    // Selector is the CONFIGURED KEY, named directly (owner ruling): no
    // path resolution, no fuzzy lookup — the exact `wisdom.areas_paths` id.
    let Some((_, areas_path)) = cfg.areas_paths.iter().find(|(id, _)| id == hub_id) else {
        let keys = if cfg.areas_paths.is_empty() {
            "(none configured)".to_string()
        } else {
            cfg.areas_paths
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        bail!(
            "rename refused: hub `{hub_id}` is not a configured wisdom.areas_paths key; \
             configured keys: {keys}"
        );
    };

    // §3.3: a same-survivor collision fails the whole block closed (m2: the
    // ONE configured_survivors walk — no inline duplicate).
    let survivors = configured_survivors(conn, cfg);
    if let Some(reason) = detect_collision(&survivors) {
        bail!("rename refused: configured hubs collide: {reason}");
    }

    // Dead/unresolvable hub refuses (chain walk + liveness).
    let hub_survivor = live_survivor(conn, hub_id)
        .map_err(|reason| anyhow!("rename refused: hub {hub_id} unresolvable: {reason}"))?;

    // The merge-group expansion (§12.2, gate-5 MAJOR-1).
    let repo_group = cluster_ids(conn, &hub_survivor)?;

    // `new` must be defined by THIS hub's areas file (§12.1).
    let file = load_areas_file(areas_path)
        .with_context(|| format!("rename refused: areas file for hub {hub_id} unusable"))?;
    if !file.areas.iter().any(|(name, _)| name == new) {
        let defined = if file.areas.is_empty() {
            "(none)".to_string()
        } else {
            file.areas
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        bail!(
            "rename refused: area `{new}` is not defined by hub {hub_id}'s areas file \
             (defined areas: {defined})"
        );
    }

    // The affected set, captured BEFORE the update so the payload re-SELECT
    // iterates exactly the same rows.
    //
    // Gauntlet fix 4: on execute=true the WRITE transaction (BEGIN IMMEDIATE
    // via `unchecked_transaction`) is opened BEFORE this capture and the
    // capture SELECT runs INSIDE it — capture + UPDATE(by captured ids) +
    // count check + outbox + audit all share one transaction, so no
    // concurrent writer can slip rows in between capture and rename. A
    // dry run keeps the read-only autocommit path (no lock, nothing written).
    let placeholders = in_placeholders(&repo_group);
    let capture = |c: &Connection| -> Result<Vec<String>> {
        let sql = format!(
            "SELECT id FROM llm_wiki_entries
              WHERE area = ?1 AND entity_id IN ({placeholders})"
        );
        let mut stmt = c.prepare(&sql)?;
        let params: Vec<&str> = std::iter::once(old)
            .chain(repo_group.iter().map(String::as_str))
            .collect();
        let rows: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    };
    let tx = if execute { Some(conn.unchecked_transaction()?) } else { None };
    let ids = match tx.as_ref() {
        Some(t) => capture(t)?,
        None => capture(conn)?,
    };

    // Documented choice: zero matching rows is a REFUSAL, not a success.
    // (On execute=true the just-opened transaction drops here and rolls
    // back — nothing was written inside it.)
    if ids.is_empty() {
        bail!(
            "rename refused: area `{old}` matches zero rows across hub {hub_id}'s \
             merge group ({} id(s)); nothing to rename",
            repo_group.len()
        );
    }

    let report = AreasRenameReport {
        hub: hub_id.to_string(),
        old: old.to_string(),
        new: new.to_string(),
        rows: ids.len(),
        executed: execute,
    };
    let Some(tx) = tx else {
        return Ok(report);
    };

    // Gauntlet fix 5: skew-safe ordering token (§12.6). `updated_at` is the
    // change token the REMOTE orders on — a skewed-back local clock must
    // never LOWER it, so the bump is max(local clock, one millisecond past
    // the affected rows' max stored updated_at).
    let now_ms = {
        let max_stored: i64 = tx.query_row(
            &format!(
                "SELECT COALESCE(MAX(updated_at), 0) FROM llm_wiki_entries
                  WHERE id IN ({})",
                in_placeholders(&ids)
            ),
            rusqlite::params_from_iter(ids.iter()),
            |r| r.get(0),
        )?;
        std::cmp::max(now_ms(), max_stored.saturating_add(1))
    };

    // REVIEW M1: the UPDATE is bound to the CAPTURED ids (single source of
    // truth — UPDATE/outbox divergence is impossible by construction) and the
    // changed-row count MUST equal `ids.len()`; a mismatch BAILS (refusal),
    // never a silent partial rename.
    // §12.6: `updated_at` bumps — it is the change token the remote orders on.
    let id_placeholders = in_placeholders(&ids);
    let changed: usize = {
        let sql = format!(
            "UPDATE llm_wiki_entries
                SET area = ?1, updated_at = ?2
              WHERE id IN ({id_placeholders})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let mut params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(new.to_string()), Box::new(now_ms)];
        params.extend(ids.iter().cloned().map(|id| Box::new(id) as _));
        stmt.execute(rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())))?
    };
    if changed != ids.len() {
        bail!(
            "rename refused: UPDATE matched {changed} of {} captured row(s) — \
             refusing a partial rename; transaction rolled back",
            ids.len()
        );
    }

    // Full-row payloads (Task 7 shape) for exactly the renamed set.
    {
        let placeholders2 = in_placeholders(&ids);
        let sql = format!(
            "SELECT id, entity_id, title, body, tags, confidence, source_type, source_hash,
                    source_ref, okf_type, okf_sources, okf_verified, okf_usage_window,
                    created_at, updated_at, deleted_at, lifecycle_status, stale_after,
                    generated_by, last_verified_at, last_verified_by, area
               FROM llm_wiki_entries
              WHERE area = ?1 AND id IN ({placeholders2})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let params: Vec<&str> = std::iter::once(new)
            .chain(ids.iter().map(String::as_str))
            .collect();
        let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
        // Re-SELECT iterating the SAME renamed set (§12.2): the payload is
        // the FULL row (`reassert_entries`, restore_sync.rs, is the shape
        // precedent), carrying area = new.
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let entity_id: String = r.get(1)?;
            let tags_raw: String = r.get(4)?;
            let payload = wiki_fact_outbox_payload(
                &id,
                &entity_id,
                &r.get::<_, String>(2)?,
                &r.get::<_, String>(3)?,
                &serde_json::from_str::<Vec<String>>(&tags_raw).unwrap_or_default(),
                &r.get::<_, String>(5)?,
                &r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?.as_deref(),
                &r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                r.get::<_, Option<String>>(9)?.as_deref(),
                r.get::<_, Option<String>>(10)?.as_deref(),
                r.get::<_, Option<String>>(11)?.as_deref(),
                r.get::<_, Option<String>>(12)?.as_deref(),
                r.get::<_, i64>(13)?,
                r.get::<_, i64>(14)?,
                r.get::<_, Option<i64>>(15)?,
                r.get::<_, Option<String>>(16)?.as_deref(),
                r.get::<_, Option<i64>>(17)?,
                r.get::<_, Option<String>>(18)?.as_deref(),
                r.get::<_, Option<i64>>(19)?,
                r.get::<_, Option<String>>(20)?.as_deref(),
                Some(new),
            );
            push_entries_outbox(
                &tx,
                &entity_id,
                &id,
                OutboxOperation::Update,
                payload,
                now_ms,
            )?;
        }
    }

    // ONE audit row, SAME transaction (§12.5 rev 12). Fail-closed: a failed
    // insert propagates and the transaction rolls back with it.
    let summary = serde_json::json!({
        "by": os_account(),
        "old": old,
        "new": new,
        "rows": ids.len(),
    })
    .to_string();
    log_agent_access_checked_with_summary(
        &tx,
        AUDIT_CLIENT,
        AUDIT_TOOL,
        Some(hub_id),
        "write",
        Some(&summary),
    )?;

    tx.commit()?;
    Ok(report)
}

/// Milliseconds since the epoch — the `updated_at` bump value (§12.6).
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn seed_entity(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?1, 'concept', 'summary', 100, 100)",
            [id],
        )
        .unwrap();
    }

    fn seed_fact(conn: &Connection, id: &str, entity_id: &str, area: Option<&str>, updated_at: i64) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding, area
             ) VALUES (?1, ?2, ?1, ?1, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, ?3, NULL, 0, NULL, NULL, NULL, ?4)",
            rusqlite::params![id, entity_id, updated_at, area],
        )
        .unwrap();
    }

    fn merge(conn: &Connection, loser: &str, survivor: &str) {
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES (?1, ?2, 1)",
            rusqlite::params![loser, survivor],
        )
        .unwrap();
    }

    /// WisdomConfig pointing `hub` at a real areas file under `dir`.
    fn cfg_with_hub(dir: &std::path::Path, areas_yaml: &str) -> (WisdomConfig, std::path::PathBuf) {
        let checkout = dir.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        let path = checkout.join(".ct-areas.yaml");
        std::fs::write(&path, areas_yaml).unwrap();
        let cfg = WisdomConfig {
            areas_paths: vec![("ent-hub".to_string(), path.clone())],
            org_entity_id: None,
        };
        (cfg, path)
    }

    const AREAS_YAML: &str = "areas:\n  old:\n    - \"**\"\n  new:\n    - \"src/**\"\n";

    fn audit_rows(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare("SELECT tool, summary FROM curated_agent_log")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn outbox_updates(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT record_id, payload FROM llm_wiki_outbox
                  WHERE table_name = 'entries' AND operation = 'UPDATE'",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    /// Contract item 1 (gate-5 MAJOR-1): the rename spans the WHOLE merge
    /// group — survivor-keyed AND loser-keyed rows both rename, and BOTH
    /// outbox Updates emit. Stale names on other groups are untouched (item 6).
    #[test]
    fn rename_expands_across_the_merge_group() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        for id in ["ent-hub", "ent-loser", "ent-other"] {
            seed_entity(&conn, id);
        }
        merge(&conn, "ent-loser", "ent-hub");
        seed_fact(&conn, "fact-surv", "ent-hub", Some("old"), 100);
        seed_fact(&conn, "fact-lose", "ent-loser", Some("old"), 100);
        seed_fact(&conn, "fact-other", "ent-other", Some("old"), 100);
        seed_fact(&conn, "fact-newname", "ent-hub", Some("new"), 100);

        let report = areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).unwrap();
        assert_eq!(report.rows, 2);
        assert!(report.executed);

        let area_of = |id: &str| -> Option<String> {
            conn.query_row(
                "SELECT area FROM llm_wiki_entries WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(area_of("fact-surv"), Some("new".into()));
        assert_eq!(area_of("fact-lose"), Some("new".into()));
        assert_eq!(
            area_of("fact-other"),
            Some("old".into()),
            "stale name on another group untouched"
        );
        let updated_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM llm_wiki_entries WHERE id = 'fact-lose'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            updated_at > 100,
            "updated_at bumped on every renamed row (§12.6)"
        );

        let updates = outbox_updates(&conn);
        assert_eq!(updates.len(), 2, "one outbox Update per renamed row");
        let renamed_ids: Vec<&String> = updates.iter().map(|(id, _)| id).collect();
        assert!(renamed_ids.contains(&&"fact-surv".to_string()));
        assert!(renamed_ids.contains(&&"fact-lose".to_string()));
        for (_, payload) in &updates {
            let v: serde_json::Value = serde_json::from_str(payload).unwrap();
            assert_eq!(v["area"], serde_json::json!("new"));
            assert!(v["id"].is_string() && v["body"].is_string(), "full-row payload");
        }
    }

    /// Contract item 5: ONE audit row, actor + names in the summary.
    #[test]
    fn audit_row_in_the_same_transaction() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        seed_entity(&conn, "ent-hub");
        seed_fact(&conn, "fact-a", "ent-hub", Some("old"), 100);

        areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).unwrap();
        let rows = audit_rows(&conn);
        assert_eq!(rows.len(), 1, "exactly one audit row");
        assert_eq!(rows[0].0, "wisdom_areas_rename");
        let v: serde_json::Value = serde_json::from_str(&rows[0].1).unwrap();
        assert_eq!(v["old"], serde_json::json!("old"));
        assert_eq!(v["new"], serde_json::json!("new"));
        assert_eq!(v["rows"], serde_json::json!(1));
        // m3: hermetic CI clears USER/USERNAME — `by` may legitimately be null.
        assert!(
            v["by"].is_string() || v["by"].is_null(),
            "summary carries the os_account() actor"
        );
    }

    /// Contract item 5: a ROLLED-BACK transaction writes NO audit row.
    #[test]
    fn rolled_back_transaction_writes_no_audit_row() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        seed_entity(&conn, "ent-hub");
        seed_fact(&conn, "fact-a", "ent-hub", Some("old"), 100);
        // Sabotage the outbox so the in-transaction push fails after the
        // UPDATE ran — the whole transaction (audit row included) must roll back.
        conn.execute_batch("DROP TABLE llm_wiki_outbox;").unwrap();

        assert!(areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).is_err());
        assert_eq!(
            audit_rows(&conn).len(),
            0,
            "rollback leaves NO audit row (§12.5 rev 12)"
        );
        let area: Option<String> = conn
            .query_row(
                "SELECT area FROM llm_wiki_entries WHERE id = 'fact-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(area, Some("old".into()), "the rename itself rolled back");
    }

    /// Contract item 4 (dry-run DEFAULT): nothing written, nothing staged.
    #[test]
    fn dry_run_writes_nothing() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        seed_entity(&conn, "ent-hub");
        seed_fact(&conn, "fact-a", "ent-hub", Some("old"), 100);

        let report = areas_rename(&conn, &cfg, "ent-hub", "old", "new", false).unwrap();
        assert_eq!(report.rows, 1);
        assert!(!report.executed);
        assert_eq!(outbox_updates(&conn).len(), 0);
        assert_eq!(audit_rows(&conn).len(), 0);
        let area: Option<String> = conn
            .query_row(
                "SELECT area FROM llm_wiki_entries WHERE id = 'fact-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(area, Some("old".into()));
    }

    /// Contract item 3: unknown hub → error LISTING ALL configured keys.
    #[test]
    fn unknown_hub_lists_all_configured_keys() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (mut cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        cfg.areas_paths.push(("ent-second".to_string(), std::path::PathBuf::from("/nonexistent/.ct-areas.yaml")));
        let err = format!(
            "{}",
            areas_rename(&conn, &cfg, "ent-nope", "old", "new", true).unwrap_err()
        );
        assert!(err.contains("ent-hub") && err.contains("ent-second"), "{err}");
    }

    /// Contract item 3: `new` not defined by the hub's areas file refuses.
    #[test]
    fn undefined_new_area_refuses() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        seed_entity(&conn, "ent-hub");
        seed_fact(&conn, "fact-a", "ent-hub", Some("old"), 100);
        let err = format!(
            "{}",
            areas_rename(&conn, &cfg, "ent-hub", "old", "bogus", true).unwrap_err()
        );
        assert!(err.contains("refused") && err.contains("old"), "{err}");
        assert_eq!(outbox_updates(&conn).len(), 0);
        assert_eq!(audit_rows(&conn).len(), 0);
    }

    /// Contract item 3: dead/unresolvable hub refuses.
    #[test]
    fn dead_hub_refuses() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        // NO curated_entities row → the configured key is not live.
        let err = format!(
            "{}",
            areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).unwrap_err()
        );
        assert!(err.contains("unresolvable"), "{err}");
    }

    /// Contract item 3: a same-survivor collision refuses the whole block.
    #[test]
    fn collided_hubs_refuse() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (mut cfg, path2) = cfg_with_hub(dir.path(), AREAS_YAML);
        let second_checkout = path2.parent().unwrap().join("checkout2");
        std::fs::create_dir_all(&second_checkout).unwrap();
        let path_a = second_checkout.join(".ct-areas.yaml");
        std::fs::write(&path_a, AREAS_YAML).unwrap();
        cfg.areas_paths.insert(0, ("ent-hub2".to_string(), path_a));
        seed_entity(&conn, "ent-hub");
        // Both keys chain-resolve to the same live survivor `ent-hub`.
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent-hub2', 'ent-hub', 1)",
            [],
        )
        .unwrap();
        let err = format!(
            "{}",
            areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).unwrap_err()
        );
        assert!(err.contains("collide"), "{err}");
    }

    /// Contract item 3 (documented choice): zero matching rows REFUSES.
    #[test]
    fn zero_matching_rows_refuses() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        seed_entity(&conn, "ent-hub");
        let err = format!(
            "{}",
            areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).unwrap_err()
        );
        assert!(err.contains("zero rows"), "{err}");
    }

    /// Gauntlet fix 5: a skewed-back local clock must never LOWER the
    /// remote-ordering token — the bump is max(local, stored_max + 1ms).
    /// The seed's `updated_at` (4_102_444_800_000 ≈ year 2100) is far in the
    /// future of any test-run clock, so a plain `now_ms()` would go backwards.
    #[test]
    fn updated_at_never_regresses_past_stored_max() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        seed_entity(&conn, "ent-hub");
        let far_future = 4_102_444_800_000i64;
        seed_fact(&conn, "fact-a", "ent-hub", Some("old"), far_future);
        seed_fact(&conn, "fact-b", "ent-hub", Some("old"), far_future - 5);

        areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).unwrap();
        let (ua_a, ua_b): (i64, i64) = conn
            .query_row(
                "SELECT MAX(updated_at), MIN(updated_at) FROM llm_wiki_entries
                  WHERE id IN ('fact-a','fact-b')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            ua_a > far_future && ua_b > far_future,
            "every renamed row's updated_at must exceed the stored max \
             (skew-safe ordering token): a={ua_a} b={ua_b} stored_max={far_future}"
        );
        assert_eq!(ua_a, ua_b, "both renamed rows share one bump value");
        // The outbox Update carries the same monotone token.
        let outbox_ms: Vec<i64> = conn
            .prepare("SELECT created_at FROM llm_wiki_outbox WHERE operation = 'UPDATE'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            outbox_ms.iter().all(|&ms| ms > far_future),
            "outbox rows carry the skew-safe token too: {outbox_ms:?}"
        );
    }

    /// REVIEW M1 pin: the UPDATE is bound to the CAPTURED ids and the
    /// changed-row count MUST equal `ids.len()` — a mismatch bails (refusal),
    /// never a silent partial rename. Manufactured mismatch: a `RAISE(IGNORE)`
    /// trigger silently skips one targeted row, so the UPDATE changes fewer
    /// rows than were captured. The rollback must leave NO row renamed, NO
    /// outbox Update, NO audit row.
    #[test]
    fn changed_count_mismatch_bails_and_renames_nothing() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (cfg, _) = cfg_with_hub(dir.path(), AREAS_YAML);
        seed_entity(&conn, "ent-hub");
        merge(&conn, "ent-loser", "ent-hub");
        seed_fact(&conn, "fact-a", "ent-hub", Some("old"), 100);
        seed_fact(&conn, "fact-b", "ent-loser", Some("old"), 100);
        conn.execute_batch(
            "CREATE TRIGGER sabotage BEFORE UPDATE OF area ON llm_wiki_entries
              WHEN OLD.id = 'fact-b'
             BEGIN
               SELECT RAISE(IGNORE);
             END;",
        )
        .unwrap();

        let err = format!(
            "{}",
            areas_rename(&conn, &cfg, "ent-hub", "old", "new", true).unwrap_err()
        );
        assert!(err.contains("partial"), "must refuse a partial rename: {err}");

        let area_of = |id: &str| -> Option<String> {
            conn.query_row(
                "SELECT area FROM llm_wiki_entries WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(area_of("fact-a"), Some("old".into()), "rollback: fact-a untouched");
        assert_eq!(area_of("fact-b"), Some("old".into()), "rollback: fact-b untouched");
        assert_eq!(outbox_updates(&conn).len(), 0, "rollback stages no outbox rows");
        assert_eq!(audit_rows(&conn).len(), 0, "rollback writes no audit row");
    }
}
