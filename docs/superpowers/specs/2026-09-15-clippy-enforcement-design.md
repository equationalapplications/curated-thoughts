# Clippy enforcement: triage the backlog, make CI blocking (issue #175)

**Status:** Design approved 2026-09-15. Ready for implementation plan.
**Follow-up to:** PR 4 of the ingest integrity wave
(`2026-09-04-ingest-integrity-wave-design.md` §5), which added clippy to CI
warn-only and named this flip as its own follow-up.

---

## §1 — Context

PR 4 made clippy visible but deliberately non-blocking
(`continue-on-error: true`, no `-D warnings`) because the then-current backlog
was untriaged. Issue #175 asks for the flip, gated on triaging every finding.

**The issue's premise is stale.** Its four named findings no longer exist on
`main`. Everything in this spec is based on a fresh verification run.

## §2 — Verified baseline (2026-09-15, clippy 1.95.0, rolling stable)

### 2.1 The issue's four findings are resolved

| Issue finding | Why it is gone |
| --- | --- |
| `cloned_ref_to_slice_refs` ×2 (`reconcile.rs:225`, `:341`) | `reconcile.rs` was rewritten by the rename-reconciliation wave (issue #159); the lints no longer fire. |
| `&PathBuf` instead of `&Path` | Same rewrite. |
| `OutboxOperation::Delete` dead_code | Now constructed in 6+ sites (`db/commit.rs:564`, `db/wisdom.rs:401`, `db/bundle_apply.rs:770`/`:784`, evidence regrade/repair) via the wiki_forget replication work and PR #188's tx+outbox delete arm. |

### 2.2 Current findings

`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
--features test-utils` (CI's exact flags) reports **exactly 3 warnings**:

| # | Lint | Location | Triage |
| --- | --- | --- | --- |
| 1 | `bool_assert_comparison` | `wiki_graph.rs:1380` (test) | Style. Fix: `assert!(!result.partitions_truncated)`. |
| 2 | `bool_assert_comparison` | `wiki_graph.rs:1466` (test) | Style. Fix: `assert!(result.partitions_truncated)`. |
| 3 | `bool_assert_comparison` | `wiki_graph.rs:1622` (test) | Style. Fix: `assert!(result.truncated)`. |

`cargo clippy --manifest-path tools/Cargo.toml --all-targets` reports one
additional distinct warning:

| # | Lint | Location | Triage |
| --- | --- | --- | --- |
| 4 | `print_literal` | `tools/src/bin/tier_backfill.rs:65` | Style. Fix: inline `"TIER"` into the format string. |

No finding is a genuine defect. No lint needs an `#[allow]`; the crate's
existing deliberate allows (`too_many_arguments`, `if_same_then_else`) do not
fire as warnings and are untouched.

### 2.3 `PipelineJob::Delete` — real dead code, invisible under CI flags

Building `tools/` compiles the src-tauri lib as a path dependency **without**
the `test-utils` feature, which surfaces a warning CI's flags hide:

```
warning: variant `Delete` is never constructed
  --> src-tauri/src/pipeline/mod.rs:40
```

Mechanism: `PipelineJob` is only publicly reachable through a
`test-utils`-gated re-export; with `test-utils` on, `tests/deletion.rs`'s
construction silences dead_code even though **no production code constructs
the variant**.

Archaeology: commit `2ed0acf` (DB-backed `enqueue_vault_event` swap) removed
both producers — the watcher callback and the reconcile pass — leaving the
consumer orphaned. Deletions now flow through the DB queue, which deletes the
documents row directly (`db/queue.rs` module docs: "For Delete events: …
DELETE the documents row").

**Decision (user-approved): remove the variant.** Deletion is the DB queue's
job; the pipeline consumer is tested dead weight one refactor away from a
confusing CI break.

**Discovered gap (follow-up issue, not this PR):** the orphaned worker arm
(`pipeline/mod.rs:266`) contains the repo's **only** `.brain/converted/*.md`
shadow-copy cleanup. Every other reference to that directory creates it
(`lib.rs:610`, `vault/layout.rs:29`) or tests it. Shadow copies are therefore
never removed when a document is deleted — a latent bug that predates this
spec and must be fixed separately, in the DB-queue delete path.

## §3 — Design

### 3.1 Mechanical fixes

- `wiki_graph.rs:1380/1466/1622`: rewrite the three `assert_eq!` calls
  per §2.2. No other lines change.
- `tools/src/bin/tier_backfill.rs:65`: `println!("\n{:<40} TIER", "ENTRY ID")`.

### 3.2 Remove `PipelineJob::Delete`

Delete all of:

- the variant (`pipeline/mod.rs:40`) and its doc-era comments;
- the two consumer arms — the path-extraction arm at `pipeline/mod.rs:193`
  and the worker arm at `pipeline/mod.rs:266` (including the shadow-copy
  body — it is unreachable; the gap is tracked separately, §2.3);
- the exhaustive-match arms in the sweep tests
  (`pipeline/watchdog/sweep.rs:190`, `:214`, `:298`) — the matches remain
  exhaustive over `Ingest` alone;
- the whole of `src-tauri/tests/deletion.rs` (117 lines, one test, exists
  solely to exercise the orphaned consumer).

After removal, grep the crate for `PipelineJob::Delete` and `Stage::Deleting`
stragglers in comments. `Stage::Deleting` itself is **kept**: the watchdog
heartbeat parses it back from its discriminant (`heartbeat.rs:45,
8 => Stage::Deleting`), so it stays constructed and warning-free.

### 3.3 CI step becomes blocking

In `.github/workflows/ci.yml`, the clippy step becomes:

```yaml
      - name: Clippy
        run: cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings
```

- `continue-on-error: true` is removed.
- `-D warnings` goes **after `--`** so it escalates every warning — clippy
  *and* rustc — to an error. It must not be set via `RUSTFLAGS`, which would
  also poison dependency rebuilds.
- The stale "Warn-only on purpose" comment block is replaced with the
  blocking policy and the fix-forward runbook: rolling stable means a future
  Rust release can add a lint and fail CI with no repo change; the response
  is to fix the finding, or add a targeted `#[allow(clippy::…)]` with a
  reason comment when the lint is wrong for the code. Version pinning stays
  rejected per the ingest-wave spec (§5: the three `toolchain: stable`
  comments are load-bearing).
- Keep the existing placement/comment constraints: after "Pre-create
  placeholder sidecar binaries", with the `--features test-utils` rationale.

### 3.4 Extending clippy to `tools/` — measured, folded in

`tools/` is a separate manifest the clippy step never linted. Its backlog
measured at exactly one style finding (§2.2 #4), so this PR also adds a
second CI step immediately after the src-tauri one:

```yaml
      - name: Clippy (tools)
        run: cargo clippy --manifest-path tools/Cargo.toml --all-targets -- -D warnings
```

No `continue-on-error`; same fix-forward runbook. Building `tools/` re-lints
the src-tauri lib as a dependency — which is exactly what surfaced §2.3 — so
3.2 must land in the same PR or this step fails.

## §4 — Out of scope

- `cargo fmt --check` gating (never asked for; separate decision).
- Windows-target lints (CI clippy runs on ubuntu only).
- Shadow-copy cleanup on document deletion — **file a follow-up issue**
  describing the §2.3 gap and link it from the PR body (same pattern as
  PR 4's AC6).

## §5 — Acceptance criteria

- AC1: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
  --features test-utils -- -D warnings` exits 0 locally.
- AC2: `cargo clippy --manifest-path tools/Cargo.toml --all-targets
  -- -D warnings` exits 0 locally.
- AC3: `grep -r PipelineJob::Delete src-tauri/ tools/` returns nothing.
- AC4: `src-tauri/tests/deletion.rs` is gone; the full test suite passes
  without it (`cargo test --manifest-path src-tauri/Cargo.toml --features
  test-utils,mcp-server`).
- AC5: ci.yml has no `continue-on-error` on either clippy step; both carry
  `-D warnings`; the warn-only comment block is gone.
- AC6: The three `toolchain: stable` lines in `ci.yml`/`build.yml` are
  byte-identical to before (`git diff` proves it).
- AC7: Follow-up issue for the shadow-copy gap is filed and linked in the
  PR body.
- AC8: All CI checks green on the PR tip SHA, verified via
  `gh pr view --json mergeable,mergeStateStatus` and check-runs on that SHA
  (never via the PR body).

## §6 — Files

`.github/workflows/ci.yml`,
`src-tauri/src/wiki_graph.rs`,
`src-tauri/src/pipeline/mod.rs`,
`src-tauri/src/pipeline/watchdog/sweep.rs`,
`src-tauri/tests/deletion.rs` (deleted),
`tools/src/bin/tier_backfill.rs`.
