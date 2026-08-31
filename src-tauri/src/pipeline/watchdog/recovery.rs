use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;

use super::heartbeat::{now_ms, Stage};

/// Strikes before a document is quarantined (spec §4.4).
pub const QUARANTINE_THRESHOLD: i64 = 2;

/// Respawns tolerated per rolling hour before parking in `degraded`
/// (spec §4.5). Also bounds leaked threads to a small constant.
pub const RESPAWN_CAP_PER_HOUR: usize = 3;

const RESPAWN_WINDOW: Duration = Duration::from_secs(3600);

/// Record a stall strike against a path. Returns the new count.
pub fn record_strike(conn: &Connection, path: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO stall_strikes (path, strikes, last_ms)
         VALUES (?1, 1, ?2)
         ON CONFLICT(path) DO UPDATE
            SET strikes = strikes + 1, last_ms = excluded.last_ms",
        rusqlite::params![path, now_ms()],
    )?;
    Ok(conn.query_row(
        "SELECT strikes FROM stall_strikes WHERE path = ?1",
        [path],
        |r| r.get(0),
    )?)
}

pub fn quarantine(conn: &Connection, path: &str) -> Result<()> {
    conn.execute(
        "UPDATE documents SET quarantined_at = ?1 WHERE path = ?2",
        rusqlite::params![now_ms(), path],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn is_quarantined(conn: &Connection, path: &str) -> Result<bool> {
    // Outer Option: row missing. Inner Option: column NULL.
    let q: Option<Option<i64>> = conn
        .query_row(
            "SELECT quarantined_at FROM documents WHERE path = ?1",
            [path],
            |r| r.get(0),
        )
        .ok();
    Ok(matches!(q, Some(Some(_))))
}

/// Which stages talk to a remote endpoint and are therefore worth probing
/// before blaming the document (spec §4.2).
pub fn stage_has_network_dependency(stage: Stage) -> bool {
    matches!(stage, Stage::Embedding | Stage::Summarizing)
}

/// Rolling-window count of worker respawns.
#[derive(Debug, Default)]
pub struct RespawnLedger {
    events: VecDeque<Instant>,
}

impl RespawnLedger {
    pub fn new() -> Self {
        RespawnLedger {
            events: VecDeque::new(),
        }
    }

    /// Records a respawn, expiring any events that have fallen outside the
    /// rolling hour window before adding the new event.
    pub fn record(&mut self) {
        self.expire(Instant::now());
        self.events.push_back(Instant::now());
    }

    /// Returns `true` when the ledger has recorded `RESPAWN_CAP_PER_HOUR`
    /// respawns within the rolling window.
    ///
    /// # Invariant
    ///
    /// This method does **not** expire stale entries. Callers must invoke
    /// `record()` first to refresh the rolling window, or use this method only
    /// immediately after a `record()` call. Using `over_cap()` on a stale
    /// ledger (one where all events are older than the window) will continue
    /// to return `true` until `record()` is called.
    pub fn over_cap(&self) -> bool {
        self.events.len() >= RESPAWN_CAP_PER_HOUR
    }

    fn expire(&mut self, now: Instant) {
        while let Some(front) = self.events.front() {
            if now.duration_since(*front) >= RESPAWN_WINDOW {
                self.events.pop_front();
            } else {
                break;
            }
        }
    }

    #[cfg(test)]
    pub fn expire_for_test(&mut self, now: Instant) {
        self.expire(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::pipeline::watchdog::heartbeat::Stage;

    fn seed_doc(conn: &rusqlite::Connection, path: &str) {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, 'h', 'user_doc', 'pending')",
            [path],
        )
        .unwrap();
    }

    #[test]
    fn strikes_accumulate_per_path() {
        let conn = open_in_memory().unwrap();
        assert_eq!(record_strike(&conn, "/a.md").unwrap(), 1);
        assert_eq!(record_strike(&conn, "/a.md").unwrap(), 2);
        assert_eq!(record_strike(&conn, "/b.md").unwrap(), 1);
    }

    #[test]
    fn quarantine_marks_the_document_and_is_readable() {
        let conn = open_in_memory().unwrap();
        seed_doc(&conn, "/a.md");
        assert!(!is_quarantined(&conn, "/a.md").unwrap());

        quarantine(&conn, "/a.md").unwrap();
        assert!(is_quarantined(&conn, "/a.md").unwrap());
    }

    #[test]
    fn only_network_stages_are_probed() {
        assert!(stage_has_network_dependency(Stage::Embedding));
        assert!(stage_has_network_dependency(Stage::Summarizing));
        for stage in [
            Stage::Reading,
            Stage::Extracting,
            Stage::Chunking,
            Stage::Linking,
            Stage::Committing,
            Stage::Deleting,
            Stage::Idle,
        ] {
            assert!(
                !stage_has_network_dependency(stage),
                "{:?} must not be probed",
                stage
            );
        }
    }

    #[test]
    fn respawn_ledger_trips_the_cap() {
        let mut ledger = RespawnLedger::new();
        for _ in 0..RESPAWN_CAP_PER_HOUR {
            assert!(!ledger.over_cap());
            ledger.record();
        }
        assert!(ledger.over_cap(), "cap must trip once exhausted");
    }

    #[test]
    fn respawn_ledger_forgets_entries_older_than_an_hour() {
        let mut ledger = RespawnLedger::new();
        let start = Instant::now();

        // Fill the ledger.
        for _ in 0..RESPAWN_CAP_PER_HOUR {
            ledger.record();
        }
        assert!(ledger.over_cap());

        // Simulate the rolling window elapsing: expire at start + 1 hour.
        let elapsed = start + RESPAWN_WINDOW + Duration::from_secs(1);
        ledger.expire_for_test(elapsed);

        // `record()` is the caller's entry point — it expires stale entries before
        // checking. After expiration a single `record()` pops all old events and
        // pushes one new one, so the window is clear and `over_cap()` is false.
        ledger.record();
        assert!(!ledger.over_cap(), "old respawns must age out");
    }
}
