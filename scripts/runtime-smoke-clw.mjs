#!/usr/bin/env node
/**
 * Unmocked runtime smoke for the core-llm-wiki engine (7.13.0 host contract).
 *
 * Every vitest suite mocks the engine package, so nothing else exercises real
 * runtime lines. This script drives the REAL installed engine in Node over
 * node:sqlite (bundled SQLite, FTS5-enabled) wrapped in the expo-sqlite
 * adapter shape CT's Tauri adapter implements:
 *
 * setup() -> upsertGraph() -> syncSearchIndex() -> searchKeyword probe via
 * __testAccess must return the written node (the discriminating convergence
 * assertion; isKeywordAvailable() alone is vacuous on a fresh tiny store).
 *
 * Exit 0 = sentinel green. Any failure exits non-zero with the error.
 *
 * Node >= 22.5: run plain (node:sqlite stable behind no flag on 23+; on the
 * 22.x CI line pass --experimental-sqlite).
 */
import { DatabaseSync } from 'node:sqlite';
import { createWiki } from '@equationalapplications/react-llm-wiki';

const ENTITY = 'smoke::entity';

// --- expo-sqlite adapter shape over node:sqlite ----------------------------
// Methods the engine calls: execAsync, runAsync, getFirstAsync, getAllAsync,
// withTransactionAsync (verified against dist: this.db.* usage).
function makeAdapter() {
  const db = new DatabaseSync(':memory:');
  db.exec('PRAGMA journal_mode = WAL;');
  const wrap = (handle) => ({
    execAsync: (sql) => handle.exec(sql),
    runAsync: (sql, params = []) => handle.prepare(sql).run(...params),
    getFirstAsync: async (sql, params = []) =>
      handle.prepare(sql).get(...params) ?? null,
    getAllAsync: async (sql, params = []) =>
      handle.prepare(sql).all(...params),
    withTransactionAsync: async (fn) => {
      handle.exec('BEGIN');
      try {
        const t = {
          execAsync: (sql) => handle.exec(sql),
          runAsync: (sql, params = []) => handle.prepare(sql).run(...params),
          getFirstAsync: async (q, p = []) => handle.prepare(q).get(...p) ?? null,
          getAllAsync: async (q, p = []) => handle.prepare(q).all(...p),
          meter: () => {},
          entityId: ENTITY,
          trigger: 'call',
        };
        await fn(t);
        handle.exec('COMMIT');
      } catch (e) {
        handle.exec('ROLLBACK');
        throw e;
      }
    },
  });
  return wrap(db);
}

function fail(msg, err) {
  console.error(`SMOKE FAIL: ${msg}`, err ?? '');
  process.exit(1);
}

const adapter = makeAdapter();
const wiki = createWiki(adapter, {
  config: {
    // Force the FTS5 strategy so the state machine actually runs; tiny budget
    // so convergence is driven by syncSearchIndex, mirroring the host story.
    indexStrategy: 'fts5',
    setupBudgetMs: 10,
  },
});

// 1. setup() must not throw (migration gate + FTS init state machine).
try {
  await wiki.setup();
} catch (e) {
  fail('setup() threw', e);
}

// 2. Write a graph through the real write path.
try {
  await wiki.upsertGraph(ENTITY, {
    sourceRef: 'smoke://runtime/1',
    sourceHash: 'a'.repeat(64),
    nodes: [
      { id: 'smoke-node-1', type: 'SoftwareApplication', title: 'Curated Thoughts smoke node', body: 'FTS convergence sentinel content quintessence' },
    ],
    edges: [],
  }, adapter);
} catch (e) {
  fail('upsertGraph threw', e);
}

// 3. Host-side convergence: no-arg syncSearchIndex() (the exact call the
//    scheduler in src/lib/wiki.ts makes). Before it, the upsertGraph node is
//    stashed, not yet keyword-indexed; after it, the ledger drain must have
//    indexed it. searchKeyword is the discriminating assertion (r2 finding 2:
//    isKeywordAvailable() alone is vacuous — it never degrades on a fresh
//    tiny store).
try {
  await wiki.syncSearchIndex();
} catch (e) {
  fail('syncSearchIndex threw', e);
}

// 4. The convergence sentinel: the written node must be keyword-findable.
let hits;
try {
  hits = await wiki.__testAccess.searchService.searchKeyword('quintessence', [ENTITY], 5);
} catch (e) {
  fail('searchKeyword probe threw', e);
}
if (!Array.isArray(hits) || hits.length === 0) {
  fail(`keyword index did not converge: searchKeyword returned ${JSON.stringify(hits)}`);
}
if (!hits.some((h) => h.id === 'smoke-node-1')) {
  fail(`keyword hits lack the written node: ${JSON.stringify(hits)}`);
}
console.log(`SMOKE OK: keyword index converged; ${hits.length} hit(s), incl. smoke-node-1`);
process.exit(0);
