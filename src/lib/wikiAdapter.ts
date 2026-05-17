import { invoke } from "@tauri-apps/api/core";
import type { SQLiteAdapter } from "@equationalapplications/core-llm-wiki";

export const tauriWikiAdapter: SQLiteAdapter = {
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
      const result = await fn(this);
      await invoke("wiki_exec", { sql: "COMMIT" });
      return result;
    } catch (e) {
      await invoke("wiki_exec", { sql: "ROLLBACK" }).catch(() => {});
      throw e;
    }
  },

  async closeAsync() {
    // Connection lifetime managed by Rust
  },
};
