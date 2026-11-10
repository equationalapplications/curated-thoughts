# Plan — `ct evidence re-anchor`

Spec: `spec.md` (Reviewed, Kurt adjudications 2026-11-10). Implementation WIP
recovered onto this branch already covers §2/§3/§5 and most of §4; this plan
closes the one remaining spec gap and pins the gate order.

## T1 — Backup before first write (spec §4, adjudicated: KEPT)

- [ ] `pub fn backup_before_apply(conn: &Connection, db_path: &Path) -> Result<PathBuf>`
      in `src-tauri/src/db/evidence_reanchor.rs`: `VACUUM INTO`
      `<db_dir>/<db_file>.bak-pre-reanchor-<unix_ts>`; returns the path.
      Runs on the apply path only, BEFORE the first UPDATE; a failure here
      aborts the apply with zero rows touched.
- [ ] `tools/src/cmds.rs` `evidence_reanchor_cmd(yes=true)`: call the helper
      after `open_rw`, print `backup=<path>` before applying.
- [ ] Dry-run path (`yes=false`) must write no backup file (it already never
      touches a write conn; assert in T2 test).

## T2 — Tests (spec §6)

- [ ] RED-first: backup test in `evidence_reanchor.rs` tests module —
      file-backed tempdir db, backup file exists, snapshot readable and
      carries the pre-state; in-memory `re_anchor(apply=true)` still passes.
- [ ] Existing WIP suite (10 tests: unique-match re-anchor + flag clear,
      idempotency, ambiguous multi-match, unresolved stays flagged,
      hash-mismatch drift skip, soft-deleted untouched, parse gates) stays green.

## T3 — Gates and review

- [ ] `cargo test --manifest-path tools/Cargo.toml` (no `test-utils` feature —
      that feature belongs to the lib package) + `cargo test --manifest-path
      src-tauri/Cargo.toml --lib --features test-utils`, CARGO_TARGET_DIR
      pinned to the shared dir per AGENTS.md, serial.
- [ ] Tier-1.5 ladder (sor → haiku low/medium → escalate per findings) on the
      full branch diff; fixes between rungs, one commit each.
- [ ] Flip PR #336 out of draft after gates; cleanup commit deletes plan.md +
      LEDGER.md in the SAME PR.

## Ops tail (post-merge, NOT this PR)

Dry-run `ct evidence re-anchor` against `~/.brain/brain.db` → Kurt gates the
`--yes` apply → re-measure the doomed set → residual ~20% true orphans
adjudicated by Kurt → only then the heal breaker reset decision.
