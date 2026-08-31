# Ingest Drain-Stall Watchdog — Design

**Date:** 2026-08-31
**Status:** Implemented (PR landed; per-task reviews + fix-rounds captured in `.superpowers/sdd/2026-08-31-ingest-drain-stall-watchdog/`)
**Branch:** `spec/ingest-drain-stall-watchdog`
**Priority:** P2

## 1. Problem

On 2026-08-29 the ingest pipeline stopped making progress and stayed stuck for
92+ minutes with the screen unlocked. Only an application restart recovered it,
and the restart did not durably fix the condition.

### 1.1 Evidence

Established from journald (13:00–16:00 EDT, PIDs 131713, 134542, 152704,
155270) and from incident triage. An evidence inventory was deposited to the
operator's vault at `agents/operations/ct-drain-stall-evidence-inventory-2026-08-31.md`
on the incident host; it is not reachable from the development machine, so this
spec restates the findings rather than citing the file.

- The pipeline worker emitted **nothing** during the window — no panic, no DB
  error, no stage trace. The log establishes only that ingest made no progress.
- The only output came from the watcher thread, which kept running: transient
  `[watch] enqueue_vault_event failed ... .hermes-tmp.*` read errors.
- `errors.log` in the vault did not cover the incident (stale, mtime 2026-08-25);
  its 221 backlog entries date from an earlier misconfiguration.
- **No stack sample was captured** while the process was stuck.
- Three app instances ran across the window. Ingest was silent in all three. The
  outbox worker partially revived after each restart (12 → 4 rows). The pending
  count was frozen at 83. The session was unlocked throughout.

### 1.2 What the code actually does

Traced through `src-tauri/src/pipeline/mod.rs`, `src-tauri/src/lib.rs`,
`src-tauri/src/db/queue.rs`, and the `tools` crate. **The pending count and the
pipeline worker are on two disconnected paths**, which reframes the incident:

- The "83 pending" is `SELECT COUNT(*) FROM documents WHERE status = 'pending'`
  (`db/queries.rs:130-136`) — a database row count, not the worker's
  `AtomicUsize`.
- The file watcher and the startup reconcile pass stage work by writing
  `documents` rows with `status = 'pending'` via `enqueue_vault_event`
  (`lib.rs:1097`, `:947`, `:977`). They never send a `PipelineJob`. They only
  set `flags.ingesting = true` optimistically (`lib.rs:975`, `:1077`).
- Both `PipelineJob` producers (`lib.rs:1502` in `queue_full_reindex`, `:1711`
  in `run_wiki_reembed`) source their paths from `list_indexed_user_doc_paths`,
  which filters `WHERE tier = 'user_doc' AND status = 'indexed'`
  (`db/queries.rs:38-40`) — excluding pending rows by construction.
- The headless CLI does not close the gap either: `ct ingest`
  (`tools/src/cmds.rs:98`) walks the vault from disk rather than reading the
  pending rows, and no query in `tools` selects `documents` by
  `status = 'pending'`.

**No component anywhere consumes `documents.status = 'pending'`.** Rows written
by the watcher are cleared only if some other path happens to re-ingest that
same file.

This is a structural gap, not a stall: a worker whose channel is empty is
correctly parked in `recv()`, so 83 pending rows beside a silent worker is the
expected steady state of the current design. It also explains "restart did not
durably fix" more directly than a poison document does — nothing about a restart
creates a consumer. A stack sample, had one been taken, would most likely have
shown a worker idle in `recv()`.

### 1.3 Why the worker still cannot be supervised

Independently of §1.2, the worker has no failure detection or recovery:

- `PipelineWorker` is a single OS thread in a blocking `rx.recv()` loop
  (`pipeline/mod.rs:170`), with no heartbeat and no per-job deadline.
- `catch_unwind` (`pipeline/mod.rs:191`) covers panics; nothing covers hangs.
- The channel is `mpsc::sync_channel(256)` (`pipeline/mod.rs:678`) with blocking
  `send`. Once full, `queue_full_reindex` (`lib.rs:1502`) blocks on a Tauri IPC
  thread and freezes the frontend bridge.
- Shutdown is `drop(tx); join()` (`lib.rs:1253`), unbounded, so `switch_vault`
  inherits a wedge instead of recovering from it.
