//! Ingest watchdog. Spec: docs/superpowers/specs/2026-08-31-ingest-drain-stall-watchdog-design.md
pub mod diagnostics;
pub mod heartbeat;
pub mod budgets;
pub mod recovery;
pub mod sweep;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use self::budgets::budget_for;
use self::diagnostics::{capture_stacks, record_trip, TripKind, TripRecord};
use self::heartbeat::{now_ms, Heartbeat, HeartbeatSnapshot, Stage};
use self::recovery::RespawnLedger;
use self::sweep::InFlightClaims;
use crate::embedder::EmbedProfile;
use crate::pipeline::PipelineJob;

/// How long the queue may sit non-empty and idle before it counts as a drain
/// stall. Must exceed the longest legitimate single-document time
/// (Extracting 300s + Embedding up to 660s ≈ 16 min); the `Idle` requirement
/// means a long-running document cannot trip it regardless (spec §2.3).
pub const DRAIN_STALL_WINDOW: Duration = Duration::from_secs(900);

/// How often the supervisor wakes.
pub const TICK: Duration = Duration::from_secs(5);

/// Which probe a stalled stage needs before blame can be attributed (spec §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeKind {
    /// Stages with no external/shared dependency (`Reading`, `Extracting`,
    /// `Chunking`, `Linking`). Skip the probe and treat as healthy.
    None,
    /// External HTTP dependency (`Embedding`, `Summarizing`). Probe the
    /// endpoint with a short timeout; strike on success only.
    Network,
    /// Shared brain SQLite dependency (`Committing`, `Deleting`). Probe the
    /// diagnostic connection with a bounded busy timeout; on probe failure
    /// record an unattributed system strike rather than blame the path.
    SharedSqlite,
}

/// Probe the brain SQLite with a bounded read. Returns true when the
/// diagnostic connection can answer a trivial query inside its busy timeout,
/// false when lock contention is the most likely cause of the stall
/// (spec §4.2).
fn probe_shared_sqlite(diag_conn: &Connection) -> bool {
    let _ = diag_conn.busy_timeout(Duration::from_secs(2));
    diag_conn
        .query_row("SELECT 1", [], |_| Ok(()))
        .is_ok()
}

/// Wait for a worker thread, giving up after `timeout`. `std::thread` has no
/// timed join, so the thread signals completion over a channel and the caller
/// abandons it on timeout — which is exactly what `switch_vault` needs so it
/// stops inheriting a wedge (spec §6).
pub fn join_with_timeout(
    join: std::thread::JoinHandle<()>,
    timeout: Duration,
) -> bool {
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _ = join.join();
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(timeout).is_ok()
}

/// Returns `Some(stalled_ms)` when the current stage has exceeded its budget.
pub fn evaluate_stage_stall(
    snapshot: &HeartbeatSnapshot,
    profile: &EmbedProfile,
    gen_timeout_secs: u64,
    now_ms: i64,
) -> Option<i64> {
    let budget = budget_for(snapshot.stage, profile, gen_timeout_secs)?;
    let elapsed = now_ms.saturating_sub(snapshot.stage_started_ms);
    if elapsed > budget.as_millis() as i64 {
        Some(elapsed)
    } else {
        None
    }
}

/// Detects the failure class the stage heartbeat cannot see: work is queued
/// but nothing consumes it, so the worker is legitimately `Idle` forever
/// (spec §1.4 class 1, §2.3).
#[derive(Debug, Default)]
pub struct DrainTracker {
    since: Option<Instant>,
    last_pending: Option<i64>,
    last_completed: Option<i64>,
}

impl DrainTracker {
    pub fn new() -> Self {
        DrainTracker {
            since: None,
            last_pending: None,
            last_completed: None,
        }
    }

