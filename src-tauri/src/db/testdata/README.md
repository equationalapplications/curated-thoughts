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

## `ct-brain-v33-src-8.0.2-fts.db`
Engine-generated **8.0.2** brain carrying FTS-at-rest text — the real desktop
upgrade input (#376). Generated 2026-11-25 (Task 10) by
`@equationalapplications/core-llm-wiki@8.0.2` (generator) via
`createWiki(adapter).setup()`, 8 entities x 3 entries (24 rows), then
`await wiki.syncSearchIndex()`, then `VACUUM INTO`. Consumers: 9.18.0 `setup()`
(`engine9-setup-compat.test.ts`) and the V33 chain
(`engine9_mirror_over_fts_carrying_8_0_2_brain_preserves_data_and_fts`).
Facts: engine `schema_version` = `15`; 15 `%fts%` objects;
`llm_wiki_entries_fts_content` holds 24 rows. It is ENGINE-ONLY: no CT-owned
tables (the Rust test creates `schema_version` itself).

Regenerate: `pnpm add` / install `@equationalapplications/core-llm-wiki@8.0.2`
in an empty neutral dir, save the harness below as `gen.cjs`, run
`node --experimental-sqlite gen.cjs <install-dir> <work>.db`, then
`sqlite3 <work>.db "VACUUM INTO '<repo>/src-tauri/src/db/testdata/ct-brain-v33-src-8.0.2-fts.db'"`.

```js
const { DatabaseSync } = require('node:sqlite');
const { createRequire } = require('node:module');
const [, , installDir, outPath] = process.argv;
const { createWiki } = createRequire(installDir + '/')('@equationalapplications/core-llm-wiki');

function adapterOver(db) {
  const params = (p) => (p ?? []);
  const handle = {
    async execAsync(sql) { db.exec(sql); },
    async runAsync(sql, p) {
      const r = db.prepare(sql).run(...params(p));
      return { changes: Number(r.changes), lastInsertRowId: Number(r.lastInsertRowid) };
    },
    async getAllAsync(sql, p) { return db.prepare(sql).all(...params(p)); },
    async getFirstAsync(sql, p) { return db.prepare(sql).get(...params(p)) ?? null; },
    async withTransactionAsync(fn) {
      db.exec('BEGIN');
      try { const out = await fn(handle); db.exec('COMMIT'); return out; }
      catch (e) { db.exec('ROLLBACK'); throw e; }
    },
    async closeAsync() {},
  };
  return handle;
}

const TOPICS = ['astronomy', 'baking', 'cartography', 'dendrology', 'ethology', 'fermentation', 'geology', 'horology'];
const db = new DatabaseSync(outPath);
const wiki = createWiki(adapterOver(db), { config: {} });
(async () => {
  await wiki.setup();
  const now = 1700000000000;
  const ins = db.prepare(`INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at, access_count, lifecycle_status)
    VALUES (?, ?, ?, ?, '[]', 'inferred', 'librarian_inferred', ?, ?, 0, 'stable')`);
  TOPICS.forEach((t, i) => {
    for (let j = 0; j < 3; j++) {
      ins.run(`ent_${t}_fact_${j}`, `ent_${t}`, `${t} note ${j}`,
        `The ${t} fixture entry ${j} carries distinctword${t}${j} through the FTS shadow across the desktop upgrade`, now + i, now + i);
    }
  });
  await wiki.syncSearchIndex();
  const q = (s) => db.prepare(s).get().n;
  console.log('fts objects:', q("SELECT COUNT(*) n FROM sqlite_master WHERE name LIKE '%fts%'"));
  console.log('entries:', q('SELECT COUNT(*) n FROM llm_wiki_entries'));
  console.log('entries_fts_content rows:', q('SELECT COUNT(*) n FROM llm_wiki_entries_fts_content'));
  console.log('schema_version:', db.prepare("SELECT value FROM llm_wiki_meta WHERE key='schema_version'").get().value);
  db.close();
})().catch((e) => { console.error('FAIL:', e.message); process.exit(1); });
```

Hygiene check (must print `0`; run before committing any regenerated fixture):
`strings src-tauri/src/db/testdata/ct-brain-v33-src-8.0.2-fts.db | grep -ciE 'kv-work|/home/|/mnt/|scratch'`

## Line endings

`testdata/*.sql` is pinned to LF via `.gitattributes` (the Rust drift gate
byte-compares it). Windows clones created before that attribute landed need
one re-checkout: `git rm --cached -r src-tauri/src/db/testdata && git checkout -- src-tauri/src/db/testdata`.
