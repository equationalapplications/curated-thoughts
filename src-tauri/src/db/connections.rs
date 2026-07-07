//! Entity connections for Brain mode's right panel: outgoing edges + wikilink backlinks.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityEdgeView {
    pub id: String,
    pub edge_type: String,
    pub source_id: String,
    pub source_label: String,
    pub target_id: String,
    pub target_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityBacklink {
    pub entity_id: String,
    pub name: String,
    pub entity_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityConnections {
    pub outgoing: Vec<EntityEdgeView>,
    pub backlinks: Vec<EntityBacklink>,
}

/// Batch-load endpoint labels (facts + tasks) to avoid N+1 queries.
fn get_endpoint_labels_batch(
    conn: &Connection,
    entity_id: &str,
    record_ids: &[String],
) -> Result<HashMap<String, String>> {
    let mut labels = HashMap::new();

    if record_ids.is_empty() {
        return Ok(labels);
    }

    // Load fact titles in one batch
    let placeholders = record_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query_str = format!(
        "SELECT id, title FROM llm_wiki_entries WHERE entity_id = ? AND id IN ({})",
        placeholders
    );

    let mut stmt = conn.prepare(&query_str)?;
    let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&entity_id];
    for id in record_ids {
        params_vec.push(id);
    }

    let mut iter = stmt.query(rusqlite::params_from_iter(params_vec.iter().copied()))?;
    while let Some(row) = iter.next()? {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        labels.insert(id, title);
    }
    drop(iter);
    drop(stmt);

    // Load task descriptions for any remaining IDs
    let remaining_ids: Vec<&String> = record_ids
        .iter()
        .filter(|id| !labels.contains_key(id.as_str()))
        .collect();
    if !remaining_ids.is_empty() {
        let placeholders = remaining_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query_str = format!(
            "SELECT id, description FROM llm_wiki_tasks WHERE entity_id = ? AND id IN ({})",
            placeholders
        );

        let mut stmt = conn.prepare(&query_str)?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&entity_id];
        for id in &remaining_ids {
            params_vec.push(*id);
        }

        let mut iter = stmt.query(rusqlite::params_from_iter(params_vec.iter().copied()))?;
        while let Some(row) = iter.next()? {
            let id: String = row.get(0)?;
            let description: String = row.get(1)?;
            labels.insert(id, description);
        }
    }

    // For any IDs still missing, use the raw ID as fallback
    for id in record_ids {
        if !labels.contains_key(id) {
            labels.insert(id.clone(), id.clone());
        }
    }

    Ok(labels)
}

fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Outgoing edges (endpoint labels resolved) + name-based wikilink backlinks.
pub fn get_entity_connections(conn: &Connection, entity_id: &str) -> Result<EntityConnections> {
    let mut outgoing = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, source_id, target_id, edge_type FROM llm_wiki_edges
             WHERE entity_id = ?1
             ORDER BY edge_type, created_at",
        )?;
        let rows: Vec<(String, String, String, String)> = stmt
            .query_map([entity_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Collect all endpoint IDs for batch-loading (deduplicated)
        let mut dedup_ids = std::collections::HashSet::new();
        for (_, source_id, target_id, _) in &rows {
            dedup_ids.insert(source_id.clone());
            dedup_ids.insert(target_id.clone());
        }
        let all_record_ids: Vec<String> = dedup_ids.into_iter().collect();

        // Batch-load all labels in two queries (facts, then tasks)
        let labels = get_endpoint_labels_batch(conn, entity_id, &all_record_ids)?;

        // Build edges using pre-loaded labels
        for (id, source_id, target_id, edge_type) in rows {
            outgoing.push(EntityEdgeView {
                id,
                edge_type,
                source_label: labels.get(&source_id).cloned().unwrap_or(source_id.clone()),
                source_id,
                target_label: labels.get(&target_id).cloned().unwrap_or(target_id.clone()),
                target_id,
            });
        }
    }

    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM curated_entities WHERE id = ?1",
            [entity_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(name) = name else {
        return Ok(EntityConnections {
            outgoing,
            backlinks: Vec::new(),
        });
    };

    let pattern = format!("%[[{}]]%", escape_like(&name));
    let mut stmt = conn.prepare(
        "SELECT e.id, e.name, e.entity_type FROM curated_entities e
         WHERE e.id != ?1 AND e.deleted_at IS NULL
           AND (e.summary LIKE ?2 ESCAPE '\\'
                OR EXISTS (SELECT 1 FROM llm_wiki_entries f
                           WHERE f.entity_id = e.id AND f.deleted_at IS NULL
                             AND f.body LIKE ?2 ESCAPE '\\'))
         ORDER BY e.name COLLATE NOCASE",
    )?;
    let backlinks = stmt
        .query_map(params![entity_id, pattern], |r| {
            Ok(EntityBacklink {
                entity_id: r.get(0)?,
                name: r.get(1)?,
                entity_type: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(EntityConnections { outgoing, backlinks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::db::entities::{create_entity, CreateEntityInput};

    fn make_entity(conn: &Connection, name: &str, summary: &str) -> String {
        create_entity(
            conn,
            &CreateEntityInput {
                name: name.into(),
                entity_type: None,
                summary: Some(summary.into()),
            },
        )
        .unwrap()
        .id
    }

    fn seed_fact(conn: &Connection, entity_id: &str, fact_id: &str, title: &str, body: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'user_confirmed', 100, 100)",
            params![fact_id, entity_id, title, body],
        )
        .unwrap();
    }

    #[test]
    fn outgoing_edges_resolve_endpoint_labels() {
        let conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn, "Alpha", "");
        seed_fact(&conn, &entity_id, "fact_a", "Fact A title", "Body A");
        conn.execute(
            "INSERT INTO llm_wiki_tasks (id, entity_id, description, status, priority, created_at, updated_at)
             VALUES ('task_b', ?1, 'Task B description', 'pending', 0, 100, 100)",
            [&entity_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_1', ?1, 'fact_a', 'task_b', 'blocks', 100)",
            [&entity_id],
        )
        .unwrap();

        let connections = get_entity_connections(&conn, &entity_id).unwrap();
        assert_eq!(connections.outgoing.len(), 1);
        let edge = &connections.outgoing[0];
        assert_eq!(edge.edge_type, "blocks");
        assert_eq!(edge.source_label, "Fact A title");
        assert_eq!(edge.target_label, "Task B description");
    }

    #[test]
    fn backlinks_found_in_fact_bodies_and_summaries() {
        let conn = open_in_memory().unwrap();
        let alpha = make_entity(&conn, "Alpha", "");
        let by_fact = make_entity(&conn, "Fact Referrer", "");
        seed_fact(&conn, &by_fact, "fact_1", "T", "Works with [[Alpha]] weekly.");
        let by_summary = make_entity(&conn, "Summary Referrer", "Depends on [[Alpha]].");
        // Self-mention and unrelated entity must not appear.
        seed_fact(&conn, &alpha, "fact_self", "T", "I am [[Alpha]].");
        make_entity(&conn, "Bystander", "Nothing relevant.");

        let connections = get_entity_connections(&conn, &alpha).unwrap();
        let names: Vec<&str> = connections.backlinks.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["Fact Referrer", "Summary Referrer"]);
        assert_eq!(connections.backlinks[0].entity_id, by_fact);
        assert_eq!(connections.backlinks[1].entity_id, by_summary);
    }

    #[test]
    fn like_wildcards_in_entity_name_are_escaped() {
        let conn = open_in_memory().unwrap();
        let pct = make_entity(&conn, "100% Done", "");
        let exact = make_entity(&conn, "Exact Referrer", "See [[100% Done]].");
        // Would match "100% Done" under an unescaped LIKE '%[[100% Done]]%'? No —
        // but an unescaped '%' means '[[100' + anything + ' Done]]' matches too:
        make_entity(&conn, "Loose Referrer", "See [[100 NOT Done]].");
        let _ = exact;

        let connections = get_entity_connections(&conn, &pct).unwrap();
        let names: Vec<&str> = connections.backlinks.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["Exact Referrer"]);
    }
}
