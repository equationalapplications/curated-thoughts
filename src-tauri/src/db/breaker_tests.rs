//! Task 10 (issue #271 spec §10 + §8(b)): tests for the cross-process
//! bulk-deletion breaker. Lives as its own `db` test module so the breaker
//! contract is reviewable without opening the (large) writer files.
//!
//! Covered here, per the plan's Step 1 list:
//! - threshold math `max(⌈0.05·L⌉, 10)` on BOTH denominators;
//! - cross-process budget (two `Connection`s on one file, shared refusal);
//! - Small-L rows (rev-34 rule: `|doomed| >= L` refuses at ANY L; partial
//!   batches stay budget-governed);
//! - tripped epoch does NOT roll over (24h rollover only for healthy epochs);
//! - `breaker_reset` clears every breaker key;
//! - refusal reasons name the threshold that was hit.

#[cfg(test)]
mod tests {
    use crate::db::breaker::{
        breaker_check, breaker_reset, breaker_spend, breaker_spent, heal_denominator,
        heal_threshold, regrade_denominator, regrade_threshold, BreakerVerdict, BREAKER_L_HEAL,
        BREAKER_L_REGRADE,
    };
    use crate::db::connection::{open_app_db, open_in_memory};
    use rusqlite::params;
    use tempfile::TempDir;

    const BREAKER_SPENT_KEY: &str = "breaker_spent";
    const BREAKER_EPOCH_TS_KEY: &str = "breaker_epoch_ts";
    const BREAKER_TRIPPED_KEY: &str = "breaker_tripped";

    /// Seed `live` grounded + `ungrounded` vanished-path librarian_inferred
    /// rows. Grounded rows carry a legacy path ref backed by a `documents`
    /// row; ungrounded rows point at documents that do not exist.
    fn seed_selection(conn: &rusqlite::Connection, live: usize, ungrounded: usize) {
        for i in 0..live {
            let path = format!("documents/keep{i}.md");
            conn.execute(
                "INSERT INTO documents (path, hash, tier, status)
                 VALUES (?1, 'h', 'user_doc', 'indexed')",
                [&path],
            )
            .unwrap();
            insert_entry(conn, &format!("keep{i}"), &path);
        }
        for i in 0..ungrounded {
            insert_entry(
                conn,
                &format!("lost{i}"),
                &format!("documents/vanished{i}.md"),
            );
        }
    }

