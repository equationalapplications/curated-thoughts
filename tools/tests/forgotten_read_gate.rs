//! Issue #378 ruling #1: the tools-crate read surfaces (MCP sidecar entry
//! lookup, `ct wiki list/get`, keyword entry search) honor the shared
//! forgotten arm. Seeds through the real migration path.

mod common;

use common::{brain_db, seed_fact, with_seeded_brain};
use curated_thoughts_tools::queries::{rank_wiki_entries, wiki_entry_sql, wiki_rows};

fn event(id: &str, entry: &str, forgotten_at: Option<i64>) {
    brain_db()
        .execute(
            "INSERT INTO llm_wiki_events
               (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES (?1, 'ent_t', 'synthesized', 's', ?2, 100, ?3)",
            rusqlite::params![id, entry, forgotten_at],
        )
        .unwrap();
}

fn mcp_bodies(by_entity: bool, arg: &str) -> Vec<String> {
    let conn = brain_db();
    let sql = wiki_entry_sql(&conn, by_entity).unwrap();
    let mut stmt = conn.prepare(&sql).unwrap();
    stmt.query_map([arg], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn tools_surfaces_hide_fully_forgotten_entry_and_keep_survivors() {
    with_seeded_brain(|| {
        seed_fact("gone"); // only backing event forgotten
        seed_fact("kept"); // two events, one forgotten
        seed_fact("plain"); // no events
        event("e-gone", "gone", Some(5));
        event("e-k1", "kept", Some(5));
        event("e-k2", "kept", None);

        let conn = brain_db();
        let ids = |rows: Vec<serde_json::Value>| -> Vec<String> {
            rows.iter()
                .map(|r| r["id"].as_str().unwrap().to_string())
                .collect()
        };

        let rows = ids(wiki_rows(&conn).unwrap());
        assert!(!rows.contains(&"gone".to_string()), "wiki_rows: {rows:?}");
        assert!(rows.contains(&"kept".to_string()), "wiki_rows survival: {rows:?}");
        assert!(rows.contains(&"plain".to_string()));

        let ranked = ids(rank_wiki_entries(&conn, "Body", 10).unwrap());
        assert!(!ranked.contains(&"gone".to_string()), "rank: {ranked:?}");
        assert!(ranked.contains(&"kept".to_string()), "rank survival: {ranked:?}");

        for (by_entity, arg) in [(true, "ent_t"), (false, "Body")] {
            let b = mcp_bodies(by_entity, arg);
            assert!(!b.contains(&"Body gone".to_string()), "mcp {by_entity}: {b:?}");
            assert!(b.contains(&"Body kept".to_string()), "mcp survival {by_entity}: {b:?}");
            assert!(b.contains(&"Body plain".to_string()));
        }
    });
}
