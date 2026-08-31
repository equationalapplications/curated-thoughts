# Ingest Drain-Stall Watchdog — Design

**Date:** 2026-08-31
**Status:** Approved for planning
**Branch:** `spec/ingest-drain-stall-watchdog`
**Priority:** P2

## 1. Problem

On 2026-08-29 the ingest pipeline worker stopped consuming its queue and stayed
wedged for 92+ minutes with the screen unlocked. Only an application restart
recovered it, and the restart did not durably fix the condition.

### 1.1 Evidence

Established from journald (13:00–16:00 EDT, PIDs 131713, 134542, 152704,
155270) and from incident triage. An evidence inventory was deposited to the
operator's vault at `agents/operations/ct-drain-stall-evidence-inventory-2026-08-31.md`
on the incident host; it is not reachable from the development machine, so this
spec restates the findings rather than citing the file.

- The pipeline worker emitted **nothing** during the wedge — no panic, no DB
  error, no stage trace. The log narrows the fault to "the worker thread stopped
  consuming the queue" and can say nothing more.
- The only output in the window came from the watcher thread, which kept
  running: transient `[watch] enqueue_vault_event failed ... .hermes-tmp.*`
  read errors.
- `errors.log` in the vault did not cover the incident (stale, mtime 2026-08-25);
  its 221 backlog entries date from an earlier misconfiguration.
- **No stack sample was ever captured** while the process was wedged. This is the
  single artifact that would have identified the wedged call.
- Three app instances ran across the window. The ingest worker was silent in all
  three. The outbox worker partially revived after each restart (12 → 4 rows).
  The pending counter was frozen at 83. The session was unlocked throughout.

Because a full process restart did not durably fix the condition, the fault
outlives the worker thread. The plausible classes are a poison document in the
83 pending rows that re-wedges each fresh worker, or an unhealthy dependency
(embedding or generation endpoint black-holing, or a lock held by another
connection). The evidence cannot separate them, so the design must handle both.

### 1.2 Why the current code cannot recover

Read from `src-tauri/src/pipeline/mod.rs` and `src-tauri/src/lib.rs`:

- `PipelineWorker` is a single OS thread in a blocking `rx.recv()` loop
  (`pipeline/mod.rs:170`). There is no heartbeat and no per-job deadline.
- `catch_unwind` (`pipeline/mod.rs:191`) covers panics but nothing covers hangs.
- The channel is `mpsc::sync_channel(256)` (`pipeline/mod.rs:678`) with blocking
  `send`. Once it fills, every producer blocks forever — including
  `queue_full_reindex` (`lib.rs:1502`), which runs on a Tauri IPC thread. A
  wedged worker therefore wedges the frontend bridge.
- Shutdown is `drop(tx); join()` (`lib.rs:1253`) with an unbounded join, so
  `switch_vault` inherits the wedge instead of recovering from it.
- The only status signal is `flags.ingesting = count > 0` (`lib.rs:862`), derived
  from an in-memory counter. A worker that wedges mid-job leaves it above zero
  permanently — the observed frozen 83.
- Every network stage already has a ceiling (embed 120s at
  `embedder/mod.rs:86`, Ollama 600s at `embedder/ollama.rs:21`, generation
  `timeout_secs` default 600 at `librarian/synthesis.rs:138`). A 92-minute stall
  therefore cannot be one HTTP call.

### 1.3 Non-goals

- Identifying the specific 2026-08-29 root cause. The design is generic
  detection plus recovery; §3 makes the *next* occurrence diagnosable.
- Moving ingest out of process (see §7).
- Rewriting the pipeline on an async runtime (see §7).

## 2. Detection — heartbeat and per-stage budgets

The worker publishes progress to an `Arc<Heartbeat>` of atomics, updated at every
stage transition. Lock-free, so a stalled reader can never block the worker and
the worker's own cost is negligible.