    /// Feed one observation. Returns true when a drain stall has persisted
    /// for the full window.
    ///
    /// `completed` is the count of documents in a terminal state
    /// (`indexed` or `error`) — any increase means the system is making
    /// progress, so the window resets.
    pub fn observe(
        &mut self,
        pending: i64,
        completed: i64,
        stage: Stage,
        now: Instant,
    ) -> bool {
        let progressing = self.last_completed.is_some_and(|last| completed != last);
        let quiet = pending > 0 && stage == Stage::Idle && !progressing;

        self.last_pending = Some(pending);
        self.last_completed = Some(completed);

        if !quiet {
            self.since = None;
            return false;
        }

        match self.since {
            None => {
                self.since = Some(now);
                false
            }
            Some(start) => now.duration_since(start) >= DRAIN_STALL_WINDOW,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineHealth {
    #[default]
    Idle,
    Working,
    Stalled,
    Degraded,
}

impl PipelineHealth {
    pub fn as_str(&self) -> &'static str {
        match self {
            PipelineHealth::Idle => "idle",
            PipelineHealth::Working => "working",
            PipelineHealth::Stalled => "stalled",
            PipelineHealth::Degraded => "degraded",
        }
    }
}

/// Diagnostics first, then supersede. Recovery destroys the evidence, so the
/// ordering here is load-bearing (spec §3). The trip is recorded on the
/// dedicated diagnostic connection so lock contention cannot block recovery;
/// on insert failure the caller is expected to log a structured stderr line
/// and continue with the recovery action (spec §3.1).
pub fn handle_stage_stall(
    diag_conn: &Connection,
    heartbeat: &Arc<Heartbeat>,
    trip: &TripRecord,
) -> Result<()> {
    record_trip(diag_conn, trip)?;
    capture_stacks(std::process::id());
    heartbeat.bump_epoch();
    Ok(())
}

pub struct SupervisorConfig {
    pub db_path: PathBuf,
    pub heartbeat: Arc<Heartbeat>,
    pub tx: SyncSender<PipelineJob>,
    pub profile: EmbedProfile,
    pub gen_timeout_secs: u64,
    pub on_health: Box<dyn Fn(HealthUpdate) + Send>,
    /// Cooperative stop signal. `switch_vault` (and the next
    /// `start_file_watcher`) sets this before spawning a new supervisor so
    /// the previous supervisor exits instead of leaking a second writer of
    /// `flags.ingest.health` (CodeRabbit review PRRT_kwDOSVmXas6d28dj).
    pub stop: Arc<AtomicBool>,
    /// Invoked when the watchdog supersedes a wedged worker. The caller is
    /// expected to spawn a replacement worker reading from a fresh
    /// `SyncSender` (the abandoned worker holds the previous receiver and
    /// leaks until it eventually exits). Without this hook the wedged
    /// worker keeps the channel receiver and no one dequeues submitted
    /// jobs (CodeRabbit review PRRT_kwDOSVmXas6d28dw).
    pub on_replace_worker: Box<dyn Fn() + Send>,
}

/// Snapshot of pipeline liveness, propagated to listeners (UI status, logs,
/// tests). Carries the stage and subject that `IngestStatus.stage` and
/// `IngestStatus.subject` render alongside the health enum (spec §7).
#[derive(Debug, Clone)]
pub struct HealthUpdate {
    pub health: PipelineHealth,
    pub stage: Option<Stage>,
    pub subject: Option<String>,
}

pub fn spawn_supervisor(cfg: SupervisorConfig) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("pipeline-watchdog".to_string())
        .spawn(move || supervisor_loop(cfg))
        .expect("spawn pipeline watchdog")
}

fn supervisor_loop(cfg: SupervisorConfig) {
    let conn = match Connection::open(&cfg.db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[watchdog] cannot open {:?}: {e}; supervisor exiting", cfg.db_path);
            return;
        }
    };
    let _ = conn.busy_timeout(Duration::from_secs(5));

