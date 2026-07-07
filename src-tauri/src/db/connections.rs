//! Entity connections for Brain mode's right panel: outgoing edges + wikilink backlinks.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

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

fn endpoint_label(conn: &Connection, entity_id: &str, record_id: &str) -> Result<String> {
    let title: Option<String> = conn
        .query_row(
            "SELECT title FROM llm_wiki_entries WHERE id = ?1 AND entity_id = ?2",
            params![record_id, entity_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(title) = title {
        return Ok(title);
    }
    let description: Option<String> = conn
        .query_row(
            "SELECT description FROM llm_wiki_tasks WHERE id = ?1 AND entity_id = ?2",
            params![record_id, entity_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(description.unwrap_or_else(|| record_id.to_string()))
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
        for (id, source_id, target_id, edge_type) in rows {
            outgoing.push(EntityEdgeView {
                id,
                edge_type,
                source_label: endpoint_label(conn, entity_id, &source_id)?,
                source_id,
                target_label: endpoint_label(conn, entity_id, &target_id)?,
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
