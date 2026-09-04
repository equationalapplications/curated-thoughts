//! Read-only queries over core-llm-wiki tables in brain.db (llm_wiki_entries, llm_wiki_edges,
//! llm_wiki_entity_manifests). Used by MCP tools; no mutation paths.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::search::{bytes_to_f32, cosine_similarity};

pub const MAX_TRAVERSAL_NODES: usize = 50;
pub const DEFAULT_MAX_DEPTH: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiSearchHit {
    pub id: String,
    pub entity_id: String,
    pub title: String,
    pub score: f32,
    /// Stored entry tier: `fact`, `wisdom`, or None for an ordinary live entry.
    /// Independent of `entity_id` — tier is not a namespace (spec §3.4).
    pub tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiOntologyResult {
    pub mode: String,
    pub manifest: Option<WikiManifest>,
}

/// One declared node type. Mirrors `core-llm-wiki`'s `OntologyNodeType`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WikiNodeType {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// One-level parent slug, when the manifest declares inheritance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_type: Option<String>,
}

/// One declared edge type. Mirrors `core-llm-wiki`'s `OntologyEdgeType`.
///
/// A manifest may declare the same `type` several times with different
/// endpoints, so a name is a *set* of triples — membership by name is what the
/// strict write gate tests (spec §2.3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WikiEdgeType {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default)]
    pub source_type: String,
    #[serde(default)]
    pub target_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WikiManifest {
    pub node_types: Vec<WikiNodeType>,
    pub edge_types: Vec<WikiEdgeType>,
}

impl WikiManifest {
    /// Whether `edge_type` is declared, compared case-insensitively.
    ///
    /// Case-insensitive to match the engine's own `resolveEdgeDefinitions`,
    /// which lowercases both sides. A guard stricter than the producer would
    /// reject edge types the librarian was told were legal.
    pub fn declares_edge_type(&self, edge_type: &str) -> bool {
        let needle = edge_type.trim().to_lowercase();
        self.edge_types
            .iter()
            .any(|e| e.type_name.trim().to_lowercase() == needle)
    }

    /// Declared edge-type names in manifest order, deduplicated — the
    /// vocabulary a rejection diagnostic names.
    pub fn edge_type_names(&self) -> Vec<&str> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for e in &self.edge_types {
            if seen.insert(e.type_name.trim().to_lowercase()) {
                out.push(e.type_name.as_str());
            }
        }
        out
    }
}

