## PR #369 review (sso9 / engine 9.x PR-A), depth: max

I read the sso9 migration in `connection.rs` at PR head (the worktree copy, lines 1419–2515), the planner and fold in `entity_gate.rs`, the new test files, and the small hunks. The `connection.rs` diff is mostly relocation noise: `AppDb`, `open_in_memory`, and `migrate_brain_db` are moved, not dropped. I checked that in the worktree. I did not run cargo or vitest, per the AGENTS.md disk rules.

Most of the logic holds up. The snapshot, plan, and backup cross-check run under `BEGIN IMMEDIATE`. The CAS UPDATE, the remap/dedupe ordering, the backup reservation, and fail-closed on backup or verify failures are all correct. The one real problem is below.

### BLOCKER

**1. The `about` arm drops person/place/event edges without the ungated/declared check. (`src-tauri/src/db/connection.rs:1937-1938`)**

The round-2 fix made `author`/`publisher`/`supersedes` consult `sso9_parent_shape_declared`, which keeps the edge when the entity is ungated or the shape is declared (lines 1843–1854). The `about` arm does not use it:

```rust
Some("person") | Some("place") | Some("event") => plan.parent_drop.push(e),
```

- **Failure scenario:** a brain where the entity has no applicable strict manifest (ungated), or a custom strict manifest that declares `about(statement→person)`. A `statement`-typed handoff row has an `about` edge to a person. The live gate admits that edge today, but step 4 deletes it. The success path then removes the backup, so the loss can't be undone. This is the same class the team already marked REQUEST CHANGES for.
- **Test gap:** `sso9_retype_ungated_entity_keeps_parent_family_edges` (line ~7009) deletes the tier_fact manifest, and its fixture contains `e_about_person` (line ~6242). It only asserts `author`/`publisher`, so the about→person drop goes unchecked.
- **Fix:** route the arm through the same check:
  ```rust
  Some("person") | Some("place") | Some("event") => {
      if sso9_parent_shape_declared(conn, &mut declared_cache, &mut cache, &e)? {
          plan.kept_declared_parent.push(e);
      } else {
          plan.parent_drop.push(e);
      }
  }
  ```
  This keeps the spec's drop for the ratified default brain, where tier_fact does not declare `about(statement→person)`. Only ungated and custom-manifest entities change behavior. Add `e_about_person` to the ungated assertions.

### MAJOR

None.

### MINOR

**2. The post-VACUUM sanity count can false-abort under concurrent writes. (`src-tauri/src/db/connection.rs:1561-1563`)**

The source counts run on `conn` after `VACUUM INTO`, in autocommit mode with no shared snapshot. A writer that commits in between (the MCP sidecar, or a second app instance) makes `src != dst`, so the open aborts. It fails closed with no data loss, and it retries on the next open. Suggested fix: on mismatch, retry the backup once, or downgrade the check to a WARN while keeping the backup. Either way the per-row content check still protects the destructive path.

**3. One malformed manifest can block the whole open. (`src-tauri/src/db/connection.rs:1820-1823`, propagated by `?` at ~2481)**

`sso9_declared_edges` bails on any `wiki_get_ontology` error. The refresh step leaves malformed manifests untouched, so this state is reachable on upgraded brains. It only fires when an `author`/`publisher`/`supersedes`/`about` edge touches a re-typed row, and then every open fails until someone repairs the row. Fail-closed is a defensible choice, and the error message does name the repair. Please confirm the owner accepts a single bad manifest blocking app start.

**4. Review artifacts are committed to the product repo. (`.hermes/reviews/*`, 3 files)**

The diff adds internal review notes and verdicts under `.hermes/`. Consider keeping them out of this PR, or confirm that the team tracks them on purpose.

### Open questions (not flagged)

- **NULL-typed endpoints drop parent edges.** `sso9_parent_shape_declared` returns `false` for an unresolved endpoint (`connection.rs:1872`, the `_ =>` arm), which sends the edge to `parent_drop`. The live DB has about 1,436 NULL `okf_type` rows (per the spec's own errata). I couldn't confirm from this diff how the live gate treats a NULL endpoint type, so I'm not calling this a bug. Please confirm the gate semantics, and that the drop is intended.
- **Lockfile integrity.** The `schema-software-org@9.18.0` integrity hash and its dependency list are unverified. I can't check the registry from here.

### Checked and clean

- The refresh and re-type steps are ordered correctly, the stamps are exact-membership, and the backup is deleted only after verification passes.
- The `EA_SEED_TYPES` change and the test updates are consistent with the spec's 20-slug rule.
- The TS vendored-drift test and the `ct_heal.rs` seed switch look correct.

**Verdict: REQUEST CHANGES** (BLOCKER 1: the `about` arm needs the same ungated/declared check as the parent-family arms, plus a test covering `e_about_person`).

--- STATS: models=claude-haiku-5-5 turns=24 cost=$0.1328 cache: read=1475679 write=115077 ratio=12.82
