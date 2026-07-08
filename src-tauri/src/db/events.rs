//! Unified Timeline API: unions llm_wiki_events, curated_agent_log, documents (ingestions).
//! All timestamps normalized to milliseconds, reversed chronologically, with optional filtering.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Unified timeline event across all three sources.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineEvent {
    pub id: String,
    pub kind: String, // 'synthesized'|'approved'|'rejected'|'healed'|'imported'|'exported'|'agent_access'|'ingested'|'other'
    pub summary: String,
    pub entity_id: Option<String>,
    pub entity_name: Option<String>,
    pub doc_path: Option<String>,
    pub raw_type: String, // event_type / tool / document status
    pub client: Option<String>,
    pub created_at_ms: i64,
}

/// Query filter for timeline events.
#[derive(Debug, Default, Deserialize)]
pub struct TimelineFilter {
    pub kinds: Option<Vec<String>>,
    pub entity_id: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub before_ms: Option<i64>, // cursor: return strictly older than this
    pub limit: Option<u32>,
}

/// List timeline events from all three sources, normalized and filtered.
pub fn list_events(conn: &Connection, filter: &TimelineFilter) -> Result<Vec<TimelineEvent>> {
    let limit = filter.limit.unwrap_or(100).min(500) as i64;

    // Build the UNION query that normalizes timestamps to milliseconds
    // and maps event types to kinds.
    let sql = r#"
        SELECT * FROM (
            -- llm_wiki_events (timestamps in seconds, need *1000)
            SELECT
                e.id,
                CASE
                    WHEN e.event_type IN ('synthesized','approved','rejected','healed','imported','exported')
                    THEN e.event_type
                    ELSE 'other'
                END AS kind,
                e.summary,
                e.entity_id,
                ce.name AS entity_name,
                NULL AS doc_path,
                e.event_type AS raw_type,
                NULL AS client,
                CASE
                    WHEN e.created_at < 100000000000 THEN e.created_at * 1000
                    ELSE e.created_at
                END AS created_at_ms
            FROM llm_wiki_events e
            LEFT JOIN curated_entities ce ON ce.id = e.entity_id

            UNION ALL

            -- curated_agent_log (timestamps in seconds, need *1000)
            SELECT
                'agent_' || CAST(a.id AS TEXT),
                'agent_access',
                a.client || ' called ' || a.tool,
                a.entity_id,
                ce.name,
                NULL,
                a.tool,
                a.client,
                a.created_at * 1000
            FROM curated_agent_log a
            LEFT JOIN curated_entities ce ON ce.id = a.entity_id

            UNION ALL

            -- documents (ingestions): tier='user_doc' with last_indexed set)
            SELECT
                'ingest_' || CAST(d.id AS TEXT),
                'ingested',
                'Ingested *' || d.path || '*',
                NULL,
                NULL,
                d.path,
                d.status,
                NULL,
                d.last_indexed * 1000
            FROM documents d
            WHERE d.tier = 'user_doc' AND d.last_indexed IS NOT NULL
        )
        WHERE
            (:entity_id IS NULL OR entity_id = :entity_id)
            AND (:before_ms IS NULL OR created_at_ms < :before_ms)
            AND (:since_ms IS NULL OR created_at_ms >= :since_ms)
            AND (:until_ms IS NULL OR created_at_ms <= :until_ms)
        ORDER BY created_at_ms DESC
        LIMIT :limit
    "#;

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        params![
            filter.entity_id.as_deref(),
            filter.before_ms,
            filter.since_ms,
            filter.until_ms,
            limit,
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, i64>(8)?,
            ))
        },
    )?;

    let mut events = Vec::new();
    for row in rows {
        let (id, kind, summary, entity_id, entity_name, doc_path, raw_type, client, created_at_ms) =
            row?;

        // Filter by kinds if specified
        if let Some(ref kinds) = filter.kinds {
            if !kinds.contains(&kind) {
                continue;
            }
        }

        events.push(TimelineEvent {
            id,
            kind,
            summary,
            entity_id,
            entity_name,
            doc_path,
            raw_type,
            client,
            created_at_ms,
        });
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn setup_test_db() -> Result<Connection> {
        open_in_memory()
    }

    #[test]
    fn unions_and_orders_all_three_legs() -> Result<()> {
        let conn = setup_test_db()?;

        // Seed a wiki event (timestamp in seconds)
        let wiki_event_ts = 1000; // seconds
        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, created_at)
             VALUES ('wiki_1', 'ent_1', 'approved', 'Wiki event', ?1)",
            params![wiki_event_ts],
        )?;

        // Seed an agent log (created_at defaults to unixepoch(), which is seconds)
        let agent_log_ts = 1500; // seconds
        conn.execute(
            "INSERT INTO curated_agent_log (client, tool, operation, entity_id, created_at)
             VALUES ('test_client', 'test_tool', 'read', 'ent_1', ?1)",
            params![agent_log_ts],
        )?;

        // Seed a document ingestion (last_indexed in seconds)
        let doc_ingest_ts = 1200; // seconds
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status, last_indexed)
             VALUES ('test.md', 'hash1', 'user_doc', 'indexed', ?1)",
            params![doc_ingest_ts],
        )?;

        // Query all events
        let filter = TimelineFilter::default();
        let events = list_events(&conn, &filter)?;

        // Should have 3 events
        assert_eq!(events.len(), 3, "Expected 3 events, got {}", events.len());

        // Check timestamps are in milliseconds
        let wiki_event = events.iter().find(|e| e.id == "wiki_1").unwrap();
        assert_eq!(wiki_event.created_at_ms, wiki_event_ts * 1000);

        let agent_event = events
            .iter()
            .find(|e| e.id.starts_with("agent_"))
            .unwrap();
        assert_eq!(agent_event.created_at_ms, agent_log_ts * 1000);

        let ingest_event = events
            .iter()
            .find(|e| e.id.starts_with("ingest_"))
            .unwrap();
        assert_eq!(ingest_event.created_at_ms, doc_ingest_ts * 1000);

        // Check reverse chronological order (newest first)
        assert!(
            events[0].created_at_ms > events[1].created_at_ms,
            "Events not in reverse chronological order"
        );
        assert!(
            events[1].created_at_ms > events[2].created_at_ms,
            "Events not in reverse chronological order"
        );

        Ok(())
    }

    #[test]
    fn known_event_types_map_to_kind_and_unknown_to_other() -> Result<()> {
        let conn = setup_test_db()?;

        // Seed events with different event_types
        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, created_at)
             VALUES ('wiki_approved', 'ent_1', 'approved', 'Approved', 1000)",
            [],
        )?;

        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, created_at)
             VALUES ('wiki_synthesized', 'ent_1', 'synthesized', 'Synthesized', 1001)",
            [],
        )?;

        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, created_at)
             VALUES ('wiki_weird', 'ent_1', 'weird_custom_type', 'Weird', 1002)",
            [],
        )?;

        let filter = TimelineFilter::default();
        let events = list_events(&conn, &filter)?;

        assert_eq!(events.len(), 3);

        let approved = events.iter().find(|e| e.id == "wiki_approved").unwrap();
        assert_eq!(approved.kind, "approved");
        assert_eq!(approved.raw_type, "approved");

        let synthesized = events
            .iter()
            .find(|e| e.id == "wiki_synthesized")
            .unwrap();
        assert_eq!(synthesized.kind, "synthesized");
        assert_eq!(synthesized.raw_type, "synthesized");

        let weird = events.iter().find(|e| e.id == "wiki_weird").unwrap();
        assert_eq!(weird.kind, "other");
        assert_eq!(weird.raw_type, "weird_custom_type");

        Ok(())
    }

    #[test]
    fn filters_by_kind_entity_and_cursor() -> Result<()> {
        let conn = setup_test_db()?;

        // Create entity
        conn.execute(
            "INSERT INTO curated_entities (id, name, created_at, updated_at)
             VALUES ('ent_1', 'Entity 1', 1000, 1000)",
            [],
        )?;

        // Seed multiple events
        for i in 0..5 {
            let ts = 1000 + (i * 100);
            let event_type = if i % 2 == 0 { "approved" } else { "rejected" };
            conn.execute(
                "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, created_at)
                 VALUES ('wiki_' || ?1, 'ent_1', ?2, 'Event ' || ?1, ?3)",
                params![i, event_type, ts],
            )?;
        }

        // Filter by kind='approved' (should only get i=0,2,4)
        let filter = TimelineFilter {
            kinds: Some(vec!["approved".to_string()]),
            ..Default::default()
        };
        let events = list_events(&conn, &filter)?;
        assert_eq!(events.len(), 3);
        for event in &events {
            assert_eq!(event.kind, "approved");
        }

        // Filter by entity_id='ent_1'
        let filter = TimelineFilter {
            entity_id: Some("ent_1".to_string()),
            ..Default::default()
        };
        let events = list_events(&conn, &filter)?;
        assert_eq!(events.len(), 5);
        for event in &events {
            assert_eq!(event.entity_id, Some("ent_1".to_string()));
        }

        // Filter by before_ms (cursor pagination)
        let middle_ts_ms = 1200 * 1000; // one of the middle events
        let filter = TimelineFilter {
            before_ms: Some(middle_ts_ms),
            ..Default::default()
        };
        let events = list_events(&conn, &filter)?;
        for event in &events {
            assert!(event.created_at_ms < middle_ts_ms);
        }

        Ok(())
    }
}
