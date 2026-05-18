import { invoke } from "@tauri-apps/api/core";
import type { SQLiteAdapter } from "@equationalapplications/core-llm-wiki";

// Serializes all wiki adapter operations so that transactions are atomic.
// When withTransactionAsync holds the lock, no other queued operation can
// interleave between BEGIN and COMMIT, even across await boundaries.
let _opQueue: Promise<void> = Promise.resolve();

function enqueue<T>(op: () => Promise<T>): Promise<T> {
  let unlock!: () => void;
  const prev = _opQueue;
  _opQueue = new Promise<void>(r => { unlock = r; });
  return prev.then(() => op()).finally(unlock) as Promise<T>;
}

// Used as the `tx` handle inside withTransactionAsync. Bypasses the queue
// because the enclosing transaction already holds the queue lock — going
// through enqueue() again would deadlock.
const directAdapter: SQLiteAdapter = {
  async execAsync(sql: string) {
    await invoke("wiki_exec", { sql });
  },
  async runAsync(sql: string, params: unknown[] = []) {
    const r = await invoke<{ changes: number; last_insert_row_id: number }>(
      "wiki_run", { sql, params }
    );
    return { changes: r.changes, lastInsertRowId: r.last_insert_row_id };
  },
  async getAllAsync<T>(sql: string, params: unknown[] = []) {
    return invoke<T[]>("wiki_get_all", { sql, params });
  },
  async getFirstAsync<T>(sql: string, params: unknown[] = []) {
    return invoke<T | null>("wiki_get_first", { sql, params });
  },
  async withTransactionAsync<T>(fn: (tx: SQLiteAdapter) => Promise<T>): Promise<T> {
    await invoke("wiki_exec", { sql: "BEGIN" });
    try {
      const result = await fn(directAdapter);
      await invoke("wiki_exec", { sql: "COMMIT" });
      return result;
    } catch (e) {
      await invoke("wiki_exec", { sql: "ROLLBACK" }).catch(() => {});
      throw e;
    }
  },
  async closeAsync() {},
};

export const tauriWikiAdapter: SQLiteAdapter = {
  async execAsync(sql: string) {
    return enqueue(() => invoke<void>("wiki_exec", { sql }));
  },

  async runAsync(sql: string, params: unknown[] = []) {
    return enqueue(async () => {
      const r = await invoke<{ changes: number; last_insert_row_id: number }>(
        "wiki_run", { sql, params }
      );
      return { changes: r.changes, lastInsertRowId: r.last_insert_row_id };
    });
  },

  async getAllAsync<T>(sql: string, params: unknown[] = []) {
    return enqueue(() => invoke<T[]>("wiki_get_all", { sql, params }));
  },

  async getFirstAsync<T>(sql: string, params: unknown[] = []) {
    return enqueue(() => invoke<T | null>("wiki_get_first", { sql, params }));
  },

  async withTransactionAsync<T>(fn: (tx: SQLiteAdapter) => Promise<T>): Promise<T> {
    return enqueue(async () => {
      await invoke("wiki_exec", { sql: "BEGIN" });
      try {
        const result = await fn(directAdapter);
        await invoke("wiki_exec", { sql: "COMMIT" });
        return result;
      } catch (e) {
        await invoke("wiki_exec", { sql: "ROLLBACK" }).catch(() => {});
        throw e;
      }
    });
  },

  async closeAsync() {
    // Connection lifetime managed by Rust
  },
};