    fn insert_entry(conn: &rusqlite::Connection, id: &str, source_ref: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_ref, created_at, updated_at, deleted_at
             ) VALUES (?1, 'tier_fact', ?2, 'body', '[]', 'inferred',
                       'librarian_inferred', ?3, 1, 1, NULL)",
            params![id, format!("Title {id}"), source_ref],
        )
        .unwrap();
    }

    /// Seed `n` live evidence rows (one per fresh live entry) so the regrade
    /// denominator (librarian_evidence JOIN live entries) counts them.
    fn seed_evidence_population(conn: &rusqlite::Connection, n: usize) {
        for i in 0..n {
            let id = format!("fact{i}");
            insert_entry(conn, &id, &format!("documents/f{i}.md"));
            conn.execute(
                "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json,
                     unanchored, created_at)
                 VALUES (?1, ?2, ?3, 0, 1)",
                params![id, format!("prop_{id}"), r#"{"evidence":[]}"#],
            )
            .unwrap();
        }
    }

    fn meta(conn: &rusqlite::Connection, key: &str) -> Option<String> {
        conn.query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .ok()
    }

    // ── Threshold math: max(⌈0.05·L⌉, 10) on BOTH denominators ──────────────

    #[test]
    fn heal_threshold_is_max_ceil_5pct_of_selection_floor_10() {
        // L=3 → ceil(0.15)=1 → floor wins → 10.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 3);
        assert_eq!(heal_denominator(&conn).unwrap(), 3);
        assert_eq!(heal_threshold(&conn).unwrap(), 10);

        // L=7 → ceil(0.35)=1 → 10 (the spec's Small-L test row).
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 7);
        assert_eq!(heal_threshold(&conn).unwrap(), 10);

        // L=50 → ceil(2.5)=3 → floor wins → 10.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 50, 0);
        assert_eq!(heal_threshold(&conn).unwrap(), 10);

        // L=400 → ceil(20)=20 → formula wins → 20.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 400, 0);
        assert_eq!(heal_threshold(&conn).unwrap(), 20);

        // Soft-deleted rows and NULL/other-type refs are OUT of the denominator.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 2, 2);
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id = 'lost0'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET source_ref = NULL WHERE id = 'lost1'",
            [],
        )
        .unwrap();
        assert_eq!(heal_denominator(&conn).unwrap(), 2);
    }

    #[test]
    fn regrade_threshold_uses_librarian_evidence_join_denominator() {
        // L = COUNT(librarian_evidence JOIN live entries) — a DIFFERENT
        // denominator than heal's (spec §10 pins both). L=3 → 10.
        let conn = open_in_memory().unwrap();
        seed_evidence_population(&conn, 3);
        assert_eq!(regrade_denominator(&conn).unwrap(), 3);
        assert_eq!(regrade_threshold(&conn).unwrap(), 10);

        // Evidence rows on SOFT-deleted entries do not count.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id = 'fact0'",
            [],
        )
        .unwrap();
        assert_eq!(regrade_denominator(&conn).unwrap(), 2);

        // L=400 → ceil(20)=20.
        let conn = open_in_memory().unwrap();
        seed_evidence_population(&conn, 400);
        assert_eq!(regrade_threshold(&conn).unwrap(), 20);
    }

    // ── Cross-process budget: two Connections, one file, shared refusal ─────

    #[test]
    fn budget_is_shared_across_two_connections_on_one_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("brain.db");
        let c1 = open_app_db(&path, None).unwrap();
        let c2 = open_app_db(&path, None).unwrap();

        // L=12 → threshold max(⌈0.6⌉,10) = 10.
        seed_selection(&c1, 0, 12);
        assert_eq!(heal_threshold(&c2).unwrap(), 10);

        // Writer 1 spends the whole budget through ITS OWN connection.
        breaker_spend(&c1, 10).unwrap();

        // Writer 2 (a fresh Connection on the same file) must see the trip:
        // even a small batch (|doomed|=2 < threshold) is refused through the
        // shared counter — no TOCTOU across processes (spec §10).
        let own_l = heal_denominator(&c2).unwrap();
        let verdict =
            breaker_check(&c2, 2, own_l, heal_threshold(&c2).unwrap(), BREAKER_L_HEAL).unwrap();
        match verdict {
            BreakerVerdict::Refuse { reason } => {
                assert!(reason.contains("TRIPPED"), "reason: {reason}");
                assert!(
                    reason.contains("10"),
                    "reason must name the threshold: {reason}"
                );
            }
            BreakerVerdict::Allow => panic!("shared budget must refuse across processes"),
        }

        // The epoch L snapshots are visible on the other connection too.
        assert_eq!(meta(&c2, BREAKER_L_HEAL).as_deref(), Some("12"));
    }

    // ── Small-L rows (rev-34): |doomed| >= L refuses at ANY L ────────────────

    #[test]
    fn small_l_full_population_batches_refuse_and_partial_proceeds() {
        // L=7, |doomed|=7 ⇒ refuse.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 7);
        let verdict =
            breaker_check(&conn, 7, 7, heal_threshold(&conn).unwrap(), BREAKER_L_HEAL).unwrap();
        assert!(
            matches!(verdict, BreakerVerdict::Refuse { .. }),
            "L=7 with the whole selection doomed must refuse"
        );

        // L=3, |doomed|=3 ⇒ refuse.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 3);
        let verdict =
            breaker_check(&conn, 3, 3, heal_threshold(&conn).unwrap(), BREAKER_L_HEAL).unwrap();
        assert!(matches!(verdict, BreakerVerdict::Refuse { .. }));

        // L=50, |doomed|=50 ⇒ refuse (the budget floor alone would NOT refuse
        // here — 50 > 10 would also trip the budget, but the pinned rule is
        // the full-population one and its refusal reason must say so).
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 50);
        let verdict = breaker_check(
            &conn,
            50,
            50,
            heal_threshold(&conn).unwrap(),
            BREAKER_L_HEAL,
        )
        .unwrap();
        match verdict {
            BreakerVerdict::Refuse { reason } => {
                assert!(reason.contains("|doomed|"), "reason: {reason}");
            }
            BreakerVerdict::Allow => panic!("L=50 full-population batch must refuse"),
        }

        // L=50, |doomed|=4 ⇒ proceeds (budget-governed: 4 ≤ 10, rev-34 row).
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 46, 4);
        let verdict =
            breaker_check(&conn, 4, 50, heal_threshold(&conn).unwrap(), BREAKER_L_HEAL).unwrap();
        assert!(
            matches!(verdict, BreakerVerdict::Allow),
            "partial batch within budget must proceed"
        );
    }

    // ── Epoch window: tripped epochs NEVER roll over ─────────────────────────

    #[test]
    fn tripped_epoch_does_not_roll_over_but_healthy_epochs_do() {
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 10); // L=10 → threshold 10

        // First check initializes the epoch (healthy, spent 0).
        let verdict = breaker_check(&conn, 1, 10, 10, BREAKER_L_HEAL).unwrap();
        assert!(matches!(verdict, BreakerVerdict::Allow));
        assert!(meta(&conn, BREAKER_EPOCH_TS_KEY).is_some());

        let now_ms = crate::db::commit::ms_now();

        // HEALTHY rollover: age the epoch past 24h with spent below threshold
        // → the check rolls it over (spent resets, ts re-snaps, L re-snaps).
        breaker_spend(&conn, 3).unwrap();
        conn.execute(
            "UPDATE llm_wiki_meta SET value = ?1 WHERE key = ?2",
            params![
                (now_ms - 25 * 60 * 60 * 1000).to_string(),
                BREAKER_EPOCH_TS_KEY
            ],
        )
        .unwrap();
        let verdict = breaker_check(&conn, 1, 10, 10, BREAKER_L_HEAL).unwrap();
        assert!(matches!(verdict, BreakerVerdict::Allow));
        assert_eq!(
            breaker_spent(&conn).unwrap(),
            0,
            "healthy rollover resets spent"
        );
        let ts: i64 = meta(&conn, BREAKER_EPOCH_TS_KEY).unwrap().parse().unwrap();
        assert!(
            now_ms - ts < 60 * 1000,
            "healthy rollover must re-stamp the epoch start (ts={ts})"
        );
        assert_eq!(meta(&conn, BREAKER_L_HEAL).as_deref(), Some("10"));

        // TRIPPED rollover refusal: spend to the threshold, age the epoch,
        // and check again — the epoch must NOT roll over (spent stays at the
        // threshold, the old ts stays) until an operator resets.
        breaker_spend(&conn, 10).unwrap();
        let backdated = (now_ms - 25 * 60 * 60 * 1000).to_string();
        conn.execute(
            "UPDATE llm_wiki_meta SET value = ?1 WHERE key = ?2",
            params![backdated, BREAKER_EPOCH_TS_KEY],
        )
        .unwrap();
        let verdict = breaker_check(&conn, 1, 10, 10, BREAKER_L_HEAL).unwrap();
        match verdict {
            BreakerVerdict::Refuse { reason } => assert!(reason.contains("TRIPPED")),
            BreakerVerdict::Allow => panic!("tripped epoch must refuse"),
        }
        assert_eq!(
            breaker_spent(&conn).unwrap(),
            10,
            "a tripped epoch keeps spent at the threshold"
        );
        assert_eq!(
            meta(&conn, BREAKER_EPOCH_TS_KEY).as_deref(),
            Some(backdated.as_str()),
            "a tripped epoch must NOT roll over"
        );
    }

    /// Fix round (task-reviewer IMPORTANT 1): the epoch's trip-ness is a
    /// PERSISTED, SHARED fact — when ANY writer refuses, the epoch is marked
    /// tripped, and no writer may roll it over. Concrete hole: regrade
    /// (L=100 → threshold 10) trips at spent=10; heal (L=1000 → threshold 50)
    /// checking >24h later used to infer "healthy" from 10 < 50, ROLL the
    /// epoch, and reset spent to 0 — erasing the trip and reopening a fresh
    /// budget. B must be refused (naming ITS threshold), the epoch must NOT
    /// roll, spent must NOT reset.
    #[test]
    fn a_marked_tripped_epoch_refuses_a_higher_threshold_writer_and_never_rolls() {
        let conn = open_in_memory().unwrap();

        // Writer A (regrade, L=100 → threshold 10) spends to its threshold…
        breaker_spend(&conn, 10).unwrap();
        // …and its next batch is refused (spent 10 >= threshold 10). ANY
        // writer's refusal must MARK the epoch tripped.
        match breaker_check(&conn, 1, 100, 10, BREAKER_L_REGRADE).unwrap() {
            BreakerVerdict::Refuse { .. } => {}
            BreakerVerdict::Allow => panic!("writer A at its threshold must refuse"),
        }
        assert!(
            meta(&conn, BREAKER_TRIPPED_KEY).is_some(),
            "ANY writer's refusal must mark the epoch tripped"
        );

        // Age the epoch past the healthy-rollover window (>24h).
        let now_ms = crate::db::commit::ms_now();
        let backdated = (now_ms - 25 * 60 * 60 * 1000).to_string();
        conn.execute(
            "UPDATE llm_wiki_meta SET value = ?1 WHERE key = ?2",
            params![backdated, BREAKER_EPOCH_TS_KEY],
        )
        .unwrap();

        // Writer B (heal, L=1000 → threshold 50): spent 10 < 50, but the
        // epoch is MARKED tripped — B must be refused, naming ITS threshold.
        let verdict = breaker_check(&conn, 1, 1000, 50, BREAKER_L_HEAL).unwrap();
        match verdict {
            BreakerVerdict::Refuse { reason } => {
                assert!(reason.contains("TRIPPED"), "reason: {reason}");
                assert!(
                    reason.contains("50"),
                    "refusal must name writer B's OWN threshold: {reason}"
                );
            }
            BreakerVerdict::Allow => {
                panic!("a marked-tripped epoch must refuse every writer, not roll over")
            }
        }
        assert_eq!(
            breaker_spent(&conn).unwrap(),
            10,
            "a marked-tripped epoch must NOT reset spent"
        );
        assert_eq!(
            meta(&conn, BREAKER_EPOCH_TS_KEY).as_deref(),
            Some(backdated.as_str()),
            "a marked-tripped epoch must NOT roll over"
        );
    }

    // ── Reset surface: `ct heal --reset-breaker` semantics ───────────────────

    #[test]
    fn breaker_reset_clears_epoch_ts_spent_and_both_l_keys() {
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 12);
        breaker_check(&conn, 1, 12, 10, BREAKER_L_HEAL).unwrap();
        breaker_spend(&conn, 4).unwrap();
        for key in [BREAKER_EPOCH_TS_KEY, BREAKER_SPENT_KEY, BREAKER_L_HEAL] {
            assert!(meta(&conn, key).is_some(), "{key} must exist before reset");
        }
        // Regrade's L key: present only because regrade ran a check too.
        crate::db::breaker::breaker_check(&conn, 0, 5, 10, crate::db::breaker::BREAKER_L_REGRADE)
            .unwrap();
        assert!(meta(&conn, BREAKER_L_REGRADE).is_some());

        breaker_reset(&conn).unwrap();

        for key in [
            BREAKER_EPOCH_TS_KEY,
            BREAKER_SPENT_KEY,
            BREAKER_L_HEAL,
            BREAKER_L_REGRADE,
        ] {
            assert!(meta(&conn, key).is_none(), "{key} must be gone after reset");
        }

        // A post-reset check starts a FRESH epoch: Allow again, keys re-seeded.
        let verdict = breaker_check(&conn, 1, 12, 10, BREAKER_L_HEAL).unwrap();
        assert!(matches!(verdict, BreakerVerdict::Allow));
        assert!(meta(&conn, BREAKER_EPOCH_TS_KEY).is_some());
        assert_eq!(meta(&conn, BREAKER_L_HEAL).as_deref(), Some("12"));
        assert_eq!(
            meta(&conn, BREAKER_L_REGRADE),
            None,
            "regrade's key only appears when regrade itself runs a check"
        );
    }

    // ── Refusal messages name the threshold that was hit ─────────────────────

    #[test]
    fn refusal_reasons_name_the_hit_threshold() {
        // Budget/trip refusal: names spent vs threshold.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 10);
        breaker_spend(&conn, 10).unwrap();
        let verdict = breaker_check(&conn, 2, 10, 10, BREAKER_L_HEAL).unwrap();
        match verdict {
            BreakerVerdict::Refuse { reason } => {
                assert!(
                    reason.contains("10"),
                    "threshold number in reason: {reason}"
                );
                assert!(
                    !reason.contains("100"),
                    "threshold must not accidentally pass on '100': {reason}"
                );
                assert!(reason.contains("spent"), "reason: {reason}");
            }
            BreakerVerdict::Allow => panic!("tripped breaker must refuse"),
        }

        // Full-population refusal: names |doomed| and L.
        let conn = open_in_memory().unwrap();
        seed_selection(&conn, 0, 5);
        let verdict =
            breaker_check(&conn, 5, 5, heal_threshold(&conn).unwrap(), BREAKER_L_HEAL).unwrap();
        match verdict {
            BreakerVerdict::Refuse { reason } => {
                assert!(reason.contains("|doomed|"), "reason: {reason}");
                assert!(
                    reason.contains("L 5") || reason.contains("L=5"),
                    "reason: {reason}"
                );
            }
            BreakerVerdict::Allow => panic!("full-population batch must refuse"),
        }
    }

    #[test]
    fn breaker_spent_defaults_to_zero_on_a_fresh_brain() {
        let conn = open_in_memory().unwrap();
        assert_eq!(breaker_spent(&conn).unwrap(), 0);
        breaker_spend(&conn, 2).unwrap();
        assert_eq!(breaker_spent(&conn).unwrap(), 2);
        breaker_spend(&conn, 3).unwrap();
        assert_eq!(
            breaker_spent(&conn).unwrap(),
            5,
            "spend accumulates on the shared counter"
        );
    }
}