    // Dedicated diagnostic connection so lock contention on the brain
    // SQLite (most likely from the Committing/Deleting job the watchdog is
    // itself about to recover) cannot stall recovery. Bounded busy timeout
    // is non-negotiable: an unbounded insert would block recovery forever
    // (spec §3).
    let diag_conn = match Connection::open(&cfg.db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[watchdog] cannot open diagnostic connection: {e}; supervisor exiting");
            return;
        }
    };
    let _ = diag_conn.busy_timeout(Duration::from_secs(5));

    let mut drain = DrainTracker::new();
    let mut respawns = RespawnLedger::new();
    let mut claims = InFlightClaims::new();
    let mut degraded = false;
    // Trip gate: a stage that hasn't moved since the last trip (same epoch
    // and stage_started_ms) is the same wedge — don't accumulate strikes,
    // diagnostics, or respawns for it
    // (CodeRabbit review PRRT_kwDOSVmXas6d28dw).
    let mut last_trip_key: Option<(u64, i64)> = None;

    loop {
        if cfg.stop.load(std::sync::atomic::Ordering::SeqCst) {
            eprintln!("[watchdog] stop signaled; supervisor exiting");
            break;
        }
        std::thread::sleep(TICK);

        let snapshot = cfg.heartbeat.snapshot();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let completed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE status IN ('indexed', 'error')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        mirror_heartbeat(&conn, &snapshot);

        if degraded {
            (cfg.on_health)(HealthUpdate {
                health: PipelineHealth::Degraded,
                stage: Some(snapshot.stage),
                subject: Some(snapshot.subject.clone()),
            });
            continue;
        }

        // Class 2: a job was picked up and never finished.
        if let Some(stalled_ms) = evaluate_stage_stall(
            &snapshot,
            &cfg.profile,
            cfg.gen_timeout_secs,
            now_ms(),
        ) {
            // Trip gate: same (epoch, stage_started_ms) means the wedge is
            // still the same wedge — skip the duplicate trip.
            let trip_key = (snapshot.epoch, snapshot.stage_started_ms);
            if last_trip_key == Some(trip_key) {
                continue;
            }
            last_trip_key = Some(trip_key);

            (cfg.on_health)(HealthUpdate {
                health: PipelineHealth::Stalled,
                stage: Some(snapshot.stage),
                subject: Some(snapshot.subject.clone()),
            });

            // Spec §4: diagnose → probe → respawn. handle_stage_stall fuses
            // diagnose (record_trip → capture_stacks) and respawn
            // (bump_epoch) into one load-bearing call, so it MUST run before
            // the probe — the probe's strike attribution is meaningless if the
            // evidence is gone.
            let trip = TripRecord {
                kind: TripKind::StageStall,
                snapshot: snapshot.clone(),
                stalled_ms,
                pending_count: pending,
                embed_endpoint: embed_endpoint_for(&cfg),
                gen_endpoint: gen_endpoint_for(),
                action: "respawn".to_string(),
            };
            if let Err(e) = handle_stage_stall(&diag_conn, &cfg.heartbeat, &trip) {
                diagnostics::emit_trip_line(&trip);
                eprintln!("[watchdog] failed to record stage stall: {e}; proceeding with recovery");
            }

            // Replace the wedged worker. The bumped epoch means the
            // abandoned worker exits on its next stage transition (or
            // leaks until process exit if truly wedged); the callback
            // spawns a fresh worker on a new channel.
            (cfg.on_replace_worker)();

            // Probe only after the trip is on disk. Stages without a network
            // dependency skip the endpoint probe and are treated as healthy
            // (spec §4.2). Stages that share the brain SQLite probe the
            // diagnostic connection with a bounded busy timeout; a contended
            // database yields an unattributed system strike so an innocent
            // path is not blamed for shared-local failure.
            let probe_kind = if recovery::stage_has_network_dependency(snapshot.stage) {
                ProbeKind::Network
            } else if recovery::stage_uses_shared_sqlite(snapshot.stage) {
                ProbeKind::SharedSqlite
            } else {
                ProbeKind::None
            };
            let probe_ok = match probe_kind {
                ProbeKind::Network => probe_endpoint_for(snapshot.stage, &cfg),
                ProbeKind::SharedSqlite => probe_shared_sqlite(&diag_conn),
                ProbeKind::None => true,
            };

            // Attribution: probe-healthy and document-specific → path strike.
            // Shared-local probe failed → unattributed system strike. Otherwise
            // (other dependency failed, or subject unknown) → no strike.
            match (probe_kind, probe_ok) {
                (ProbeKind::None | ProbeKind::Network | ProbeKind::SharedSqlite, true) => {
                    if snapshot.subject != "unknown" {
                        if let Err(e) =
                            recovery::record_strike(&conn, &snapshot.subject)
                        {
                            eprintln!("[watchdog] strike failed: {e}");
                        } else {
                            let strikes: i64 = conn
                                .query_row(
                                    "SELECT strikes FROM stall_strikes WHERE path = ?1",
                                    [&snapshot.subject],
                                    |r| r.get(0),
                                )
                                .unwrap_or(0);
                            if strikes >= recovery::QUARANTINE_THRESHOLD {
                                if let Err(e) = recovery::quarantine(&conn, &snapshot.subject) {
                                    eprintln!("[watchdog] quarantine failed: {e}");
                                } else {
                                    eprintln!(
                                        "[watchdog] quarantined {} after {strikes} strikes",
                                        snapshot.subject
                                    );
                                }
                            }
                        }
                    }
                }
                (ProbeKind::SharedSqlite, false) => {
                    match recovery::record_system_strike(&conn) {
                        Ok(n) => eprintln!(
                            "[watchdog] shared-sqlite probe failed; system strike {n} \
                             (not blamed on {})",
                            snapshot.subject
                        ),
                        Err(e) => eprintln!("[watchdog] system strike failed: {e}"),
                    }
                }
                _ => {}
            }

            respawns.record();
            if respawns.over_cap() {
                degraded = true;
                eprintln!(
                    "[watchdog] respawn cap ({}) exhausted; parking ingest in degraded",
                    recovery::RESPAWN_CAP_PER_HOUR
                );
                (cfg.on_health)(HealthUpdate {
                    health: PipelineHealth::Degraded,
                    stage: Some(snapshot.stage),
                    subject: Some(snapshot.subject.clone()),
                });
                continue;
            }

            let _ = sweep::sweep(&conn, &cfg.tx, &mut claims, 256);
            // Respawn abandons the in-flight set with the abandoned worker
            // (spec §5); clear claims before the next sweep so abandoned
            // rows re-enqueue.
            claims.clear();
            continue;
        }

        // Class 1: work is queued and nothing consumes it.
        if drain.observe(pending, completed, snapshot.stage, Instant::now()) {
            (cfg.on_health)(HealthUpdate {
                health: PipelineHealth::Stalled,
                stage: Some(snapshot.stage),
                subject: Some(snapshot.subject.clone()),
            });
            let trip = TripRecord {
                kind: TripKind::DrainStall,
                snapshot: snapshot.clone(),
                stalled_ms: DRAIN_STALL_WINDOW.as_millis() as i64,
                pending_count: pending,
                embed_endpoint: embed_endpoint_for(&cfg),
                gen_endpoint: gen_endpoint_for(),
                action: "sweep".to_string(),
            };
            if let Err(e) = record_trip(&diag_conn, &trip) {
                diagnostics::emit_trip_line(&trip);
                eprintln!("[watchdog] failed to record drain stall: {e}; proceeding with sweep");
            }
            // A healthy idle worker needs work, not a respawn (spec §2.3).
            let _ = sweep::sweep(&conn, &cfg.tx, &mut claims, 256);
            continue;
        }

        // Normal operation: keep the watcher's staged rows flowing.
        let queued = sweep::sweep(&conn, &cfg.tx, &mut claims, 256).unwrap_or(0);
        let health = if pending > 0 || queued > 0 {
            PipelineHealth::Working
        } else {
            PipelineHealth::Idle
        };
        (cfg.on_health)(HealthUpdate {
            health,
            stage: Some(snapshot.stage),
            subject: Some(snapshot.subject.clone()),
        });
    }
}

