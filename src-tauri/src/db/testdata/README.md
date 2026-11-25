# DB test fixtures (engine-9x PR-B lane + prior lanes)

Provenance and regeneration recipes. All binaries here are synthetic
(brains opened through CT's real open path on empty dirs — no real data).

## `ct-brain-v32-core-8.0.2.db`
Frozen pre-PR-B brain: CT migration chain stamped through V32, core-llm-wiki
**8.0.2** engine shape. Built 2026-11-25 (T0, commit `bb653071`) by opening a
fresh ROOTED brain via `AppDb::open_with_config` at base `78160f98`, then
`VACUUM INTO` this file. Regenerate: check out `78160f98`, run any rooted-open
path (e.g. the app or a test that calls `open_with_config`), `VACUUM INTO`.
Serves as the "existing DB" input for V33 engine-mirror tests
(`engine9_fixture_copy` in `connection.rs`), the guard fidelity test, and the
Rust-generated migrated-schema dump's source.

## `ct-engine9-migrated.llm_wiki.schema.sql`
Rust-generated `llm_wiki_*` schema dump of the fixture brain AFTER the V33
mirror + helpers. Regenerated and drift-tested at every gate run by
`engine9_migrated_schema_fixture_is_current` (`connection.rs`) — do not hand
edit. Consumed by `src/__tests__/engine9-setup-compat.test.ts`.

## `core-llm-wiki-8.0.2.expectations.json`
Column expectations as CT's `schema_guard::LLM_WIKI_TABLES` described them at
base `78160f98` (captured via T0 from that tree's guard — so it INCLUDES the
CT-local `tier`/`embed_scheme`/`area` columns, not just the engine's). It is
the "8.0.2 shape" input the 9.18.0 `verify_schema_with` parameterization
rejects.

## `schema-software-org-8.0.2.manifest.json`
Frozen sso 8.0.2 manifest captured during the PR-A lane (#369, T1 step 1);
diff baseline for the sso9 re-type tests.

## Line endings

`testdata/*.sql` is pinned to LF via `.gitattributes` (the Rust drift gate
byte-compares it). Windows clones created before that attribute landed need
one re-checkout: `git rm --cached -r src-tauri/src/db/testdata && git reset --hard`.
