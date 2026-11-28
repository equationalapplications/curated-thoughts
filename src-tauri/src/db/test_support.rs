//! Test-only DB shape helpers shared across db test modules.
//! Drop the three forgotten_at partial indexes, then the column — the
//! exact order SQLite requires before ALTER TABLE … DROP COLUMN (r1-B3;
//! the live-half index added in #401 references both `forgotten_at` and
//! `related_entry_id`, so it must drop first). `DROP INDEX IF EXISTS`
//! keeps the pre-#401 lines correct on shapes that lack the live index.
// No #[cfg(test)] here (r5 minor 5): the module declaration in mod.rs is
// already cfg(test)-gated.
pub(crate) fn drop_forgotten_column(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "DROP INDEX IF EXISTS llm_wiki_events_related_live_idx;
         DROP INDEX IF EXISTS llm_wiki_events_related_forgotten_idx;
         DROP INDEX IF EXISTS llm_wiki_events_forgotten_idx;
         ALTER TABLE llm_wiki_events DROP COLUMN forgotten_at;",
    )
    .unwrap();
}