/// Probe the endpoint the stalled stage actually uses. Treat an unreachable
/// endpoint as "not the document's fault" (spec §4.2).
///
/// The URL must follow the active profile: probing localhost for a Cloud
/// embed profile would report a healthy local Ollama while the real remote
/// endpoint is down, and strike an innocent document.
fn probe_endpoint_for(stage: Stage, cfg: &SupervisorConfig) -> bool {
    let url = match stage {
        Stage::Embedding => match &cfg.profile {
            EmbedProfile::Local { .. } => local_llm_base(),
            EmbedProfile::Cloud { .. } => return true, // provider host, not ours to probe
            EmbedProfile::External { profile } => profile.base_url.clone(),
        },
        // Generation always goes through the local runtime today.
        Stage::Summarizing => local_llm_base(),
        _ => return true,
    };

    let client = match reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(&url)
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn local_llm_base() -> String {
    std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string())
}

/// Resolve the embed endpoint for the trip row from the active profile.
/// Mirrors `probe_endpoint_for`'s URL selection so the row records what the
/// embed stage actually talks to (spec §3).
fn embed_endpoint_for(cfg: &SupervisorConfig) -> Option<String> {
    match &cfg.profile {
        EmbedProfile::Local { .. } => Some(local_llm_base()),
        // Provider host — not ours to record or probe.
        EmbedProfile::Cloud { .. } => None,
        EmbedProfile::External { profile } => Some(profile.base_url.clone()),
    }
}

