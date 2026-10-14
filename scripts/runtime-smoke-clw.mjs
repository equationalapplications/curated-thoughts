#!/usr/bin/env node
/**
 * Unmocked runtime smoke for the core-llm-wiki engine (7.13.0 host contract).
 *
 * Every vitest suite mocks the engine package, so nothing else exercises real
 * runtime lines. This script drives the REAL installed engine in Node over
 * node:sqlite (bundled SQLite, FTS5-enabled) wrapped in the expo-sqlite
 * adapter shape CT's Tauri adapter implements:
 *
 *   setup() -> upsertGraph() -> syncSearchIndex() -> keyword search hits,
 *   no WikiSearchUnavailableError.
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
//    scheduler in src/lib/wiki.ts makes).
try {
  await wiki.syncSearchIndex();
} catch (e) {
  fail('syncSearchIndex threw', e);
}

// 4. Keyword availability assertion through the engine's test access surface
//    (searchService.isKeywordAvailable() — the exact predicate the FTS state
//    machine drives; read() additionally needs an LLM provider, out of scope
//    for this sentinel).
let available;
try {
  available = wiki.__testAccess.searchService.isKeywordAvailable();
} catch (e) {
  fail('isKeywordAvailable() probe threw', e);
}
if (available !== true) {
  fail('keyword search unavailable after syncSearchIndex() — FTS state machine did not converge');
}
console.log('SMOKE OK: keyword search available after real-engine setup + write + convergence');
process.exit(0);