- Every network stage has a ceiling (embed 120s at `embedder/mod.rs:86`, Ollama
  600s at `embedder/ollama.rs:21`, generation `timeout_secs` default 600 at
  `librarian/synthesis.rs:138`), so a 92-minute stall cannot be one HTTP call.

### 1.4 Two failure classes

The design must cover both, because §1.2 establishes the first and §1.3 leaves
the second undetectable:

1. **Silent non-consumption** — work is queued but nothing drains it. This is
   what the evidence supports. Detected by queue-depth liveness (§2.3), fixed by
   the drainer (§5).
2. **Worker stall** — a job is picked up and never completes. Not established by
   the evidence, but currently invisible and unrecoverable. Detected by the stage
   heartbeat (§2.1–2.2), recovered by the ladder (§4).

### 1.5 Non-goals

- Identifying a specific wedged call from the 2026-08-29 logs. §3 makes the next
  occurrence diagnosable instead.
- Moving ingest out of process (see §8).
- Rewriting the pipeline on an async runtime (see §8).
- Reworking `ct ingest`'s disk-walk model. §5 adds a consumer for pending rows;
  it does not unify the two ingest entrypoints.

## 2. Detection

### 2.1 Stage heartbeat

The worker publishes progress to a shared `Heartbeat`, updated at every stage
transition.

```rust
pub struct Heartbeat {
    epoch: AtomicU64,            // worker generation; see §4
    seq: AtomicU64,              // bumped on every transition; seqlock key
    stage: AtomicU8,             // Stage discriminant
    stage_started_ms: AtomicI64,
    subject: Mutex<Option<String>>, // path, or entity id while Linking
}
```

`Stage` is: `Idle`, `Reading`, `Extracting`, `Chunking`, `Embedding`,
`Summarizing`, `Linking`, `Committing`, `Deleting`.

`Deleting` covers `PipelineJob::Delete`, which removes the converted shadow
file, calls `delete_document`, and runs an unindexed
`UPDATE wiki_pages ... WHERE source_doc_ids LIKE '%path%'`
(`pipeline/mod.rs:229-256`). That scan is a plausible stall site under lock
contention and must not be an unmodeled hole in the stage map.

**Consistent snapshot reads (seqlock).** `epoch`, `seq`, `stage`, and
`stage_started_ms` are independent atomics; independent loads can interleave
with a concurrent `enter()` and combine fields from different transitions —
the classic torn-read hazard that produces a false trip or a missed stall.
`Heartbeat::snapshot()` therefore implements a seqlock with the full writer
protocol: `enter()` bumps `seq` to an **odd** value to open the write window,
writes `subject`, `stage_started_ms` and `stage`, then bumps `seq` **even**
again to close it. `snapshot()` reads `seq`, loads the remaining fields,
re-reads `seq`, and accepts the read only when the two `seq` values are equal
*and* even — a reader that lands mid-write sees either an odd `seq` or two
different values, and retries. All `enter()` and snapshot reads use
`Ordering::SeqCst` to make the protocol well-defined on every supported
architecture.

`subject` is written **inside** the window and read with `try_lock` per the
next paragraph, degrading to `"unknown"` on contention; keeping it inside the
window is what stops a snapshot from pairing a new stage with the previous
document's path and striking an innocent file.

The retry budget is bounded (`SNAPSHOT_RETRY_BUDGET`) so a writer that races
continuously cannot stall the supervisor. Exhausting it does **not** yield a
usable snapshot: the returned value carries `consistent: false`, and the
supervisor skips the tick entirely rather than deciding a trip on fields that
may straddle two transitions.

**The supervisor MUST NOT block on `subject`.** It reads with `try_lock` and
degrades to `"unknown"` on contention. A wedged worker can be stalled while
holding that mutex, and a supervisor that waits on it would deadlock against
precisely the condition it exists to detect. Only the atomics are load-bearing
for trip decisions; `subject` is diagnostic detail. (An `ArcSwap` slot is an
acceptable alternative implementation; a blocking `lock()` in the supervisor is
not.)

### 2.2 Stage budgets

Each stage carries its own budget, because the stages have very different
legitimate durations and because the stage identity is itself the diagnostic.

| Stage | Budget | Rationale |
|---|---|---|
| `Idle` | never trips | Blocked on `recv()` with an empty channel is correct |
| `Reading` | 60s | Local file I/O |
| `Extracting` | 300s | `pdf_extract` / docx have no internal ceiling |
| `Chunking` | 120s | CPU-bound, bounded by file size |
| `Embedding` | active profile's HTTP timeout + 60s | See below |
| `Summarizing` | `generation.timeout_secs` + 60s | Config-driven (`synthesis.rs:259`) |
| `Linking` | 60s per entity | Per entity, not per flush batch |
| `Committing` | 60s | SQLite writes |
| `Deleting` | 120s | Unindexed `LIKE` scan over `wiki_pages` |