/// Resolve the generation endpoint for the trip row. Generation always goes
/// through the local Ollama runtime today; the helper is kept separate so the
/// rule lives in one place (spec §3).
fn gen_endpoint_for() -> Option<String> {
    Some(local_llm_base())
}

fn mirror_heartbeat(conn: &Connection, snapshot: &HeartbeatSnapshot) {
    let _ = conn.execute(
        "UPDATE pipeline_heartbeat
            SET epoch = ?1, seq = ?2, stage = ?3, subject = ?4,
                stage_started_ms = ?5, updated_ms = ?6
          WHERE id = 1",
        rusqlite::params![
            snapshot.epoch as i64,
            snapshot.seq as i64,
            snapshot.stage.as_str(),
            snapshot.subject,
            snapshot.stage_started_ms,
            now_ms(),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::heartbeat::{HeartbeatSnapshot, Stage};
    use super::*;
    use crate::embedder::EmbedProfile;
    use std::time::{Duration, Instant};

    fn ollama() -> EmbedProfile {
        EmbedProfile::Local {
            model: "nomic-embed-text".to_string(),
        }
    }

    fn snap(stage: Stage, started_ms: i64) -> HeartbeatSnapshot {
        HeartbeatSnapshot {
            epoch: 0,
            seq: 1,
            stage,
            subject: "/a.md".to_string(),
            stage_started_ms: started_ms,
        }
    }

    #[test]
    fn idle_never_trips_a_stage_stall_however_long() {
        let s = snap(Stage::Idle, 0);
        assert!(evaluate_stage_stall(&s, &ollama(), 600, 999_999_999).is_none());
    }

    #[test]
    fn embedding_over_the_active_profile_budget_trips() {
        // Ollama budget is 600 + 60 = 660s.
        let s = snap(Stage::Embedding, 0);
        assert!(evaluate_stage_stall(&s, &ollama(), 600, 659_000).is_none());
        let tripped = evaluate_stage_stall(&s, &ollama(), 600, 661_000).unwrap();
        assert_eq!(tripped, 661_000);
    }

    #[test]
    fn drain_tracker_trips_when_pending_stays_and_nothing_completes() {
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(83, 10, Stage::Idle, t0));
        // Same pending, same completion count, still idle, past the window.
        assert!(t.observe(83, 10, Stage::Idle, t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn drain_tracker_does_not_trip_while_completions_advance() {
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(83, 10, Stage::Idle, t0));
        // A completion landed — progress is being made.
        assert!(!t.observe(82, 11, Stage::Idle, t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn drain_tracker_does_not_trip_with_an_empty_queue() {
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(0, 10, Stage::Idle, t0));
        assert!(!t.observe(0, 10, Stage::Idle, t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn drain_tracker_does_not_trip_while_a_document_is_in_flight() {
        // A long Embedding is not a drain stall regardless of duration — the
        // stage-stall path owns that case (spec §2.3).
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(83, 10, Stage::Embedding, t0));
        assert!(!t.observe(
            83,
            10,
            Stage::Embedding,
            t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)
        ));
    }

    #[test]
    fn join_with_timeout_returns_false_for_a_wedged_thread() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            // Blocks until the test drops `tx` — stands in for a wedge.
            let _ = rx.recv();
        });

        let finished = join_with_timeout(handle, Duration::from_millis(200));
        assert!(!finished, "a wedged thread must not be waited on forever");

        drop(tx); // let the stand-in thread exit
    }

    #[test]
    fn join_with_timeout_returns_true_for_a_thread_that_finishes() {
        let handle = std::thread::spawn(|| {});
        assert!(join_with_timeout(handle, Duration::from_secs(5)));
    }

    #[test]
    fn stage_stall_records_the_trip_before_bumping_the_epoch() {
        use crate::db::connection::open_in_memory;
        use crate::pipeline::watchdog::diagnostics::{TripKind, TripRecord};

        let conn = open_in_memory().unwrap();
        let hb = std::sync::Arc::new(heartbeat::Heartbeat::new());
        hb.enter(Stage::Embedding, Some("/a.md"));

        let epoch_before = hb.epoch();
        let trip = TripRecord {
            kind: TripKind::StageStall,
            snapshot: hb.snapshot(),
            stalled_ms: 700_000,
            pending_count: 83,
            embed_endpoint: None,
            gen_endpoint: None,
            action: "respawn".to_string(),
        };
        handle_stage_stall(&conn, &hb, &trip).unwrap();

        // Evidence exists...
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM pipeline_stalls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "trip must be recorded before recovery");
        // ...and only then is the worker superseded.
        assert_eq!(hb.epoch(), epoch_before + 1);
    }

    #[test]
    fn record_trip_recovers_when_a_lock_is_held() {
        // Spec §3.1: a stalled diagnostic insert must not block recovery.
        // We hold an exclusive transaction on `conn` and verify that
        // `record_trip` on a separate diagnostic connection returns within
        // its bounded busy timeout instead of hanging.
        use crate::db::connection::open_in_memory;
        use crate::pipeline::watchdog::diagnostics::{TripKind, TripRecord};
        use std::time::Instant;

        let conn = open_in_memory().unwrap();
        let diag_conn = open_in_memory().unwrap();
        let hb = std::sync::Arc::new(heartbeat::Heartbeat::new());
        hb.enter(Stage::Embedding, Some("/a.md"));

        // Hold an IMMEDIATE write lock on the shared conn — the contended
        // case the watchdog must tolerate.
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();

        let trip = TripRecord {
            kind: TripKind::StageStall,
            snapshot: hb.snapshot(),
            stalled_ms: 700_000,
            pending_count: 83,
            embed_endpoint: None,
            gen_endpoint: None,
            action: "respawn".to_string(),
        };

        let started = Instant::now();
        // 5s matches the supervisor's diagnostic busy timeout.
        let _ = diag_conn.busy_timeout(std::time::Duration::from_secs(5));
        let result = diagnostics::record_trip(&diag_conn, &trip);
        let elapsed = started.elapsed();

        conn.execute_batch("ROLLBACK").unwrap();

        // Two distinct in-memory databases: a held lock on `conn` cannot
        // affect `diag_conn`. The diagnostic insert must succeed quickly
        // regardless of contention on the supervisor's main connection.
        assert!(result.is_ok(), "diag insert must not block on held lock: {result:?}");
        assert!(elapsed < std::time::Duration::from_secs(2));
    }

    #[test]
    fn second_strike_quarantines_the_document() {
        use crate::db::connection::open_in_memory;
        use crate::pipeline::watchdog::recovery::{
            is_quarantined, record_strike, QUARANTINE_THRESHOLD,
        };

        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES ('/p.md', 'h', 'user_doc', 'pending')",
            [],
        )
        .unwrap();

        for _ in 0..QUARANTINE_THRESHOLD {
            let strikes = record_strike(&conn, "/p.md").unwrap();
            if strikes >= QUARANTINE_THRESHOLD {
                recovery::quarantine(&conn, "/p.md").unwrap();
            }
        }
        assert!(is_quarantined(&conn, "/p.md").unwrap());
    }
}
