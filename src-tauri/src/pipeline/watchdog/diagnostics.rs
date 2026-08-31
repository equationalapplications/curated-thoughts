use anyhow::Result;
use rusqlite::Connection;

use super::heartbeat::{now_ms, HeartbeatSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripKind {
    StageStall,
    DrainStall,
}

impl TripKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TripKind::StageStall => "stage_stall",
            TripKind::DrainStall => "drain_stall",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TripRecord {
    pub kind: TripKind,
    pub snapshot: HeartbeatSnapshot,
    pub stalled_ms: i64,
    pub pending_count: i64,
    pub embed_endpoint: Option<String>,
    pub gen_endpoint: Option<String>,
    pub action: String,
}

/// Persist a trip. MUST be called before any recovery action — recovery
/// destroys the evidence (spec §3).
///
/// `conn` must be the **dedicated diagnostic connection** with a bounded busy
/// timeout (≤ 5s). The supervisor opens a separate connection for this purpose
/// so that lock contention on the brain SQLite cannot block recovery (spec §3).
/// Failures are returned to the caller, which must log a structured stderr
/// line and continue with the recovery action rather than abort.
pub fn record_trip(conn: &Connection, trip: &TripRecord) -> Result<i64> {
    conn.execute(
        "INSERT INTO pipeline_stalls
            (tripped_ms, kind, stage, subject, stalled_ms, heartbeat_seq,
             epoch, pending_count, embed_endpoint, gen_endpoint, action)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            now_ms(),
            trip.kind.as_str(),
            trip.snapshot.stage.as_str(),
            trip.snapshot.subject,
            trip.stalled_ms,
            trip.snapshot.seq as i64,
            trip.snapshot.epoch as i64,
            trip.pending_count,
            trip.embed_endpoint,
            trip.gen_endpoint,
            trip.action,
        ],
    )?;

    emit_trip_line(trip);
    Ok(conn.last_insert_rowid())
}

/// Emit the structured journald line for a trip. Always reachable even when
/// the SQLite insert failed, so the operator still sees the trip in the
/// stream the 2026-08-29 incident was reconstructed from (spec §3.2).
pub fn emit_trip_line(trip: &TripRecord) {
    eprintln!(
        "[watchdog] {} stage={} subject={} ms={} pending={} action={}",
        trip.kind.as_str(),
        trip.snapshot.stage.as_str(),
        trip.snapshot.subject,
        trip.stalled_ms,
        trip.pending_count,
        trip.action,
    );
}

/// Best-effort thread-stack capture into the journal. Failure MUST NOT block
/// recovery (spec §3) — every path here swallows its error.
pub fn capture_stacks(pid: u32) {
    #[cfg(target_os = "linux")]
    let attempt = std::process::Command::new("eu-stack")
        .args(["-p", &pid.to_string()])
        .output();

    #[cfg(target_os = "macos")]
    let attempt = std::process::Command::new("sample")
        .args([&pid.to_string(), "1", "-mayDie"])
        .output();

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let attempt: std::io::Result<std::process::Output> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "stack capture unsupported on this platform",
    ));

    match attempt {
        Ok(out) if out.status.success() => {
            eprintln!(
                "[watchdog] stacks pid={pid}\n{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
        Ok(out) => {
            eprintln!(
                "[watchdog] stack capture pid={pid} exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Err(e) => {
            eprintln!("[watchdog] stack capture unavailable for pid={pid}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::pipeline::watchdog::heartbeat::{HeartbeatSnapshot, Stage};

    fn snap() -> HeartbeatSnapshot {
        HeartbeatSnapshot {
            epoch: 2,
            seq: 41,
            stage: Stage::Embedding,
            subject: "/vault/documents/a.md".to_string(),
            stage_started_ms: 1_000,
        }
    }

    #[test]
    fn record_trip_persists_every_field() {
        let conn = open_in_memory().unwrap();
        let trip = TripRecord {
            kind: TripKind::StageStall,
            snapshot: snap(),
            stalled_ms: 700_000,
            pending_count: 83,
            embed_endpoint: Some("http://localhost:11434".to_string()),
            gen_endpoint: None,
            action: "respawn".to_string(),
        };

        let id = record_trip(&conn, &trip).unwrap();
        assert!(id > 0);

        let (kind, stage, subject, stalled, seq, epoch, pending, action): (
            String, String, String, i64, i64, i64, i64, String,
        ) = conn
            .query_row(
                "SELECT kind, stage, subject, stalled_ms, heartbeat_seq, epoch,
                        pending_count, action
                   FROM pipeline_stalls WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?,
                        r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(kind, "stage_stall");
        assert_eq!(stage, "embedding");
        assert_eq!(subject, "/vault/documents/a.md");
        assert_eq!(stalled, 700_000);
        assert_eq!(seq, 41);
        assert_eq!(epoch, 2);
        assert_eq!(pending, 83);
        assert_eq!(action, "respawn");
    }

    #[test]
    fn capture_stacks_never_panics_on_a_bogus_pid() {
        // Best-effort by contract: failure must not block recovery (§3).
        capture_stacks(u32::MAX);
    }
}
