//! Read-only queries over core-llm-wiki tables in brain.db (llm_wiki_entries, llm_wiki_edges,
//! llm_wiki_entity_manifests). Used by MCP tools; no mutation paths.

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::search::{bytes_to_f32, cosine_similarity};

pub const DEFAULT_ENTITY_IDS: &[&str] = &["tier_fact", "tier_wisdom"];
pub const MAX_TRAVERSAL_NODES: usize = 50;
pub const DEFAULT_MAX_DEPTH: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiSearchHit {
    pub id: String,
    pub entity_id: String,
    pub title: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiOntologyResult {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<WikiManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiManifest {
    pub node_types: Vec<String>,
    pub edge_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiTraverseNode {
    pub id: String,
    pub title: String,
    pub entity_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiTraverseEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiTraverseResult {
    pub nodes: Vec<WikiTraverseNode>,
    pub edges: Vec<WikiTraverseEdge>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraverseDirection {
    Inbound,
    Outbound,
    Both,
}

impl TraverseDirection {
    pub fn parse(s: &str) -> Self {
        match s {
            "inbound" => Self::Inbound,
            "outbound" => Self::Outbound,
            _ => Self::Both,
        }
    }
}

/// Mirrors `tieredRead` weights in `src/lib/wiki.ts` (tier_fact 1.5×, tier_wisdom 1.0×, other 1.0×).
pub fn tier_weight(entity_id: &str) -> f32 {
    match entity_id {
        "tier_fact" => 1.5,
        "tier_wisdom" => 1.0,
        _ => 1.0,
    }
}

pub fn clamp_max_depth(requested: usize) -> usize {
    let clamped = requested.clamp(1, 3);
    if clamped != requested {
        eprintln!("wiki_traverse_graph: clamped maxDepth {requested} to {clamped}");
    }
    clamped
}

pub fn f32_vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

pub fn wiki_get_ontology(conn: &Connection, entity_id: &str) -> Result<WikiOntologyResult> {
    let mut stmt = conn.prepare(
        "SELECT mode, manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = ?1",
    )?;
    let mut rows = stmt.query([entity_id])?;
    let Some(row) = rows.next()? else {
        return Ok(WikiOntologyResult {
            mode: "off".into(),
            manifest: None,
        });
    };
    let mode: String = row.get(0)?;
    let manifest_json: String = row.get(1)?;
    let parsed: WikiManifest = serde_json::from_str(&manifest_json)?;
    Ok(WikiOntologyResult {
        mode,
        manifest: Some(parsed),
    })
}

pub fn wiki_search(
    conn: &Connection,
    query_vec: &[f32],
    entity_ids: &[&str],
    limit: usize,
) -> Result<Vec<WikiSearchHit>> {
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 25);
    let dim = query_vec.len();
    let placeholders = entity_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, entity_id, title, embedding_blob
         FROM llm_wiki_entries
         WHERE entity_id IN ({placeholders}) AND deleted_at IS NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(entity_ids.iter()))?;
    let mut scored: Vec<WikiSearchHit> = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let entity_id: String = row.get(1)?;
        let title: String = row.get(2)?;
        let bytes: Vec<u8> = row.get(3)?;
        if bytes.len() / 4 != dim {
            continue;
        }
        let vec = bytes_to_f32(&bytes);
        let raw = cosine_similarity(query_vec, &vec);
        if raw <= 0.0 {
            continue;
        }
        let score = raw * tier_weight(&entity_id);
        scored.push(WikiSearchHit {
            id,
            entity_id,
            title,
            score,
        });
    }
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    Ok(scored)
}

fn load_live_entry(
    conn: &Connection,
    entity_id: &str,
    id: &str,
) -> Result<Option<WikiTraverseNode>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, entity_id FROM llm_wiki_entries
         WHERE id = ?1 AND entity_id = ?2 AND deleted_at IS NULL",
    )?;
    let mut rows = stmt.query(rusqlite::params![id, entity_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(WikiTraverseNode {
        id: row.get(0)?,
        title: row.get(1)?,
        entity_id: row.get(2)?,
    }))
}

fn fetch_neighbors(
    conn: &Connection,
    entity_id: &str,
    node_id: &str,
    direction: TraverseDirection,
    edge_types: &[&str],
) -> Result<Vec<(WikiTraverseEdge, String)>> {
    let edge_filter = if edge_types.is_empty() {
        String::new()
    } else {
        let ph: String = edge_types.iter().map(|_| "?,").collect();
        format!(" AND e.edge_type IN ({})", ph.trim_end_matches(','))
    };

    let mut out = Vec::new();

    if matches!(direction, TraverseDirection::Outbound | TraverseDirection::Both) {
        fetch_outbound_neighbors(
            conn,
            entity_id,
            node_id,
            &edge_filter,
            edge_types,
            &mut out,
        )?;
    }
    if matches!(direction, TraverseDirection::Inbound | TraverseDirection::Both) {
        fetch_inbound_neighbors(conn, entity_id, node_id, &edge_filter, edge_types, &mut out)?;
    }
    Ok(out)
}

fn fetch_outbound_neighbors(
    conn: &Connection,
    entity_id: &str,
    node_id: &str,
    edge_filter: &str,
    edge_types: &[&str],
    out: &mut Vec<(WikiTraverseEdge, String)>,
) -> Result<()> {
    let sql = format!(
        "SELECT e.source_id, e.target_id, e.edge_type, t.id, t.title, t.entity_id
         FROM llm_wiki_edges e
         JOIN llm_wiki_entries s ON s.id = e.source_id AND s.deleted_at IS NULL
         JOIN llm_wiki_entries t ON t.id = e.target_id AND t.deleted_at IS NULL
         WHERE e.entity_id = ?1 AND e.source_id = ?2{edge_filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
        vec![Box::new(entity_id.to_string()), Box::new(node_id.to_string())];
    for et in edge_types {
        params.push(Box::new(et.to_string()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;
    while let Some(row) = rows.next()? {
        let edge = WikiTraverseEdge {
            source_id: row.get(0)?,
            target_id: row.get(1)?,
            edge_type: row.get(2)?,
        };
        let neighbor_id: String = row.get(3)?;
        out.push((edge, neighbor_id));
    }
    Ok(())
}

fn fetch_inbound_neighbors(
    conn: &Connection,
    entity_id: &str,
    node_id: &str,
    edge_filter: &str,
    edge_types: &[&str],
    out: &mut Vec<(WikiTraverseEdge, String)>,
) -> Result<()> {
    let sql = format!(
        "SELECT e.source_id, e.target_id, e.edge_type, s.id, s.title, s.entity_id
         FROM llm_wiki_edges e
         JOIN llm_wiki_entries s ON s.id = e.source_id AND s.deleted_at IS NULL
         JOIN llm_wiki_entries t ON t.id = e.target_id AND t.deleted_at IS NULL
         WHERE e.entity_id = ?1 AND e.target_id = ?2{edge_filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
        vec![Box::new(entity_id.to_string()), Box::new(node_id.to_string())];
    for et in edge_types {
        params.push(Box::new(et.to_string()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;
    while let Some(row) = rows.next()? {
        let edge = WikiTraverseEdge {
            source_id: row.get(0)?,
            target_id: row.get(1)?,
            edge_type: row.get(2)?,
        };
        let neighbor_id: String = row.get(3)?;
        out.push((edge, neighbor_id));
    }
    Ok(())
}

pub fn wiki_traverse_graph(
    conn: &Connection,
    entity_id: &str,
    source_id: &str,
    max_depth: usize,
    direction: TraverseDirection,
    edge_types: &[&str],
) -> Result<WikiTraverseResult> {
    let max_depth = clamp_max_depth(max_depth);
    let Some(seed) = load_live_entry(conn, entity_id, source_id)? else {
        return Ok(WikiTraverseResult {
            nodes: Vec::new(),
            edges: Vec::new(),
            truncated: false,
        });
    };

    let mut nodes: HashMap<String, WikiTraverseNode> = HashMap::new();
    let mut edges: Vec<WikiTraverseEdge> = Vec::new();
    let mut edge_keys: HashSet<(String, String, String)> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    nodes.insert(seed.id.clone(), seed.clone());
    visited.insert(source_id.to_string());
    queue.push_back((source_id.to_string(), 0));

    let mut truncated = false;

    while let Some((current_id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let pairs = fetch_neighbors(conn, entity_id, &current_id, direction, edge_types)?;
        for (edge, neighbor_id) in pairs {
            let key = (
                edge.source_id.clone(),
                edge.target_id.clone(),
                edge.edge_type.clone(),
            );
            if edge_keys.insert(key) {
                edges.push(edge);
            }
            if !visited.contains(&neighbor_id) {
                if nodes.len() >= MAX_TRAVERSAL_NODES {
                    truncated = true;
                    break;
                }
                if let Some(node) = load_live_entry(conn, entity_id, &neighbor_id)? {
                    visited.insert(neighbor_id.clone());
                    nodes.insert(neighbor_id.clone(), node);
                    queue.push_back((neighbor_id, depth + 1));
                }
            }
        }
        if truncated {
            break;
        }
    }

    let mut node_list: Vec<WikiTraverseNode> = nodes.into_values().collect();
    node_list.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(WikiTraverseResult {
        nodes: node_list,
        edges,
        truncated,
    })
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn tier_weight_matches_tiered_read() {
        assert_eq!(tier_weight("tier_fact"), 1.5);
        assert_eq!(tier_weight("tier_wisdom"), 1.0);
        assert_eq!(tier_weight("tier_working::abc"), 1.0);
        assert_eq!(tier_weight("custom_entity"), 1.0);
    }
}
