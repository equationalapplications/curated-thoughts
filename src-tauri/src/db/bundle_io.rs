//! DB → ExportEntity for OKF bundle export. Includes soft-deleted
//! facts/tasks (round-trip fidelity, profile §9 table).

use anyhow::Result;
use rusqlite::Connection;

use crate::okf::bundle_write::{ExportEntity, ExportEvent};
use crate::okf::timefmt::utc_date_from_ms;
use crate::okf::types::{WikiFact, WikiTask};

pub fn load_export_entities(
    conn: &Connection,
    entity_ids: Option<&[String]>,
) -> Result<Vec<ExportEntity>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, summary FROM curated_entities
         WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut entities = Vec::new();
    for (id, name, summary) in rows {
        if let Some(wanted) = entity_ids {
            if !wanted.contains(&id) {
                continue;
            }
        }
        entities.push(ExportEntity {
            facts: load_facts(conn, &id)?,
            tasks: load_tasks(conn, &id)?,
            edges: load_edges(conn, &id)?,
            events: load_events(conn, &id)?,
            summary: if summary.trim().is_empty() {
                None
            } else {
                Some(summary)
            },
            display_name: name,
            entity_id: id,
        });
    }
    Ok(entities)
}

fn load_facts(conn: &Connection, entity_id: &str) -> Result<Vec<WikiFact>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, body, tags, confidence, source_type, source_hash, source_ref,
                created_at, updated_at, last_accessed_at, access_count, deleted_at, okf_type,
                lifecycle_status, stale_after, generated_by, last_verified_at, last_verified_by,
                okf_sources, okf_verified, okf_usage_window
         FROM llm_wiki_entries WHERE entity_id = ?1 ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([entity_id], |r| {
        Ok(WikiFact {
            id: r.get(0)?,
            entity_id: entity_id.to_string(),
            title: r.get(1)?,
            body: r.get(2)?,
            tags: serde_json::from_str::<Vec<String>>(&r.get::<_, String>(3)?).unwrap_or_default(),
            confidence: r.get(4)?,
            source_type: r.get(5)?,
            source_hash: r.get(6)?,
            source_ref: r.get(7)?,
            created_at: r.get(8)?,
            updated_at: r.get(9)?,
            last_accessed_at: r.get(10)?,
            access_count: r.get(11)?,
            deleted_at: r.get(12)?,
            okf_type: r.get(13)?,
            lifecycle_status: r.get(14)?,
            stale_after: r.get(15)?,
            generated_by: r.get(16)?,
            last_verified_at: r.get(17)?,
            last_verified_by: r.get(18)?,
            okf_sources: r.get(19)?,
            okf_verified: r.get(20)?,
            okf_usage_window: r.get(21)?,
            // Paired librarian_evidence blob, so the bundle carries a
            // librarian fact's provenance with it. Spec §2.3. SQLite errors
            // PROPAGATE here (review round 5): this is the export path, and
            // a swallowed transient fault would write bundles whose librarian
            // facts carry bare tokens with no paired evidence.
            evidence_json: crate::db::commit::evidence_json_for_entry(
                conn,
                &r.get::<_, String>(0)?,
            )?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn load_tasks(conn: &Connection, entity_id: &str) -> Result<Vec<WikiTask>> {
    let mut stmt = conn.prepare(
        "SELECT id, description, status, priority, created_at, updated_at,
                resolved_at, deleted_at, okf_type,
                lifecycle_status, stale_after, generated_by,
                last_verified_at, last_verified_by,
                okf_sources, okf_verified, okf_usage_window
         FROM llm_wiki_tasks WHERE entity_id = ?1 ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([entity_id], |r| {
        Ok(WikiTask {
            id: r.get(0)?,
            entity_id: entity_id.to_string(),
            description: r.get(1)?,
            status: r.get(2)?,
            priority: r.get(3)?,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
            resolved_at: r.get(6)?,
            deleted_at: r.get(7)?,
            okf_type: r.get(8)?,
            lifecycle_status: r.get(9)?,
            stale_after: r.get(10)?,
            generated_by: r.get(11)?,
            last_verified_at: r.get(12)?,
            last_verified_by: r.get(13)?,
            okf_sources: r.get(14)?,
            okf_verified: r.get(15)?,
            okf_usage_window: r.get(16)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn load_edges(conn: &Connection, entity_id: &str) -> Result<Vec<(String, String, String)>> {
    // Read-side manifest filter (issue #158). Bundle export was the second
    // user-visible surface Brain Connections missed: an exported bundle
    // carried the off-manifest edge into another vault. Apply the same gate
    // `wiki_graph::fetch_neighbors` uses on the traversal path.
    let vocab = crate::db::commit::resolve_strict_edge_vocabulary(conn, entity_id);
    let mut stmt = conn.prepare(
        "SELECT source_id, target_id, edge_type FROM llm_wiki_edges
         WHERE entity_id = ?1 ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([entity_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (source_id, target_id, edge_type): (String, String, String) = row?;
        let keep = match &vocab {
            Some(v) => v.contains(&edge_type.trim().to_lowercase()),
            None => true,
        };
        if keep {
            out.push((source_id, target_id, edge_type));
        }
    }
    Ok(out)
}

fn load_events(conn: &Connection, entity_id: &str) -> Result<Vec<ExportEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, event_type, summary, related_entry_id, created_at
         FROM llm_wiki_events WHERE entity_id = ?1 ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([entity_id], |r| {
        let created_at: i64 = r.get(4)?;
        Ok(ExportEvent {
            event_id: r.get(0)?,
            event_type: r.get(1)?,
            summary: r.get(2)?,
            related_entry_id: r.get(3)?,
            date: utc_date_from_ms(created_at),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn seed(conn: &rusqlite::Connection) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'Project X', 'project', 'Summary prose.', 100, 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at)
             VALUES ('fact_1', 'ent_a', 'A fact', 'Body.', '[\"t\"]', 'certain', 'user_confirmed', 1719835200000, 1719835200000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_tasks (id, entity_id, description, status, priority, created_at, updated_at)
             VALUES ('task_1', 'ent_a', 'Do it', 'pending', 1, 1719835800000, 1719835800000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_1', 'ent_a', 'fact_1', 'task_1', 'blocks', 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
             VALUES ('evt_1', 'ent_a', 'action', 'Did a thing', 'fact_1', 1783209600000)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn loads_full_entity_for_export() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let entities = load_export_entities(&conn, None).unwrap();
        assert_eq!(entities.len(), 1);
        let e = &entities[0];
        assert_eq!(e.entity_id, "ent_a");
        assert_eq!(e.display_name, "Project X");
        assert_eq!(e.summary.as_deref(), Some("Summary prose."));
        assert_eq!(e.facts.len(), 1);
        assert_eq!(e.tasks.len(), 1);
        assert_eq!(
            e.edges,
            vec![("fact_1".into(), "task_1".into(), "blocks".into())]
        );
        assert_eq!(e.events.len(), 1);
        assert_eq!(e.events[0].event_id, "evt_1");
        assert_eq!(
            e.events[0].date,
            crate::okf::timefmt::utc_date_from_ms(1783209600000)
        );
    }

    #[test]
    fn filters_by_entity_ids() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let none = load_export_entities(&conn, Some(&["ent_missing".to_string()])).unwrap();
        assert!(none.is_empty());
    }

    /// Regression test for issue #158 on the bundle-export read path.
    /// `load_edges` is private — the public caller `load_export_entities`
    /// is exercised here, asserting that an exported bundle does not carry
    /// off-manifest edges out of a strict brain.
    #[test]
    fn load_edges_filters_off_manifest_types_when_ontology_is_strict() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent_a', 'strict',
                     '{\"node_types\":[{\"type\":\"fact\",\"description\":\"\"}],\
                      \"edge_types\":[{\"type\":\"blocks\",\"source_type\":\"fact\",\
                      \"target_type\":\"task\",\"description\":\"\"}]}', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_off', 'ent_a', 'fact_1', 'task_1', 'fabricated_2026-09-09', 101)",
            [],
        )
        .unwrap();

        let exported = load_export_entities(&conn, None).unwrap();
        let edge_types: Vec<&str> = exported[0]
            .edges
            .iter()
            .map(|(_, _, t)| t.as_str())
            .collect();
        assert_eq!(
            edge_types,
            vec!["blocks"],
            "an exported bundle must not leak off-manifest edges"
        );
    }
}
