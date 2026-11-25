## Review: PR #369 (sso9 / engine 9.x PR-A)

I read the full diff (3,886 lines), including the `connection.rs` migration body and its tests, the `entity_gate.rs` planner and fold, the new test files, and the TS/Rust test-fixture changes. I checked the cross-file assumptions: `record_ensure_memo` opens no nested transaction, `migrate` is not called inside an outer transaction (`open_with_config`, connection.rs:2646), the `wiki_graph` field types match their uses, `tauri_app_lib` is the lib name and `db`/`entity_gate` are public, and the ensure path repairs a dangling `fallback_node_type` inside the same refresh transaction (entity_gate.rs:899-909). I did not run cargo or vitest, so the test results below are unverified.

### BLOCKER
None found.

### MAJOR
None found.

### MINOR

1. **connection.rs:894-896 (`sso9_shape_check`)**: the R1 check only inspects `refreshed.first()`. A tamper or drift in any other refreshed row goes undetected by this check. Failure scenario: a concurrent writer changes the second refreshed manifest between COMMIT and the check, so the CRITICAL path never fires for that row. The per-row CAS UPDATE at connection.rs:325-334 covers the write itself, so the gap is only post-commit drift. Fix: iterate all `refreshed` rows and collect failures, which is cheap (one SELECT per row).

2. **connection.rs:1056 (backup on every first open)**: `sso9_backup_before_migration` runs whenever either slot is unstamped, including a fresh install with no manifests or edges to migrate. On a near-full disk, the free-space check at connection.rs:206-211 (db+wal+64 MB) then aborts the open with "insufficient free disk space" even though there is nothing to migrate. This matches spec S2/S4, so it is not a defect against the spec. Consider skipping the backup when the census finds no rows to change. Treat this as a design note, not a bug.

3. **src/__tests__/ontology-packages.test.ts:3751 and 3818**: `VENDORED_PATH` is relative and resolved against the process cwd. The test fails if vitest is launched from a subdirectory or an IDE runner with a different cwd. Fix: resolve from the test file's location (`fileURLToPath(new URL('../../src-tauri/...', import.meta.url))`).

4. **specs/engine-9x-adoption/spec.md:57**: the status line marks PR-A "Implemented 2026-11-24 on chore/sso-9x-readiness" while this PR is still open, and it names a branch that may not be this PR's head. AGENTS.md says status moves to Implemented as part of the implementing PR, and that the PR stays draft until then. Confirm the branch name and whether the PR is still draft.

### Unverified (not flagged)
- `tools/tests/edge_writer_gate.rs` fixture count for `src/db/connection.rs` (now 7) and the `ct_heal.rs` expectations after the seed switch to `EA_SEED_TYPES`. The diff does not show their outputs, so I could not confirm they pass.
- The `@equationalapplications/schema-software-org@9.18.0` lock integrity hash and its dependency list, which I could not check against the registry.

APPROVE WITH NITS

--- STATS: models=claude-haiku-5-5 turns=23 cost=$0.1393 cache: read=1191603 write=121535 ratio=9.80