```rust
pub struct Heartbeat {
    seq: AtomicU64,          // bumped on every transition
    stage: AtomicU8,         // Stage discriminant
    stage_started_ms: AtomicI64,
    current_path: Mutex<Option<String>>, // written only on transition
}
```

`Stage` is: `Idle`, `Reading`, `Extracting`, `Chunking`, `Embedding`,
`Summarizing`, `Linking`, `Committing`.

Each stage carries its own budget rather than one global timeout, because the
stages have wildly different legitimate durations and because the stage
identity is the diagnostic:

| Stage | Budget | Rationale |
|---|---|---|
| `Idle` | never trips | Blocked on `recv()` with an empty queue is correct behavior |
| `Reading` | 60s | Local file I/O |
| `Extracting` | 300s | `pdf_extract` / docx have no internal ceiling |
| `Chunking` | 120s | CPU-bound, bounded by file size |
| `Embedding` | configured embed timeout + 60s | Slack above the HTTP ceiling |
| `Summarizing` | `generation.timeout_secs` + 60s | Slack above the HTTP ceiling |
| `Linking` | 60s | SQLite; a longer wait implies lock contention |
| `Committing` | 60s | SQLite writes |

A supervisor thread ticks every 5s and trips when `now - stage_started_ms`
exceeds the current stage's budget. Exempting `Idle` is what makes an empty
queue distinguishable from a wedged worker — the distinction the incident logs
could not draw, and the reason `ingest_runs` (per-document) is insufficient.

The heartbeat is additionally mirrored to a single-row `pipeline_heartbeat`
table at most every 5s, so the state survives to post-mortem and is readable by
external tooling and the headless CLI.

### 2.1 Database location

All watchdog tables live in the brain database resolved by
`retrieval::resolve_brain_paths().db_path` — the same connection the worker
already opens. Implementations MUST NOT join `.brain` onto the vault root:
there are three decoy `.brain` directories under the operator's vault, and
`lib.rs:1201-1204` records a prior bug where a hardcoded `~/.brain/brain.db`
diverged from the resolver and operated on the wrong database.

## 3. Diagnostics before recovery

Recovery destroys the evidence, so capture strictly precedes it. On trip, before
any escalation step:

1. Insert a `pipeline_stalls` row: stage, current path, `stalled_ms`, heartbeat
   `seq`, the resolved embed and generation endpoints, and the escalation action
   about to be taken.
2. Emit one structured stderr line (`[watchdog] stall stage=... path=... ms=...`)
   so journald captures it in the same stream the incident was reconstructed
   from.
3. Capture thread stacks of the running process into the journal.

Stack capture is best-effort and platform-dependent; failure to capture MUST NOT
block escalation. It is the artifact whose absence blocked the 2026-08-29
diagnosis, so it is a first-class requirement rather than a debug aid.

## 4. Recovery ladder

Escalation is ordered; each step runs only if the previous did not clear the
stall.

1. **Trip.** Run §3 diagnostics.
2. **Probe the dependency** used by the stalled stage — the embedding endpoint
   for `Embedding`, the generation endpoint for `Summarizing`. Stages with no
   network dependency (`Reading`, `Extracting`, `Chunking`, `Linking`,
   `Committing`) skip the probe and are treated as healthy. If the probe passes,
   record a strike against the current document (`stall_strikes` keyed by path);
   if it fails, the document is not at fault and no strike is recorded. The
   probe therefore runs *before* the strike, so a dead endpoint never
   accumulates strikes against innocent documents.
3. **Respawn.** Abandon the wedged thread (Rust cannot kill a thread; the
   thread is detached and leaks, holding its `rusqlite` connection until process
   exit). Rebuild the channel, spawn a fresh worker, requeue undrained jobs, and
   **reconcile the pending counter from the database** rather than trusting the
   in-memory `AtomicUsize`. The reconcile is what clears a frozen counter like
   the observed 83.
