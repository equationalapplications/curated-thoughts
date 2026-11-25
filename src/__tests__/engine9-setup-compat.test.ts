/**
 * Engine-side compatibility proof (engine-9x PR-B, spec §5 test 2 TS half;
 * spec §5 TS-side proof): the REAL core-llm-wiki 9.18.0 `setup()` — `createWiki(db).setup()`,
 * the package's exported entry that runs `setupDatabase` + the v1–v19
 * migrations — completes over a DB built from exactly what CT's Rust chain
 * produces for an existing 8.0.2-era brain.
 *
 * The schema comes from `src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql`,
 * generated and drift-tested by the Rust test
 * `engine9_migrated_schema_fixture_is_current` (never from node_modules).
 * LIMIT: the dump carries NO FTS objects and no v14 legacy index, so the
 * "stamped 15" case does not exercise 9.18.0's FTS upgrade path on a brain
 * that already carries 8.0.2 FTS (the common desktop upgrade). The
 * schema-stability test below pins the no-FTS shape's footprint.
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
// the bundler never sees. Optional + skipped on Node < 22.5 (or 22.5–22.12
// without --experimental-sqlite) where the module is absent; CI
// pins Node 22 latest, which has it unflagged.
const sqliteModule = process.getBuiltinModule?.("node:sqlite");
const DatabaseSync = sqliteModule?.DatabaseSync;
type DatabaseSync = InstanceType<NonNullable<typeof DatabaseSync>>;
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

function schemaObjects(db: DatabaseSync): string[] {
  return (
    db
      .prepare(
        "SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
      )
      .all() as { type: string; name: string }[]
  ).map((r) => `${r.type}:${r.name}`);
}

// Skipped (not failed) on Node runtimes without node:sqlite — see the
// module doc above — but a HARD failure under CI: the CI Node must have the
// builtin, and a silent skip there would vacate the proof.
describe.skipIf(!DatabaseSync && !process.env.CI)(
    "core-llm-wiki 9.18.0 setup() over the Rust-chain (V33-migrated) schema",
    () => {
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

  it("setup() leaves the schema unchanged modulo the FTS family allow-list", async () => {
    // Pins that the REAL setup() adds ONLY the lazy FTS infrastructure
    // (virtual tables + shadow tables + sync indexes + sync triggers —
    // verified against the installed 9.18.0 dist) and nothing else, so a
    // future engine bump that changes schema anywhere else (including the
    // helper mirrors in okf_ddl.rs, which sit outside ddl_compat) fails
    // HERE instead of drifting silently.
    const db = rustChainDb();
    db.prepare("INSERT INTO llm_wiki_meta (key, value) VALUES ('schema_version', ?)").run(
      ENGINE_8_0_2_SCHEMA_VERSION,
    );
    const before = schemaObjects(db);
    await engine(db).setup();
    const after = schemaObjects(db);
    const beforeSet = new Set(before);
    const added = after.filter((n) => !beforeSet.has(n));
    const dropped = before.filter((n) => !after.includes(n));
    const FTS_FAMILY = [
      "index:llm_wiki_fts_map_entity",
      "index:llm_wiki_fts_pending_entity",
      "index:llm_wiki_fts_pending_id",
      "index:llm_wiki_source_passages_fts_map_doc",
      "index:llm_wiki_source_passages_fts_pending_doc",
      "table:llm_wiki_entries_fts",
      "table:llm_wiki_entries_fts_config",
      "table:llm_wiki_entries_fts_content",
      "table:llm_wiki_entries_fts_data",
      "table:llm_wiki_entries_fts_docsize",
      "table:llm_wiki_entries_fts_idx",
      "table:llm_wiki_fts_map",
      "table:llm_wiki_fts_pending",
      "table:llm_wiki_source_passages_fts",
      "table:llm_wiki_source_passages_fts_config",
      "table:llm_wiki_source_passages_fts_content",
      "table:llm_wiki_source_passages_fts_data",
      "table:llm_wiki_source_passages_fts_docsize",
      "table:llm_wiki_source_passages_fts_idx",
      "table:llm_wiki_source_passages_fts_map",
      "table:llm_wiki_source_passages_fts_pending",
      "trigger:llm_wiki_entries_fts_ad",
      "trigger:llm_wiki_entries_fts_ai",
      "trigger:llm_wiki_entries_fts_au",
      "trigger:llm_wiki_source_passages_fts_ad",
      "trigger:llm_wiki_source_passages_fts_ai",
    ];
    expect(dropped).toEqual([]);
    expect(added.sort()).toEqual(FTS_FAMILY);
  });

  it("helper-SQL drift gate: normalized index/trigger bodies match a fresh engine brain (engine-ownership filter)", async () => {
    // Rust chains never create FTS; at-rest FTS is engine-owned on real desktop brains.
    // The four source_passages entries: source-passages FTS5 subsystem (dist 2432-2451) —
    // rust chains never create any FTS.
    const FTS_FAMILY = new Set([
      "trigger:llm_wiki_source_passages_fts_ai",
      "trigger:llm_wiki_source_passages_fts_ad",
      "index:llm_wiki_source_passages_fts_map_doc",
      "index:llm_wiki_source_passages_fts_pending_doc",
      "trigger:llm_wiki_entries_fts_ai",
      "trigger:llm_wiki_entries_fts_au",
      "trigger:llm_wiki_entries_fts_ad",
      "index:llm_wiki_fts_map_entity",
      "index:llm_wiki_fts_pending_entity",
      "index:llm_wiki_fts_pending_id",
    ]);
    // Present in fresh 8.0.2 + 9.18.0 engines but absent from the dump (source brain built by
    // CT's Rust open path, which never runs engine setup); replay reconciliation is engine-owned.
    const RUST_CHAIN_LIMITS = new Set(["index:llm_wiki_entries_legacy_source_type_idx"]);
    // Rust-side llm_wiki_* objects the engine brain does not carry; an unlisted rust-only object fails.
    const CT_OWNED = new Set([
      "index:llm_wiki_vector_removals_pending",
      "trigger:llm_wiki_vector_removals_enqueue",
      "trigger:llm_wiki_vector_removals_hard_delete",
      "trigger:llm_wiki_vector_removals_resurrect",
      "index:llm_wiki_idx_source_passages_doc",
      "index:llm_wiki_idx_source_passages_entity",
      "index:llm_wiki_entries_deleted_idx",
      "index:llm_wiki_events_forgotten_idx",
    ]);
    const normalize = (s: string) => s.replace(/\s+/g, " ").trim();
    const bodies = (db: DatabaseSync) => {
      const rows = db
        .prepare(
          "SELECT type, name, sql FROM sqlite_master WHERE type IN ('index','trigger') AND sql IS NOT NULL",
        )
        .all() as { type: string; name: string; sql: string }[];
      return new Map(rows.map((r) => [`${r.type}:${r.name}`, normalize(r.sql)]));
    };
    const rust = bodies(rustChainDb());
    const fresh = new DatabaseSync(":memory:");
    await engine(fresh).setup();
    const eng = bodies(fresh);

    const problems: string[] = [];
    for (const [key, body] of eng) {
      if (!key.split(":")[1].startsWith("llm_wiki_")) continue;
      if (FTS_FAMILY.has(key) || RUST_CHAIN_LIMITS.has(key)) continue;
      const r = rust.get(key);
      if (r === undefined) problems.push(`engine-only, missing in rust chain: ${key}`);
      else if (r !== body) problems.push(`body drift: ${key}\n  rust:   ${r}\n  engine: ${body}`);
    }
    for (const key of rust.keys()) {
      if (!key.split(":")[1].startsWith("llm_wiki_")) continue;
      if (!eng.has(key) && !CT_OWNED.has(key)) problems.push(`rust-only, not CT_OWNED: ${key}`);
    }
    expect(problems).toEqual([]);
  });
});
