// Opens a scratch brain.db with the installed core-llm-wiki engine, runs
// setup(), and prints the engine version plus every source_ref that changed.
// Used by the #186 acceptance gate: post-fix, nothing may change.
//
// The engine is "bring your own adapter": createWiki(db: SQLiteAdapter,
// options: WikiOptions). This probe builds the adapter over node:sqlite
// (DatabaseSync), mirroring the shape of src/lib/wikiAdapter.ts.
import { execSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';

const dbPath = process.argv[2];
if (!dbPath) throw new Error('usage: engine-setup-probe.mjs <brain.db>');

// pnpm ls is the reliable version read: the package's exports map does not
// expose ./package.json, so `require(...package.json)` throws. pnpm is
// optional, though — without it the probe degrades to version 'unknown' and
// setup() still runs.
let version = 'unknown';
try {
  const listed = JSON.parse(
    execSync('pnpm ls --json @equationalapplications/core-llm-wiki', { encoding: 'utf8' }),
  );
  version = listed[0]?.dependencies?.['@equationalapplications/core-llm-wiki']?.version ?? version;
} catch (err) {
  console.error(`[probe] pnpm version read failed (${err.message?.split('\n')[0]}); reporting 'unknown'`);
}

const { createWiki } = await import('@equationalapplications/core-llm-wiki');

const db = new DatabaseSync(dbPath);

// Minimal SQLiteAdapter over DatabaseSync. The transaction handle bypasses no
// queue here (single-threaded probe), so it reuses the same adapter.
const adapter = {
  async execAsync(sql) {
    db.exec(sql);
  },
  async runAsync(sql, params = []) {
    const r = db.prepare(sql).run(...params);
    return { changes: Number(r.changes), lastInsertRowId: Number(r.lastInsertRowid) };
  },
  async getAllAsync(sql, params = []) {
    return db.prepare(sql).all(...params);
  },
  async getFirstAsync(sql, params = []) {
    return db.prepare(sql).get(...params) ?? null;
  },
  async withTransactionAsync(fn) {
    db.exec('BEGIN');
    try {
      const result = await fn(adapter);
      db.exec('COMMIT');
      return result;
    } catch (e) {
      db.exec('ROLLBACK');
      throw e;
    }
  },
  async closeAsync() {
    db.close();
  },
};

const snapshot = () =>
  Object.fromEntries(
    db.prepare('SELECT rowid, id, source_ref FROM llm_wiki_entries').all().map((r) => [r.rowid, r]),
  );
const before = snapshot();

// setup() only creates/migrates schema and normalizes existing source_refs —
// it must never call the LLM or the embedder, so both stubs throw loudly.
const wiki = createWiki(adapter, {
  llmProvider: {
    async generateText() {
      throw new Error('probe: llmProvider.generateText must not run during setup()');
    },
    async embed() {
      throw new Error('probe: llmProvider.embed must not run during setup()');
    },
  },
});
await wiki.setup();
const after = snapshot();

const changed = Object.keys(after)
  .filter((rowid) => before[rowid]?.source_ref !== after[rowid].source_ref)
  .map((rowid) => ({ id: after[rowid].id, before: before[rowid].source_ref, after: after[rowid].source_ref }));

console.log(JSON.stringify({ engineVersion: version, changedRows: changed, changedCount: changed.length }));