The embed budget must be derived from the **active** embed profile. The two
timeouts are hardcoded literals today — `Duration::from_secs(120)`
(`embedder/mod.rs:86`) for the external profile and `600`
(`embedder/ollama.rs:21`) for Ollama — and neither is configurable. Computing
one budget from the 120s literal would false-trip every Ollama embed at roughly
three minutes. The implementation therefore lifts both literals into named
constants that the watchdog reads per profile; making them configurable is
optional and out of scope.

`Linking` is budgeted per entity because `flush_pending_linkers` runs *between*
jobs — after the pending decrement, and again after the loop exits
(`pipeline/mod.rs:280-296`, and again at `:328-340`) — iterating `run_linker`
over a batch of entity ids whose size is unbounded. The worker enters `Linking`
once per entity and writes that entity id to `subject`, so a batch of 50
entities is 50 budgeted spans, not one. During `Linking` the heartbeat subject
is an entity id, not a path.

A supervisor thread ticks every 5s and trips when
`now - stage_started_ms` exceeds the current stage's budget.

### 2.3 Queue-depth liveness

The stage heartbeat cannot detect §1.4 class 1: a worker that consumes nothing
is legitimately `Idle`, and `Idle` never trips. A second, independent check
covers it.

The supervisor trips a **drain-stall** when, for a continuous 15 minutes:

- `count_pending_documents(conn) > 0`, and
- no document has transitioned to `indexed` or `error`, and
- the heartbeat stage has been `Idle` throughout.

This is the check that would have fired on 2026-08-29. It is deliberately
independent of the worker's internal state: it observes the queue from the
outside and asks whether the system as a whole is making progress. Its recovery
is §5's drainer sweep, not a worker respawn — respawning a healthy idle worker
accomplishes nothing.

The 15-minute window must exceed the longest legitimate single-document time
(`Extracting` 300s + `Embedding` up to 660s ≈ 16 minutes for a worst-case Ollama
PDF). Because the third condition requires `Idle` throughout, a long-running
document cannot trip it regardless of duration; the window only needs to
tolerate scheduling jitter.

### 2.4 Database location

All watchdog tables live in the brain database resolved by
`retrieval::resolve_brain_paths().db_path` — the same connection the worker
already opens. Implementations MUST NOT join `.brain` onto the vault root: there
are three decoy `.brain` directories under the operator's vault, and
`lib.rs:1201-1204` records a prior bug where a hardcoded `~/.brain/brain.db`
diverged from the resolver and operated on the wrong database.

The heartbeat is mirrored to a single-row `pipeline_heartbeat` table at most
every 5s so the state survives to post-mortem and is readable by external
tooling and the headless CLI.

## 3. Diagnostics before recovery

Recovery destroys evidence, so capture strictly precedes it. On any trip, before
any recovery step:

1. Insert a `pipeline_stalls` row: trip kind (`stage_stall` or `drain_stall`),
   stage, subject, `stalled_ms`, heartbeat `seq` and `epoch`, pending count, the
   resolved embed and generation endpoints, and the recovery action about to be
   taken. The insert runs on a **dedicated diagnostic connection** (separate
   from the supervisor's main connection) with a bounded busy timeout (≤ 5s)
   so that lock contention on the brain SQLite — most likely from a
   `Committing` or `Deleting` job the watchdog is itself about to recover —
   cannot stall recovery. If the insert fails or times out, emit the structured
   stderr line below and proceed with the recovery action anyway: lost
   diagnostics must never delay a respawn or sweep.
2. Emit one structured stderr line
   (`[watchdog] <kind> stage=... subject=... ms=... pending=...`) so journald
   captures it in the stream the incident was reconstructed from.
3. For `stage_stall` only, capture thread stacks of the running process into the
   journal.

Stack capture is best-effort and platform-dependent; failure to capture MUST NOT
block recovery. It is the artifact whose absence limited the 2026-08-29 triage,
so it is a first-class requirement rather than a debug aid.

## 4. Worker-stall recovery

Applies to a `stage_stall` trip (§1.4 class 2). Steps 1–3 run in sequence within
a single trip; steps 4 and 5 are thresholds evaluated during that sequence, not
later escalations.

1. **Diagnose.** Run §3.
2. **Probe, then attribute.** Probe the dependency the stalled stage uses — the
   embedding endpoint for `Embedding`, the generation endpoint for
   `Summarizing`. Stages with no external dependency (`Reading`, `Extracting`,
   `Chunking`, `Linking`) skip the probe and are treated as healthy. `Committing`
   and `Deleting` use the **shared** brain SQLite; they probe the diagnostic
   connection with a bounded busy timeout, and a stalled or contended database
   records an **unattributed system strike** rather than incrementing
   `stall_strikes` for the current path. If the probe passes, record a strike
   against the current subject (`stall_strikes` keyed by path); if it fails,
   the document is not at fault and no path strike is recorded. Probing before
   attributing keeps a dead endpoint (or a contended shared DB) from
   accumulating strikes against innocent documents.
3. **Respawn.** Bump the shared epoch, abandon the wedged thread, rebuild the
   channel, and spawn a fresh worker at the new epoch. Requeue undrained jobs via
   §5's sweep.

   The rebuilt channel makes **publishing the new sender** part of the step, not
   an afterthought: the abandoned worker still owns the *old* receiver, so any
   producer — including the supervisor's own sweep — left holding the old sender
   enqueues into a queue nobody drains, which is the wedge this step exists to
   clear. The supervisor therefore obtains the replacement's sender from
   `on_replace_worker` and adopts it before sweeping, and the same call
   republishes it to the shared `PipelineHandle` that every IPC producer reads.
   Because the replacement starts with an empty in-flight set, the supervisor
   clears the claim set at the same moment. If the replacement cannot be
   spawned, the pipeline parks in `degraded` (step 5) rather than reporting a
   recovery that did not happen.
4. **Quarantine threshold.** A document reaching 2 strikes is marked
   `quarantined`, skipped by the sweep, and surfaced in the UI, guaranteeing
   forward progress when one poison file is the trigger. A successful ingest
   completion (status transitions to `indexed`) **clears prior strikes for that
   path**, so a replacement file at the same path starts with no inherited
   strikes. Equivalently, a content-identity change (different `hash`) keys a
   fresh strike ledger row — both definitions produce the desired invariant;
   the implementation uses the first because it piggybacks on the existing
   completion event.
5. **Degrade threshold.** Respawns are capped at 3 per rolling hour. Past the cap
   the pipeline parks in `degraded`, stops respawning, and shows a persistent
   banner. The cap prevents a respawn loop against an unhealthy dependency and
   bounds leaked threads to a small constant.

### 4.1 Epoch guard for abandoned workers

Rust cannot kill a thread, so an abandoned worker is detached and leaks, holding
its `rusqlite` connection until process exit. It may also **wake up later** — a
stalled socket eventually returns — and it still holds `pending:
Arc<AtomicUsize>`, `status_tx`, and the old `rx`. Left unguarded it would resume
draining jobs against the same database as its replacement. Because `ingest_file`
clears a document's chunks and then re-inserts
(`pipeline/mod.rs:617-622`), a woken zombie calling `insert_chunk` /
`insert_embedding` for a document the new worker has already re-ingested produces
duplicate or orphaned chunks.

Every worker therefore captures its epoch at spawn and compares it against the
shared epoch at each stage transition. On mismatch the worker returns
immediately without touching the database, the pending counter, or `status_tx`.
The check costs one relaxed atomic load per transition.

The guard must cover **every** transition, including the ones published from
inside `ingest_file_virtual` (`Reading`, `Extracting`, `Chunking`, `Embedding`,
`Committing`) — a job that passes the check at the top of the worker loop can
still be superseded partway through. Code holding a bare `&Heartbeat` could
publish past supersession and mask a stall on the *replacement* worker, so all
in-job transitions go through `StageReporter`, which owns the epoch comparison
and returns `false` once superseded; the caller must then return immediately.
Standalone callers with no watchdog-managed worker (tooling, Tauri commands)
use `StageReporter::unguarded`, which is never superseded.

## 5. Pending-row drainer

§1.2 establishes that nothing consumes `documents.status = 'pending'`. This spec
adds that consumer, because §4.3's requeue step cannot exist without a
pending-row-to-job path.

A **sweep** selects `documents` rows where `status` is `'pending'` or
`'pending_reindex'` and the path is not quarantined, ordered by rowid, and
enqueues a job for each. The status decides which job: `'pending'` becomes a
`PipelineJob::ingest_counted` (`force: false`), while `'pending_reindex'` —
staged by `queue_full_reindex` / `run_wiki_reembed` when the channel was full
— becomes a `PipelineJob::rechunk_for_reembed` (`force: true`). Carrying that
distinction is load-bearing: re-enqueueing a deferred reindex as a plain
ingest lets the unchanged-hash check short-circuit, silently dropping the
chunk-strategy or embedding-model upgrade the user asked for.

`documents.status` admits `'pending_reindex'` as of schema V15; the original
CHECK constraint did not, so every staging write failed and the deferral was
lost.

The sweep also tracks an **in-flight claim set** owned by the supervisor: a
row whose path is currently in the set is skipped on subsequent passes, so a
long `Extracting` or `Embedding` job does not get re-enqueued by the next 60s
sweep. A claim is dropped when:

- `try_send` returns `QueueFull` (the job was *not* enqueued — leave the row
  `pending` for the next pass);
- the worker is replaced (the abandoned in-flight set is gone with the worker);
- **the row leaves the sweepable set**, which is how the supervisor infers
  completion. Nothing signals job completion back to the watchdog, so each
  sweep first expires claims for paths that are no longer `pending` /
  `pending_reindex`. Without this the claim set grows to channel capacity and
  never shrinks, and — worse — any path swept once is skipped by every later
  sweep, disabling the very backstop this section describes.

It runs:

- at pipeline startup, so rows staged while the app was closed are picked up;
- after a worker respawn (§4.3), to requeue work the abandoned worker held;
- on a `drain_stall` trip (§2.3), which is its recovery action;
- on a periodic timer (60s), which is what makes the watcher's staged rows flow
  in normal operation.

The sweep is bounded per invocation by the channel's remaining capacity and uses
`try_send` (§6), so it never blocks; leftover rows stay `pending` and are picked
up on the next pass. Enqueuing is idempotent: re-ingesting a document that is
already correct is a no-op via the hash check at `pipeline/mod.rs:616-620`, and a
partially-ingested document is cleaned by `delete_document_chunks` before
re-chunking, so a requeue after an abandoned mid-ingest is safe.

The sweep does not change how `ct ingest` or `queue_full_reindex` select their
work; it adds the missing path only.

## 6. Producer backpressure

The blocking `send` cannot freeze the watcher — the watcher writes to the
database and never touches the channel (§1.2) — so this is hardening rather than
incident causation. It still matters: `queue_full_reindex` blocks a Tauri IPC
thread when the channel fills, and shutdown's unbounded join blocks
`switch_vault`.

- Producers move from `send` to `try_send`, returning a typed `QueueFull` error
  instead of blocking.
- `queue_full_reindex` reports "queued N of M" rather than freezing its IPC
  thread. The unqueued remainder stays `pending` and is picked up by §5's sweep,
  so `QueueFull` defers work rather than dropping it.
- Shutdown's unbounded `join()` becomes a bounded wait. On timeout the epoch is
  bumped, the worker detached, and shutdown proceeds, so `switch_vault` recovers
  rather than inheriting the wedge.

## 7. Status surface

`WikiStatusFlags.ingesting: bool` is replaced by a status enum:

| Value | Meaning |
|---|---|
| `idle` | No pending rows, worker healthy |
| `working` | Actively processing; carries stage and subject |
| `stalled` | Watchdog tripped; recovery in progress |
| `degraded` | Respawn cap exhausted; ingest parked, manual action needed |

`stalled` and `degraded` carry the trip kind, stage, and subject so the frontend
renders an actionable banner instead of a spinner that never resolves.
Quarantined documents are listable so the user can inspect and re-enqueue them.

`idle` is derived from the pending count, not from the worker's stage. Under the
current code an idle worker beside 83 pending rows renders as idle-and-fine,
which is exactly how the incident stayed invisible.

This is a breaking change to the status payload. Affected call sites, all updated
in the same change:

- `WikiStatusFlags` (`lib.rs:111`) — currently `#[derive(Clone, Copy)]`. Carrying
  a stage and subject means it can no longer be `Copy`; the derive narrows to
  `Clone` and the write sites adjust accordingly.
- Backend writers: `lib.rs:864` (status-event forwarder), `lib.rs:975`,
  `lib.rs:1077`, and the JSON serialization at `lib.rs:124`. The two optimistic
  `flags.ingesting = true` writes in the watcher and reconcile paths become
  pending-count-derived rather than unconditional.
- Frontend consumers: `src/components/shell/StatusBar.tsx:27`, `:33`, `:72`, and
  `src/__tests__/useWikiStatus.test.ts`.

## 8. Approaches considered

**Out-of-process ingest sidecar.** True isolation, recovery by `kill -9`, no
leaked threads and no epoch guard needed. Rejected for now: it requires IPC,
bundling, and signing changes disproportionate to this work. It is the escalation
path if leaked threads become a real cost.

**Tokio rewrite with per-job `timeout`.** Clean cancellation for the HTTP stages,
but `rusqlite` and `pdf_extract` are blocking FFI and CPU work that
`tokio::time::timeout` cannot interrupt — the future would cancel while the
blocking thread stayed wedged. A large rewrite for partial coverage.

**In-process supervisor (chosen).** Cheap, testable, no packaging change. Its
real costs are that a wedged thread can only be abandoned (bounded by §4.5) and
that abandonment needs the epoch guard of §4.1.

## 9. Testing

Queue-depth liveness (§2.3):

- Pending rows > 0, worker `Idle`, no completions for the window → `drain_stall`.
- Pending rows > 0 with completions still occurring → no trip.
- Pending rows = 0 and worker `Idle` → no trip.
- A single document held in `Embedding` past the window → no trip, because the
  stage is not `Idle`.

Stage heartbeat and recovery (§2.1–2.2, §4):

- A fake worker sleeping past its stage budget trips; one parked in `Idle` never
  trips regardless of elapsed time.
- The embed budget is derived from the active profile: an Ollama-profile worker
  is not tripped at the external profile's 120s.
- A `Deleting` job past its budget trips.
- `Linking` is budgeted per entity: a batch of N entities each under budget does
  not trip.
- The supervisor's `subject` read does not block when the worker holds the mutex
  (`try_lock` degrades to `"unknown"`).
- `Heartbeat::snapshot()` is a seqlock: under concurrent `enter()` traffic, the
  returned `(epoch, seq, stage, stage_started_ms)` is always consistent — the
  test holds the snapshot invariant across N parallel transitions.
- A `pipeline_stalls` row exists before the respawn occurs.
- A failing dependency probe suppresses the strike; a passing probe records it.
- A stall in `Committing` or `Deleting` does not blame a path: it records an
  unattributed system strike or probes the shared SQLite dependency, so two
  shared-local failures cannot quarantine an innocent document.
- Two strikes on a path mark it `quarantined`; the sweep skips a quarantined
  path. A successful completion (or a content-identity change) clears prior
  strikes for that path so a replacement document does not inherit them.
- Exceeding the respawn cap parks the pipeline in `degraded` and halts respawns.

Epoch guard (§4.1):

- A worker whose epoch is stale exits at its next stage transition without
  writing chunks, decrementing the pending counter, or sending a status event.
- A stale worker unblocking mid-`Embedding` after its document was re-ingested
  by the replacement leaves no duplicate chunks.

Drainer and backpressure (§5, §6):

- The sweep enqueues pending rows and skips quarantined ones.
- The sweep respects channel capacity; leftover rows remain `pending` and are
  enqueued on the next pass.
- The sweep's in-flight claim set prevents re-enqueueing a path that is
  currently being processed. A long `Embedding` does not get duplicated by
  the next 60s sweep, and a `QueueFull` release returns the path to the
  pending pool for the next pass.
- Sweeping a document whose content is unchanged is a no-op.
- Sweeping a partially-ingested document produces no duplicate chunks.
- `try_send` on a full channel returns `QueueFull` rather than blocking, and
  `queue_full_reindex` reports a partial count.
- Bounded shutdown returns within its timeout when the worker is wedged.

## 10. Open items

- The stack-capture mechanism is platform-specific. The incident host is Linux;
  the primary development machine is macOS. The implementation plan must choose a
  mechanism per platform and define the no-op fallback where none is available.
- **Field confirmation of §1.2.** On the incident host, with the app healthy,
  check whether `documents` still holds pending rows that never drain. A
  non-zero, non-decreasing count while ingest otherwise works confirms the
  missing-consumer diagnosis over the worker-stall hypothesis. The design covers
  both classes either way, so this does not block implementation.
