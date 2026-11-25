/**
 * Engine-side compatibility proof (engine-9x PR-B, spec §5 test 2 TS half;
 * ruling Q9): the REAL core-llm-wiki 9.18.0 `setup()` — `createWiki(db).setup()`,
 * the package's exported entry that runs `setupDatabase` + the v1–v19
 * migrations — completes over a DB built from exactly what CT's Rust chain
 * produces for an existing 8.0.2-era brain.
 *
 * The schema comes from `src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql`,
 * generated and drift-tested by the Rust test
 * `engine9_migrated_schema_fixture_is_current` (never from node_modules).
 * The DB runs on built-in `node:sqlite` (`DatabaseSync`) behind a minimal
 * `SQLiteAdapter`; no new dependency. Runs under the suite's default jsdom
 * environment: the global `src/test-setup.ts` touches `window`, so a
 * file pinned to vitest's node environment cannot load it; vitest still runs in Node,
 * so `node:sqlite` resolves either way.
 */
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
// Loaded via `process.getBuiltinModule`, NOT a static import: vite 8.3.5's
// builtin-loader plugin does not recognize `node:sqlite` for client-environment
// (jsdom) externalization and fails the transform with "Cannot bundle Node.js
// built-in node:sqlite" on a cold cache. `getBuiltinModule` is a runtime call
// the bundler never sees (Node >= 22.3).
const sqliteModule = process.getBuiltinModule("node:sqlite");
const DatabaseSync = sqliteModule.DatabaseSync;
type DatabaseSync = InstanceType<typeof DatabaseSync>;
import { createWiki } from "@equationalapplications/core-llm-wiki";
import type { SQLiteAdapter } from "@equationalapplications/core-llm-wiki";

const SCHEMA_DUMP = resolve(
  __dirname,
  "../../src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql",
);
/** core-llm-wiki 8.0.2's last engine migration (8.0.2 dist `MIGRATIONS`). */
const ENGINE_8_0_2_SCHEMA_VERSION = "15";
/** core-llm-wiki 9.18.0's last engine migration (v19, forget-event exclusion). */
const ENGINE_9_18_0_SCHEMA_VERSION = "19";

type Params = Parameters<ReturnType<DatabaseSync["prepare"]>["run"]>;

function adapterOver(db: DatabaseSync): SQLiteAdapter {
  const handle: SQLiteAdapter = {
    async execAsync(sql: string) {
      db.exec(sql);
    },
    async runAsync(sql: string, params: unknown[] = []) {
      const r = db.prepare(sql).run(...(params as Params));
      return { changes: Number(r.changes), lastInsertRowId: Number(r.lastInsertRowid) };
    },
    async getAllAsync<T>(sql: string, params: unknown[] = []) {
      return db.prepare(sql).all(...(params as Params)) as T[];
    },
    async getFirstAsync<T>(sql: string, params: unknown[] = []) {
      return (db.prepare(sql).get(...(params as Params)) ?? null) as T | null;
    },
    async withTransactionAsync<T>(fn: (tx: SQLiteAdapter) => Promise<T>): Promise<T> {
      db.exec("BEGIN");
      try {
        const result = await fn(handle);
        db.exec("COMMIT");
        return result;
      } catch (e) {
        db.exec("ROLLBACK");
        throw e;
      }
    },
    async closeAsync() {},
  };
  return handle;
}

/**
 * `createWiki` is the package's only exported path to `setupDatabase` + the
 * migrations, and its `WikiOptions` type requires an `llmProvider`.
 * `setup()` never calls it: this provider throws on ANY method call, so the
 * proof stays DB-only and would fail loudly if setup() started needing it.
 */
const inertLlmProvider = new Proxy(
  {},
  {
    get: (_t, prop) =>
      prop === "then"
        ? undefined
        : () => {
            throw new Error(`setup() must not call llmProvider.${String(prop)}`);
          },
  },
) as Parameters<typeof createWiki>[1]["llmProvider"];

function engine(db: DatabaseSync) {
  return createWiki(adapterOver(db), { llmProvider: inertLlmProvider });
}

/** Load the Rust-generated dump: tables first, then indexes, then triggers. */
function rustChainDb(): DatabaseSync {
  const dump = readFileSync(SCHEMA_DUMP, "utf8");
  const objects = dump
    .split(/^-- object: /m)
    .slice(1)
    .map((chunk) => {
      const nl = chunk.indexOf("\n");
      const [kind] = chunk.slice(0, nl).split(" ");
      return { kind, sql: chunk.slice(nl + 1) };
    });
  const order = ["table", "index", "trigger"];
  expect(new Set(objects.map((o) => o.kind))).toEqual(new Set(order));
  const db = new DatabaseSync(":memory:");
  for (const kind of order) {
    for (const o of objects.filter((x) => x.kind === kind)) db.exec(o.sql);
  }
  return db;
}

function engineVersion(db: DatabaseSync): string | undefined {
  const row = db
    .prepare("SELECT value FROM llm_wiki_meta WHERE key = 'schema_version'")
    .get() as { value: string } | undefined;
  return row?.value;
}

describe("core-llm-wiki 9.18.0 setup() over the Rust-chain (V33-migrated) schema", () => {
  it("completes from the 8.0.2-era engine version (v16–v19 run their guards)", async () => {
    const db = rustChainDb();
    db.prepare("INSERT INTO llm_wiki_meta (key, value) VALUES ('schema_version', ?)").run(
      ENGINE_8_0_2_SCHEMA_VERSION,
    );
    await expect(engine(db).setup()).resolves.toBeUndefined();
    expect(engineVersion(db)).toBe(ENGINE_9_18_0_SCHEMA_VERSION);
  });

  it("completes with no engine version stamped (CLI-only brain: replays every migration)", async () => {
    const db = rustChainDb();
    expect(engineVersion(db)).toBeUndefined();
    await expect(engine(db).setup()).resolves.toBeUndefined();
    expect(engineVersion(db)).toBe(ENGINE_9_18_0_SCHEMA_VERSION);
  });

  it("negative control: a stray source_documents column makes v17 throw", async () => {
    const db = rustChainDb();
    db.prepare("INSERT INTO llm_wiki_meta (key, value) VALUES ('schema_version', ?)").run(
      ENGINE_8_0_2_SCHEMA_VERSION,
    );
    db.exec("ALTER TABLE llm_wiki_source_documents ADD COLUMN ct_stray TEXT");
    await expect(engine(db).setup()).rejects.toThrow(
      /Migration v17 \(add_source_store\) failed/,
    );
  });
});
