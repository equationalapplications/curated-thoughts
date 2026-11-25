## Review: PR #369 (sso 9.x readiness, PR-A)

I read the full non-fixture diff: the `connection.rs` migration body, the `entity_gate.rs` planner and fold, the TS and Rust test changes, and the dependency bump. I cross-checked these against the checked-out tree:
- the gate cascade in `commit.rs:356-428` and `EdgeVocabulary` at `commit.rs:245-297`;
- `wiki_get_ontology` in `wiki_graph.rs:305`;
- `plan_manifest_ensure` and `ensure_manifest_vocabulary` in `entity_gate.rs`;
- the `llm_wiki_edges`, `curated_entities` and `curated_proposal_items` DDL in `okf_ddl.rs`.

I did not run cargo or vitest, because the AGENTS.md disk rules make that a controller decision. I also couldn't inspect the installed 9.18.0 package, since `node_modules` is outside the allowed directories. I skimmed the 4,000-line fixture/JSON portion for integrity only.

### MAJOR

**1. `sso9_declared_edges` doesn't fully mirror the gate. Ungated entities lose edges irreversibly.**
`src-tauri/src/db/connection.rs`, `sso9_declared_edges` (diff lines ~558-583), consumed by `sso9_parent_shape_declared` (~589-617).

The rOZa fix claims to mirror `resolve_strict_edge_vocabulary_with`, but it misses the "no gate" outcomes. In `commit.rs:356-373`, a strict manifest with zero edge types returns `Strict(None)`. `resolve_strict_edge_vocabulary_with` then returns `None`, and the gate admits every edge. The doc comment at `commit.rs:316-322` calls dropping all edges on such a brain "a severe and very hard-to-diagnose failure". The migration reads these cases differently:
- **Strict manifest with `edge_types: []`:** `manifest_declared_edge_triples` returns `Some(empty set)`, so every `author`/`publisher`/`supersedes` edge touching a re-typed row counts as undeclared.
- **No strict manifest on either leg:** `set` stays `None`, so `sso9_parent_shape_declared` returns `Ok(false)`, and the same edges land in `parent_drop`.

Failure scenario: an entity with a custom or half-seeded strict manifest has `supersedes(handoff-row, X)`. The gate would admit that edge today. The migration puts it in `parent_drop` and deletes it at step 4. On success `apply_sso9_migrations` deletes the backup, so the loss can't be undone. The doc comment makes "ungated means most permissive" a deliberate repo contract. No test covers a no-gate or empty-edge-types entity. The `ent_foreign` fixture has no edges, and the inherited-partition test only covers the declared case.

Fix: when the cascade resolves to no gate, treat the shape as declared and keep the edge. Concretely, return `Some(all)` or a separate "ungated" signal, and handle an empty edge vocabulary the way `ontology_leg` does. Add RED tests for strict with empty `edge_types`, and for an entity with no manifest on either leg. If the intent really is "drop when ungated", spec change-6 doesn't say so (it says "UNDECLARED ... DROPPED"). Per AGENTS.md that needs a spec ruling, and the 8.x-era "most permissive" contract should be called out explicitly.

### MINOR

**2. Failed or aborted attempts leave backups that nothing cleans up.**
`connection.rs`, `apply_sso9_migrations`, `?` on `run_sso9_refresh_txn` and `run_sso9_retype_txn`, and `sso9_delete_backup`.

On any `Err` after the backup (late-write abort, CAS mismatch, ensure fold failure), the backup file is kept by design. The next open takes a new backup with a new timestamp. The success path only deletes its own `backup`, never earlier `*.bak-pre-sso9-*` files. If a failure repeats on each launch, each launch adds a full-DB copy, and the 64 MB free-space check only guards one copy at a time. Fix: on a verified success, also remove stale `<stem>.bak-pre-sso9-*` siblings, or cap them at N and log. Alternatively, document the accumulation as accepted.

**3. Whitespace handling in the declared-shape prediction differs from the SQL retype.**
`connection.rs`, `sso9_resolve_type` (~541) and the `post_retype` closure (~598-606).

`sso9_resolve_type` trims, so a stored `" handoff"` (leading space) maps to `decision` in the prediction. The SQL retype uses exact `WHERE okf_type = ?1`, so the row stays `" handoff"`. This is the same class of mismatch as rOZe, which fixed case but not whitespace. A kept edge could then land on an undeclared shape. It's exotic, but cheap to fix: apply `SSO9_RETYPE_MAP` to the untrimmed stored value and trim only for the lookup key.

**4. `.hermes/reviews/2026-11-24_pr369-t15-r1-haiku-high.json` isn't JSON.**
Lines 52-78 of the diff show it holds markdown plus a stats line. Rename it to `.md`, or confirm that's what the tooling expects.

### Checked and clean
- **Eligibility and idempotency:** a refreshed manifest lacks `handoff`/`session_recap`, so it fails the old-17 check and a re-run is a no-op.
- **Refresh result:** the node set adds 5 EA types plus `statement`, which is excluded from `EA_SEED_TYPES`, so the set is 20. The refreshed manifest satisfies the new subset guard.
- **Ensure fold:** `fold_sso9_ensure` records its memo inside the refresh transaction, which rolls back with the CAS. Its `ForeignNoPreferredFallback` branch would memoize where `ensure_manifest_vocabulary` doesn't. That branch is unreachable for refreshed rows, because they declare the full EA set.
- **`VACUUM INTO` target:** an exclusively created empty file is a valid target, and the failure cleanup is now unconditional on the owned path.
- **Unique-edge collision:** `UPDATE OR IGNORE` followed by a per-id `DELETE` handles a collision with the `UNIQUE(entity_id, source_id, target_id, edge_type)` constraint correctly.
- **DDL and columns:** the tables and columns used by the migration exist in `okf_ddl.rs`.
- **`redirect_scan` allowlist:** the entry is justified.
- **TS tests:** the vendored-snapshot drift check and `VENDORED_PATH` resolution are fine.
- **Unverified:** the 9.18.0 lock integrity hash and its dependency list. `core-llm-wiki` stays at 8.0.2 until PR-B, per the spec.

**REQUEST CHANGES**: one MAJOR (finding 1: the migration deletes edges on ungated entities that the gate would admit, and the success path deletes the backup). Everything else is nits.

--- STATS: models=claude-sonnet-5-5 turns=29 cost=$0.7316 cache: read=1443953 write=96251 ratio=15.00