/// Parse a stored `manifest_json` blob.
///
/// Tolerates both shapes an `llm_wiki_entity_manifests` row can hold. The
/// engine writes `JSON.stringify(manifest)`, whose entries are **objects**
/// (`{type, description, ...}`) — the shape this reader was originally typed
/// against as `Vec<String>`, which could not deserialize a real seeded manifest
/// at all. Bare strings are still accepted so a hand-written or legacy row
/// degrades to a name-only entry rather than failing the whole read: a
/// `wiki_get_ontology` that errors is indistinguishable to a caller from a
/// brain with no ontology, which is exactly the confusion §2.1 exists to end.
fn parse_manifest(manifest_json: &str) -> Result<WikiManifest> {
    fn entries(value: Option<&serde_json::Value>) -> Vec<&serde_json::Value> {
        value
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    }
    fn field(v: &serde_json::Value, key: &str) -> Option<String> {
        v.get(key)
            .and_then(|f| f.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }

    let root: serde_json::Value = serde_json::from_str(manifest_json)?;

    let node_types = entries(root.get("node_types"))
        .into_iter()
        .filter_map(|v| match v {
            serde_json::Value::String(name) => Some(WikiNodeType {
                type_name: name.clone(),
                ..Default::default()
            }),
            _ => field(v, "type").map(|type_name| WikiNodeType {
                type_name,
                description: field(v, "description"),
                parent_type: field(v, "parent_type"),
            }),
        })
        .collect();

    let edge_types = entries(root.get("edge_types"))
        .into_iter()
        .filter_map(|v| match v {
            serde_json::Value::String(name) => Some(WikiEdgeType {
                type_name: name.clone(),
                ..Default::default()
            }),
            _ => field(v, "type").map(|type_name| WikiEdgeType {
                type_name,
                source_type: field(v, "source_type").unwrap_or_default(),
                target_type: field(v, "target_type").unwrap_or_default(),
                description: field(v, "description"),
            }),
        })
        .collect();

    Ok(WikiManifest {
        node_types,
        edge_types,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiTraverseNode {
    pub id: String,
    pub title: String,
    pub entity_id: String,
}

/// One traversed edge. `entity_id` is the partition the edge lives in — two
/// seeded partitions (e.g. `tier_fact` and `tier_wisdom`) can hold the same
/// `(source_id, target_id, edge_type)`, and consumers must be able to tell
/// which partition a returned edge belongs to. The field is set on
/// construction (see `CompositeWalk::edge_keys`) so no caller can produce a
/// `WikiTraverseEdge` without a partition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiTraverseEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
    pub entity_id: String,
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

/// Mirrors `tieredRead` weights in `src/lib/wiki.ts` (tier_fact 1.5×, tier_wisdom 1.0×, tier_working::* 0.6×, other 1.0×).
pub fn tier_weight(entity_id: &str) -> f32 {
    match entity_id {
        "tier_fact" => 1.5,
        "tier_wisdom" => 1.0,
        id if id.starts_with("tier_working::") => 0.6,
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
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
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
    let parsed = parse_manifest(&manifest_json)?;
    Ok(WikiOntologyResult {
        mode,
        manifest: Some(parsed),
    })
}

/// Semantic search over live, embedded wiki entries.
///
/// `entity_ids` is the caller's filter:
/// - `None` — search **every** live embedded entry. This is the default call
///   path. It must not assume a namespace: entries written by the librarian
///   carry `ent_<hash>` ids, and a reader that guesses at namespaces is
///   exactly the bug this replaced (#133).
/// - `Some(&[])` — match nothing, preserving the prior explicit-empty contract.
/// - `Some(ids)` — filter to those entity ids.
///
/// `tier` is a second, independent filter (spec §3.4):
/// - `None` — no tier narrowing. This is the default and preserves the #133
///   all-live-entries contract exactly.
/// - `Some("fact" | "wisdom")` — only entries stored at that tier.
///
/// Tier is deliberately NOT an `entity_id` namespace. `tier_fact`/`tier_wisdom`
/// have no production writer, and making them one would require the writer
/// migration PR #135 rejected as the only change that can corrupt data.
///
/// Ranking is unaffected: `tier_weight` is applied per row either way, so a
/// `tier_fact` entry keeps its 1.5x bonus wherever tier namespaces exist.
pub fn wiki_search(
    conn: &Connection,
    query_vec: &[f32],
    entity_ids: Option<&[&str]>,
    tier: Option<&str>,
    limit: usize,
) -> Result<Vec<WikiSearchHit>> {
    if entity_ids.is_some_and(|ids| ids.is_empty()) {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 25);
    let dim = query_vec.len();
    let entity_filter = match entity_ids {
        Some(ids) => {
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            format!("entity_id IN ({placeholders}) AND ")
        }
        None => String::new(),
    };
    let tier_filter = if tier.is_some() { "tier = ? AND " } else { "" };
    let sql = format!(
        "SELECT id, entity_id, title, embedding_blob, tier
         FROM llm_wiki_entries
         WHERE {entity_filter}{tier_filter}deleted_at IS NULL AND embedding_blob IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    // Bind order matches the clause order built above: entity ids, then tier.
    let mut binds: Vec<&str> = entity_ids.map(|ids| ids.to_vec()).unwrap_or_default();
    if let Some(t) = tier {
        binds.push(t);
    }
    let mut rows = stmt.query(rusqlite::params_from_iter(binds.iter()))?;
    let mut scored: Vec<WikiSearchHit> = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let entity_id: String = row.get(1)?;
        let title: String = row.get(2)?;
        let bytes: Option<Vec<u8>> = row.get(3)?;
        let tier_value: Option<String> = row.get(4)?;
        let Some(bytes) = bytes else {
            continue;
        };
        if bytes.len() != dim * 4 {
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
            tier: tier_value,
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

/// Which endpoint table a traversal is walking.
///
/// PR #131's write contract admits three endpoint tables
/// (`llm_wiki_entries` ∪ `curated_entities` ∪ `llm_wiki_tasks`); the reader
/// handles the two that carry live edges today. The space is decided once, at
/// the seed, and a walk never crosses — mixed results would need a
/// discriminator on `WikiTraverseNode` and a merge rule for two
/// differently-keyed neighbor sets, which no caller needs (#134).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeSpace {
    Entry,
    Entity,
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

/// Resolve a live `curated_entities` row as a traversal node.
///
/// `curated_entities` has no `entity_id` column, so the caller's `entity_id`
/// (the edge partition being walked) is what the node reports. `name` is the
/// table's title-equivalent.
fn load_live_curated_entity(
    conn: &Connection,
    entity_id: &str,
    id: &str,
) -> Result<Option<WikiTraverseNode>> {
    // Test fixtures and older brains may carry only `llm_wiki_entries`. Treat
    // a missing table as "no row in this space" so entry-anchored databases
    // behave exactly as they did before heterogeneous traversal existed.
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'curated_entities'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(None);
    }
    let mut stmt =
        conn.prepare("SELECT id, name FROM curated_entities WHERE id = ?1 AND deleted_at IS NULL")?;
    let mut rows = stmt.query(rusqlite::params![id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(WikiTraverseNode {
        id: row.get(0)?,
        title: row.get(1)?,
        entity_id: entity_id.to_string(),
    }))
}

/// Resolve a seed or neighbor id in whichever endpoint space holds it.
/// Entry space is tried first so entry-anchored databases behave exactly as
/// they did before heterogeneous traversal existed.
fn load_live_node(
    conn: &Connection,
    entity_id: &str,
    id: &str,
) -> Result<Option<(WikiTraverseNode, NodeSpace)>> {
    if let Some(node) = load_live_entry(conn, entity_id, id)? {
        return Ok(Some((node, NodeSpace::Entry)));
    }
    if let Some(node) = load_live_curated_entity(conn, entity_id, id)? {
        return Ok(Some((node, NodeSpace::Entity)));
    }
    Ok(None)
}

fn fetch_neighbors(
    conn: &Connection,
    entity_id: &str,
    node_id: &str,
    direction: TraverseDirection,
    edge_types: &[&str],
    space: NodeSpace,
) -> Result<Vec<(WikiTraverseEdge, String)>> {
    let edge_filter = if edge_types.is_empty() {
        String::new()
    } else {
        let ph: String = edge_types.iter().map(|_| "?,").collect();
        format!(" AND e.edge_type IN ({})", ph.trim_end_matches(','))
    };

    let mut out = Vec::new();
    let want_outbound = matches!(
        direction,
        TraverseDirection::Outbound | TraverseDirection::Both
    );
    let want_inbound = matches!(
        direction,
        TraverseDirection::Inbound | TraverseDirection::Both
    );

    match space {
        NodeSpace::Entry => {
            if want_outbound {
                fetch_outbound_neighbors(
                    conn,
                    entity_id,
                    node_id,
                    &edge_filter,
                    edge_types,
                    &mut out,
                )?;
            }
            if want_inbound {
                fetch_inbound_neighbors(
                    conn,
                    entity_id,
                    node_id,
                    &edge_filter,
                    edge_types,
                    &mut out,
                )?;
            }
        }
        NodeSpace::Entity => {
            if want_outbound {
                fetch_entity_neighbors(
                    conn,
                    entity_id,
                    node_id,
                    &edge_filter,
                    edge_types,
                    false,
                    &mut out,
                )?;
            }
            if want_inbound {
                fetch_entity_neighbors(
                    conn,
                    entity_id,
                    node_id,
                    &edge_filter,
                    edge_types,
                    true,
                    &mut out,
                )?;
            }
        }
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
         JOIN llm_wiki_entries s ON s.id = e.source_id AND s.deleted_at IS NULL AND s.entity_id = ?1
         JOIN llm_wiki_entries t ON t.id = e.target_id AND t.deleted_at IS NULL AND t.entity_id = ?1
         WHERE e.entity_id = ?1 AND e.source_id = ?2{edge_filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(entity_id.to_string()),
        Box::new(node_id.to_string()),
    ];
    for et in edge_types {
        params.push(Box::new(et.to_string()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;
    while let Some(row) = rows.next()? {
        let edge = WikiTraverseEdge {
            source_id: row.get(0)?,
            target_id: row.get(1)?,
            edge_type: row.get(2)?,
            entity_id: entity_id.to_string(),
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
         JOIN llm_wiki_entries s ON s.id = e.source_id AND s.deleted_at IS NULL AND s.entity_id = ?1
         JOIN llm_wiki_entries t ON t.id = e.target_id AND t.deleted_at IS NULL AND t.entity_id = ?1
         WHERE e.entity_id = ?1 AND e.target_id = ?2{edge_filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(entity_id.to_string()),
        Box::new(node_id.to_string()),
    ];
    for et in edge_types {
        params.push(Box::new(et.to_string()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;
    while let Some(row) = rows.next()? {
        let edge = WikiTraverseEdge {
            source_id: row.get(0)?,
            target_id: row.get(1)?,
            edge_type: row.get(2)?,
            entity_id: entity_id.to_string(),
        };
        let neighbor_id: String = row.get(3)?;
        out.push((edge, neighbor_id));
    }
    Ok(())
}

/// Entity-space neighbor fetch. Both endpoints are resolved in
/// `curated_entities`; the entity partition comes from the edge row because
/// `curated_entities` carries no `entity_id` column.
fn fetch_entity_neighbors(
    conn: &Connection,
    entity_id: &str,
    node_id: &str,
    edge_filter: &str,
    edge_types: &[&str],
    inbound: bool,
    out: &mut Vec<(WikiTraverseEdge, String)>,
) -> Result<()> {
    let (neighbor_alias, anchor_col) = if inbound {
        ("s", "e.target_id")
    } else {
        ("t", "e.source_id")
    };
    let sql = format!(
        "SELECT e.source_id, e.target_id, e.edge_type, {neighbor_alias}.id
         FROM llm_wiki_edges e
         JOIN curated_entities s ON s.id = e.source_id AND s.deleted_at IS NULL
         JOIN curated_entities t ON t.id = e.target_id AND t.deleted_at IS NULL
         WHERE e.entity_id = ?1 AND {anchor_col} = ?2{edge_filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(entity_id.to_string()),
        Box::new(node_id.to_string()),
    ];
    for et in edge_types {
        params.push(Box::new(et.to_string()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;
    while let Some(row) = rows.next()? {
        let edge = WikiTraverseEdge {
            source_id: row.get(0)?,
            target_id: row.get(1)?,
            edge_type: row.get(2)?,
            entity_id: entity_id.to_string(),
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
    let Some((seed, space)) = load_live_node(conn, entity_id, source_id)? else {
        return Ok(WikiTraverseResult {
            nodes: Vec::new(),
            edges: Vec::new(),
            truncated: false,
        });
    };

    // Mirrors the read-side gate in `CompositeWalk::walk_seed`: a strict
    // ontology on `entity_id` keeps off-manifest edges out of the traversal
    // exactly the way the writer keeps them out of new commits (spec §2.3,
    // #158). Resolved once up-front rather than per-hop.
    let edge_vocabulary =
        crate::db::commit::resolve_strict_edge_vocabulary(conn, entity_id);

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
        let mut pairs =
            fetch_neighbors(conn, entity_id, &current_id, direction, edge_types, space)?;
        if let Some(vocab) = &edge_vocabulary {
            pairs.retain(|(edge, _)| vocab.contains(&edge.edge_type.trim().to_lowercase()));
        }
        for (edge, neighbor_id) in pairs {
            let is_new_neighbor = !visited.contains(&neighbor_id);
            if is_new_neighbor && nodes.len() >= MAX_TRAVERSAL_NODES {
                truncated = true;
                break;
            }
            let key = (
                edge.source_id.clone(),
                edge.target_id.clone(),
                edge.edge_type.clone(),
            );
            if edge_keys.insert(key) {
                edges.push(edge);
            }
            if is_new_neighbor {
                // Resolve in the walk's own space. `load_live_node` tries entry
                // space first, which would hand back an `llm_wiki_entries` title
                // for a `curated_entities` neighbor whose id also exists there
                // under this partition. A walk must not cross spaces at the
                // neighbor boundary either (spec section 3).
                let resolved = match space {
                    NodeSpace::Entry => load_live_entry(conn, entity_id, &neighbor_id)?,
                    NodeSpace::Entity => load_live_curated_entity(conn, entity_id, &neighbor_id)?,
                };
                if let Some(node) = resolved {
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
    use crate::db::connection::open_in_memory;
    use rusqlite::params;

    fn seed_entry(conn: &Connection, entity_id: &str, id: &str) {
        // Eight non-zero floats packed as 32 bytes — long enough for
        // `wiki_search`'s `dim * 4` length check (matches the `[0.0; 8]`
        // query vector the new tests use), and non-zero so a fact with
        // that embedding actually clears the `raw > 0` ranking filter.
        // The values are arbitrary as long as they are not all zero —
        // they only need to surface the row to `wiki_context`.
        let embedding: [f32; 8] = [1.0; 8];
        let embedding_blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, ?1, 'Body', '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?3, NULL)",
            params![id, entity_id, embedding_blob],
        )
        .unwrap();
    }

    fn seed_edge(
        conn: &Connection,
        entity_id: &str,
        source: &str,
        target: &str,
        edge_type: &str,
    ) {
        // Deterministic per-call id keeps tests diff-friendly. The primary-key
        // collision we have to dodge is between two calls within one test, not
        // across tests, so a counter on a static would also work — this just
        // keeps every row identifiable in `llm_wiki_edges`.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("edge-{n}");
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 100)",
            params![id, entity_id, source, target, edge_type],
        )
        .unwrap();
    }

    /// Install a strict-mode ontology manifest declaring `edge_types` (with no
    /// source/target typing). Mirrors `seed_manifest` in `db::commit::tests`
    /// trimmed to the §2.3 / #158 case: the read path only needs to know
    /// which edge-type *names* are in scope.
    fn seed_strict_ontology(conn: &Connection, entity_id: &str, edge_types: &[&str]) {
        let edges: Vec<serde_json::Value> = edge_types
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": t,
                    "source_type": "",
                    "target_type": "",
                    "description": "",
                })
            })
            .collect();
        let manifest =
            serde_json::json!({ "node_types": [], "edge_types": edges }).to_string();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES (?1, 'strict', ?2, 0)",
            params![entity_id, manifest],
        )
        .unwrap();
    }

    /// Characterization test — this behavior already exists and must not regress.
    /// A ghost edge (endpoint soft-deleted) must be invisible in BOTH directions.
    #[test]
    fn traversal_excludes_edges_whose_endpoint_is_soft_deleted() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "ent-1", "fact_live");
        seed_entry(&conn, "ent-1", "fact_other");
        // Soft-delete the ghost endpoint without touching the live helpers —
        // the seed path stays live-only, and this test still covers the
        // soft-delete code path inside the traversal.
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES ('fact_ghost', 'ent-1', 'fact_ghost', 'Body', '[]', 'inferred',
                       'librarian_inferred', NULL, NULL, 100, 100, NULL, 0, 200000,
                       NULL, NULL)",
            [],
        )
        .unwrap();

        // Outbound from the live node into a dead target.
        seed_edge(&conn, "ent-1", "fact_live", "fact_ghost", "related_to");
        // Inbound into the live node from a dead source.
        seed_edge(&conn, "ent-1", "fact_ghost", "fact_live", "related_to");
        // A wholly live edge, as the positive control.
        seed_edge(&conn, "ent-1", "fact_live", "fact_other", "related_to");

        let result =
            wiki_traverse_graph(&conn, "ent-1", "fact_live", 2, TraverseDirection::Both, &[])
                .unwrap();

        let edge_targets: Vec<&str> = result.edges.iter().map(|e| e.target_id.as_str()).collect();
        assert!(
            !edge_targets.contains(&"fact_ghost"),
            "an edge into a soft-deleted target must not surface"
        );
        let edge_sources: Vec<&str> = result.edges.iter().map(|e| e.source_id.as_str()).collect();
        assert!(
            !edge_sources.contains(&"fact_ghost"),
            "an edge from a soft-deleted source must not surface"
        );
        assert!(
            edge_targets.contains(&"fact_other"),
            "the wholly-live edge is the positive control and must surface"
        );
    }

    /// A strict-mode brain must surface only the edge types its manifest
    /// declares — even when an off-manifest row was grandfathered into
    /// `llm_wiki_edges` before the manifest existed (spec §2.3, #158).
    ///
    /// `wiki_context` walks from `fact.entity_id`, which only resolves when
    /// that id is itself a node; the plan's seed shape (entries A/B in
    /// `ent_demo`, edges between A and B) leaves `ent_demo` unanchored, so
    /// `wiki_context`'s BFS finds no neighbours. Going through
    /// `wiki_traverse_graph` with `A` as the seed reaches both edges and
    /// exercises the same vocabulary gate that `CompositeWalk::walk_seed`
    /// applies for `wiki_context`.
    #[test]
    fn wiki_context_hides_off_manifest_edges_in_strict_mode() {
        let conn = open_in_memory().unwrap();
        seed_strict_ontology(&conn, "ent_demo", &["depends_on"]);
        let a = "A".to_string();
        let b = "B".to_string();
        seed_entry(&conn, "ent_demo", &a);
        seed_entry(&conn, "ent_demo", &b);
        seed_edge(&conn, "ent_demo", &a, &b, "depends_on");
        seed_edge(
            &conn,
            "ent_demo",
            &a,
            &b,
            "has_open_bug_reported_2026-09-09",
        );

        let result =
            wiki_traverse_graph(&conn, "ent_demo", &a, 2, TraverseDirection::Both, &[])
                .unwrap();

        let types: Vec<&str> = result.edges.iter().map(|e| e.edge_type.as_str()).collect();
        assert!(
            types.contains(&"depends_on"),
            "manifest edge must survive, got: {types:?}"
        );
        assert!(
            !types.iter().any(|t| t.contains("2026-09-09")),
            "off-manifest edge must be filtered, got: {types:?}"
        );
    }

    /// Without an ontology (or with one that is not `strict`), the resolver
    /// returns `None` and the read path leaves every row in place. This
    /// uses `wiki_traverse_graph` for the same reason as
    /// `wiki_context_hides_off_manifest_edges_in_strict_mode`.
    #[test]
    fn non_strict_ontology_is_not_filtered() {
        let conn = open_in_memory().unwrap();
        let a = "A".to_string();
        let b = "B".to_string();
        seed_entry(&conn, "ent_open", &a);
        seed_entry(&conn, "ent_open", &b);
        seed_edge(&conn, "ent_open", &a, &b, "anything_goes_here");

        let result =
            wiki_traverse_graph(&conn, "ent_open", &a, 2, TraverseDirection::Both, &[])
                .unwrap();
        let types: Vec<&str> = result.edges.iter().map(|e| e.edge_type.as_str()).collect();
        assert!(
            types.contains(&"anything_goes_here"),
            "non-strict brains must be unfiltered, got: {types:?}"
        );
    }

    #[test]
    fn tier_weight_matches_tiered_read() {
        assert_eq!(tier_weight("tier_fact"), 1.5);
        assert_eq!(tier_weight("tier_wisdom"), 1.0);
        assert_eq!(tier_weight("tier_working::abc"), 0.6);
        assert_eq!(tier_weight("custom_entity"), 1.0);
    }

    #[test]
    fn wiki_ontology_result_serializes_manifest_null() {
        let result = WikiOntologyResult {
            mode: "off".into(),
            manifest: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"manifest\":null"));
    }
}

// ---------------------------------------------------------------------------
// wiki_context — the composite context primitive (spec §4)
// ---------------------------------------------------------------------------

/// Facts returned when the caller does not say otherwise (spec §4.1).
pub const DEFAULT_CONTEXT_MAX_FACTS: usize = 5;
/// Traversal depth when the caller does not say otherwise (spec §4.1).
pub const DEFAULT_CONTEXT_DEPTH: usize = 1;
/// Upper bound on `max_facts`. Each fact is a traversal seed, so this bounds
/// seed fan-out; the node cap still bounds the walk itself.
pub const MAX_CONTEXT_FACTS: usize = 25;

/// Where one fact came from: the document and chunk backing it, plus the score
/// that selected it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiContextProvenance {
    pub fact_id: String,
    pub entity_id: String,
    pub score: f32,
    pub tier: Option<String>,
    pub sources: Vec<WikiContextSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiContextSource {
    pub doc_path: String,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiContextResult {
    pub facts: Vec<WikiSearchHit>,
    pub entities: Vec<WikiTraverseNode>,
    pub edges: Vec<WikiTraverseEdge>,
    pub provenance: Vec<WikiContextProvenance>,
    /// True when the node cap or the depth clamp cut the walk short, so a
    /// caller can tell a complete neighborhood from a partial one.
    pub truncated: bool,
}

/// BFS state shared across every seed of one `wiki_context` call.
///
/// The node cap is deliberately applied across the **whole** composite walk
/// rather than per seed: a call fans out from up to `max_facts` seeds, so a
/// per-seed cap would put the effective ceiling at `max_facts × 50` (spec
/// §4.1).
///
/// Nodes and the visited set are keyed by `(entity_id, node_id)`. Seeds can sit
/// in different partitions, and the same id may legitimately exist in more than
/// one — keying by id alone would let the first partition's node mask the
/// other's. The edge deduplication key carries `entity_id` for the same
/// reason: an edge is owned by the partition it was written under, and two
/// partitions can hold the same `(source_id, target_id, edge_type)` without
/// the two being the same edge.
#[derive(Default)]
struct CompositeWalk {
    nodes: HashMap<(String, String), WikiTraverseNode>,
    edges: Vec<WikiTraverseEdge>,
    edge_keys: HashSet<(String, String, String, String)>,
    visited: HashSet<(String, String)>,
    truncated: bool,
    /// Lowercased strict-ontology edge vocabulary, resolved once per
    /// traversal. `None` means "do not gate" — the ontology is absent,
    /// non-strict, unreadable, or declares no edge types. Mirrors the
    /// write-time gate so reads and writes agree on what is legal.
    edge_vocabulary: Option<std::collections::HashSet<String>>,
}

impl CompositeWalk {
    fn at_capacity(&self) -> bool {
        self.nodes.len() >= MAX_TRAVERSAL_NODES
    }

    /// Walk outward from one seed, folding results into the shared state.
    ///
    /// A seed that resolves in neither endpoint space is skipped rather than
    /// treated as an error: an entity-less fact legitimately contributes no
    /// edges, and the call still returns its facts (PR #78 graceful
    /// degradation, spec §4.1).
    fn walk_seed(
        &mut self,
        conn: &Connection,
        entity_id: &str,
        source_id: &str,
        max_depth: usize,
        direction: TraverseDirection,
        edge_types: &[&str],
    ) -> Result<()> {
        if self.at_capacity() {
            self.truncated = true;
            return Ok(());
        }
        let Some((seed, space)) = load_live_node(conn, entity_id, source_id)? else {
            return Ok(());
        };

        let seed_key = (entity_id.to_string(), seed.id.clone());
        if self.visited.insert(seed_key.clone()) {
            self.nodes.insert(seed_key, seed);
        }

        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((source_id.to_string(), 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let mut pairs =
                fetch_neighbors(conn, entity_id, &current_id, direction, edge_types, space)?;

            // Strict ontologies gate reads the same way they gate writes. The
            // write-time gate is deliberately non-retroactive (see
            // `strict_mode_grandfathers_edges_written_before_the_manifest`), so
            // rows written before the manifest existed are still in the table
            // and would otherwise surface as first-class neighbourhood results.
            if let Some(vocab) = &self.edge_vocabulary {
                pairs.retain(|(edge, _)| vocab.contains(&edge.edge_type.trim().to_lowercase()));
            }
            for (edge, neighbor_id) in pairs {
                let neighbor_key = (entity_id.to_string(), neighbor_id.clone());
                let is_new_neighbor = !self.visited.contains(&neighbor_key);
                if is_new_neighbor && self.at_capacity() {
                    self.truncated = true;
                    return Ok(());
                }
                // Edge partition is part of the key: two seeded partitions
                // can hold the same (source_id, target_id, edge_type) and
                // they are different edges. The entity_id also lands on the
                // returned WikiTraverseEdge so consumers can route it back
                // to its partition.
                let edge_with_partition = WikiTraverseEdge {
                    entity_id: entity_id.to_string(),
                    ..edge
                };
                let key = (
                    edge_with_partition.source_id.clone(),
                    edge_with_partition.target_id.clone(),
                    edge_with_partition.edge_type.clone(),
                    edge_with_partition.entity_id.clone(),
                );
                if self.edge_keys.insert(key) {
                    self.edges.push(edge_with_partition);
                }
                if is_new_neighbor {
                    // Resolve in the walk's own space, never across it — the
                    // #134 neighbor-boundary contract.
                    let resolved = match space {
                        NodeSpace::Entry => load_live_entry(conn, entity_id, &neighbor_id)?,
                        NodeSpace::Entity => {
                            load_live_curated_entity(conn, entity_id, &neighbor_id)?
                        }
                    };
                    if let Some(node) = resolved {
                        self.visited.insert(neighbor_key.clone());
                        self.nodes.insert(neighbor_key, node);
                        queue.push_back((neighbor_id, depth + 1));
                    }
                }
            }
        }
        Ok(())
    }
}

/// The provenance record for one search hit.
fn provenance_for(conn: &Connection, hit: &WikiSearchHit) -> Result<WikiContextProvenance> {
    let source_ref: Option<String> = conn
        .query_row(
            "SELECT source_ref FROM llm_wiki_entries WHERE id = ?1",
            [&hit.id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let sources = crate::db::entities::source_docs_from_ref(conn, source_ref.as_deref())
        .into_iter()
        .map(|(doc_path, content_hash)| WikiContextSource {
            doc_path,
            content_hash,
        })
        .collect();
    Ok(WikiContextProvenance {
        fact_id: hit.id.clone(),
        entity_id: hit.entity_id.clone(),
        score: hit.score,
        tier: hit.tier.clone(),
        sources,
    })
}

/// One-call retrieval: search, then walk the neighborhood around what was
/// found (spec §4.1).
///
/// The caller needs zero namespace knowledge — no id-prefix distinction, no
/// entry-vs-entity seeding decision, no bridge mechanics. `wiki_search`
/// returns each fact's `entity_id`, which is also a `curated_entities` id, so
/// it serves as both the edge partition and the traversal seed; resolution
/// order is #134's (entry space first, then entity space).
///
/// This is a **composition over existing primitives, not a new contract**: the
/// traversal limits are `clamp_max_depth` (1..3), `MAX_TRAVERSAL_NODES`, the
/// BFS visited set and edge-key deduplication, all inherited verbatim. A
/// `depth` above the ceiling is clamped, never rejected.
///
/// Every leg degrades gracefully. A query matching nothing returns empty lists;
/// facts whose entities carry no live relationships return `edges: []`. Neither
/// is an error, and neither falls back to prose.
pub fn wiki_context(
    conn: &Connection,
    query_vec: &[f32],
    tier: Option<&str>,
    depth: usize,
    max_facts: usize,
) -> Result<WikiContextResult> {
    let max_facts = max_facts.clamp(1, MAX_CONTEXT_FACTS);
    let clamped_depth = clamp_max_depth(depth);

    // The default all-live-entries contract from #133 is preserved: entity_ids
    // stays `None`, and `tier` narrows only when the caller asked it to.
    let facts = wiki_search(conn, query_vec, None, tier, max_facts)?;

    // Resolve the strict vocabulary once for the whole walk rather than per
    // hop. All seeds in one `wiki_context` call share the caller's brain, so
    // the first seed's entity is representative; `None` disables gating.
    let edge_vocabulary = facts
        .first()
        .and_then(|f| crate::db::commit::resolve_strict_edge_vocabulary(conn, &f.entity_id));

    let mut walk = CompositeWalk {
        // `truncated` means the walk is **narrower** than the caller asked
        // for, so the neighborhood may be incomplete. A clamped depth that
        // is *wider* than the caller asked for (e.g. `depth: 0` clamps to
        // 1) is not truncation — the walk returned more than requested, and
        // the caller does not have a partial-neighbourhood hazard to
        // reason about. Report truncation only when the cap shrank the
        // walk, which happens inside `walk_seed` once the node budget is
        // exhausted.
        truncated: clamped_depth < depth,
        edge_vocabulary,
        ..Default::default()
    };

    // Seeds are deduplicated: several facts commonly share one entity, and
    // re-walking it would spend the shared node budget on work already done.
    let mut seen_seeds: HashSet<&str> = HashSet::new();
    for fact in &facts {
        if !seen_seeds.insert(fact.entity_id.as_str()) {
            continue;
        }
        walk.walk_seed(
            conn,
            &fact.entity_id,
            &fact.entity_id,
            clamped_depth,
            TraverseDirection::Both,
            &[],
        )?;
    }

    let mut entities: Vec<WikiTraverseNode> = walk.nodes.into_values().collect();
    entities.sort_by(|a, b| a.entity_id.cmp(&b.entity_id).then(a.id.cmp(&b.id)));

    let provenance = facts
        .iter()
        .map(|hit| provenance_for(conn, hit))
        .collect::<Result<Vec<_>>>()?;

    Ok(WikiContextResult {
        facts,
        entities,
        edges: walk.edges,
        provenance,
        truncated: walk.truncated,
    })
}
