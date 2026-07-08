//! Timeline event queries — union of llm_wiki_events, curated_agent_log, and documents.

use rusqlite::{params, Connection, named_params};
use serde::Serialize;

use crate::db::commit::now_timestamps;

#[derive(Debug, Clone, Serialize)]
pub struct TimelineEvent {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub entity_id: Option<String>,
    pub entity_name: Option<String>,
    pub doc_path: Option<String>,
    /// For `agent_access` kind: the agent client name.
    /// For `ingested` kind: the document status (e.g. "indexed").
    /// For other kinds: NULL.
    pub raw_type: String,
    pub client: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineFilter {
    pub entity_id: Option<String>,
    pub kinds: Option<Vec<String>>,
    pub before_ms: Option<i64>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: Option<usize>,
}

/// List timeline events, applying kind filter in SQL before LIMIT.
pub fn list_events(conn: &Connection, filter: &TimelineFilter) -> rusqlite::Result<Vec<TimelineEvent>> {
    let limit = filter.limit.unwrap_or(100).min(500) as i64;

    // Build dynamic kind IN clause if filter.kinds is set
    let (kind_where, kind_params): (String, Vec<String>) = if let Some(ref kinds) = filter.kinds {
        let placeholders: Vec<String> = kinds.iter().enumerate().map(|(i, _)| format!(":kind_{i}")).collect();
        let clause = format!("AND kind IN ({})", placeholders.join(", "));
        (clause, kinds.clone())
    } else {
        ("".into(), vec![])
    };

    let sql = format!(
        r#"
        WITH all_events AS (
            -- llm_wiki_events (timestamps in ms, may be in seconds)
            SELECT
                e.id,
                e.event_type AS kind,
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
        SELECT id, kind, summary, entity_id, entity_name, doc_path, raw_type, client, created_at_ms
        FROM all_events
        WHERE
            (:entity_id IS NULL OR entity_id = :entity_id)
            AND (:before_ms IS NULL OR created_at_ms < :before_ms)
            AND (:since_ms IS NULL OR created_at_ms >= :since_ms)
            AND (:until_ms IS NULL OR created_at_ms <= :until_ms)
            {kind_where}
        ORDER BY created_at_ms DESC
        LIMIT :limit
        "#,
        kind_where = kind_where
    );

    let mut stmt = conn.prepare(&sql)?;

    // Build named params
    let mut named = Vec::new();
    named.push((":entity_id", filter.entity_id.as_deref()));
    named.push((":before_ms", filter.before_ms));
    named.push((":since_ms", filter.since_ms));
    named.push((":until_ms", filter.until_ms));
    named.push((":limit", Some(limit)));
    for (i, k) in kind_params.iter().enumerate() {
        named.push((Box::leak(format!(":kind_{i}").into_boxed_str()), Some(k.as_str())));
    }

    // We'll use a helper to bind named params dynamically.
    // Since rusqlite's named_params! macro is compile-time, we'll fall back to positional binding
    // but ensure the order matches the SQL. Safer: use a loop with stmt.raw_bind_named.
    // For simplicity, we'll use positional binding with the same order as the SQL placeholders.
    // The SQL placeholders appear in order: :entity_id, :before_ms, :since_ms, :until_ms, :limit, then :kind_0, :kind_1, ...
    // We'll bind them positionally.
    let mut param_values: Vec<&dyn rusqlite::ToSql> = Vec::new();
    param_values.push(&filter.entity_id);
    param_values.push(&filter.before_ms);
    param_values.push(&filter.since_ms);
    param_values.push(&filter.until_ms);
    param_values.push(&limit);
    for k in &kind_params {
        param_values.push(k);
    }

    let rows = stmt.query_map(param_values.as_slice(), |r| {
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
    })?;

    let mut events = Vec::new();
    for row in rows {
        let (id, kind, summary, entity_id, entity_name, doc_path, raw_type, client, created_at_ms) = row?;
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