4. **Quarantine.** A document that accumulates 2 strikes is marked
   `quarantined`, skipped when jobs are requeued, and surfaced in the UI. This
   guarantees forward progress when a single poison file is the trigger —
   directly addressing the observation that restarting did not durably fix the
   condition.
5. **Degrade.** Respawns are capped at 3 per rolling hour. Past the cap the
   pipeline parks in a `degraded` state, stops respawning, and shows a
   persistent banner. The cap prevents a respawn loop against an unhealthy
   dependency and bounds the number of leaked threads to a small constant.

## 5. Unblocking producers

Independent of the watchdog, the blocking channel is what converts a worker
stall into a whole-application freeze, and it must be fixed in the same change —
otherwise the watchdog can detect a stall while callers remain frozen and
`switch_vault` still cannot recover.

- Producers move from `send` to `try_send`, returning a typed `QueueFull` error
  instead of blocking.
- `queue_full_reindex` reports "queued N of M" rather than freezing its IPC
  thread.
- Shutdown's unbounded `join()` becomes a bounded wait. On timeout the worker is
  detached and shutdown proceeds, so `switch_vault` recovers rather than
  inheriting the wedge.

## 6. Status surface

`WikiStatusFlags.ingesting: bool` is replaced by a status enum:

| Value | Meaning |
|---|---|
| `idle` | Queue empty, worker healthy |
| `working` | Actively processing; carries stage and path |
| `stalled` | Watchdog tripped; recovery in progress |
| `degraded` | Respawn cap exhausted; ingest parked, manual action needed |

`stalled` and `degraded` carry the stalled stage and path so the frontend can
render an actionable banner instead of a spinner that never resolves. Quarantined
documents are listable so the user can inspect and re-enqueue them.

This is a breaking change to the status payload. Affected call sites, all
updated in the same change:

- `WikiStatusFlags` (`lib.rs:111`) — currently `#[derive(Clone, Copy)]`. Carrying
  a stage and path means it can no longer be `Copy`; the derive is narrowed to
  `Clone` and the write sites adjusted accordingly.
- Backend writers: `lib.rs:864` (the status-event forwarder), `lib.rs:975`,
  `lib.rs:1077`, and the JSON serialization at `lib.rs:124`.
- Frontend consumers: `src/components/shell/StatusBar.tsx:27`, `:33`, `:72`, and
  `src/__tests__/useWikiStatus.test.ts`.

## 7. Approaches considered

**Out-of-process ingest sidecar.** True isolation, recovery by `kill -9`, no
leaked threads. Rejected for now: it requires IPC, bundling, and signing changes
disproportionate to a single incident. This is the escalation path if leaked
threads become a real cost.

**Tokio rewrite with per-job `timeout`.** Clean cancellation for the HTTP
stages, but `rusqlite` and `pdf_extract` are blocking FFI and CPU work that
`tokio::time::timeout` cannot interrupt — the future would cancel while the
blocking thread stayed wedged. A large rewrite for partial coverage.

**In-process supervisor (chosen).** Cheap, testable, no packaging change. Its
one real cost is that a wedged thread can only be abandoned, not killed; the
respawn cap in §4.5 bounds that cost.

## 8. Testing

- A fake worker that sleeps past its stage budget trips the watchdog.
- A worker parked in `Idle` never trips, regardless of elapsed time.
- Trip ordering: a `pipeline_stalls` row exists before the respawn occurs.
- Two strikes on the same path mark it `quarantined`; a quarantined path is
  skipped on requeue.
- A failing dependency probe suppresses the strike.
- Exceeding the respawn cap parks the pipeline in `degraded` and halts further
  respawns.
- `try_send` on a full channel returns `QueueFull` rather than blocking.
- Bounded shutdown returns within its timeout when the worker is wedged.
- Pending-counter reconcile derives from the database, not the in-memory
  counter.

## 9. Open items

- The stack-capture mechanism is platform-specific. The incident host is Linux;
  the primary development machine is macOS. The implementation plan must choose
  a mechanism per platform and define the no-op fallback where none is
  available.
