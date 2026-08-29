//! Active librarian: after each successful ingest, optionally synthesizes OKF proposals from **current**
//! chunks in SQLite. If chunking strategy (`ast_*`, prose, …) or embeddings change, rebuild chunks with
//! **`bulk_reindex`** (CLI) or the **`queue_full_reindex`** Tauri command (`force_rechunk: true`) before
//! relying on refreshed summaries.

mod synthesis;

use crate::db::queries::get_document_by_path;
use crate::librarian::synthesis::{run_synthesis, SynthesisMode};
use anyhow::{Context, Result};
use rusqlite::Connection;

pub use synthesis::active_generation_model;

pub struct ChunkRow {
    pub id: i64,
    pub entity_id: String,
    pub text: String,
    pub symbol_name: Option<String>,
    pub start_line: i64,
    pub end_line: i64,
    pub tier: String,
    pub path: String,
}

pub fn assemble_librarian_context(chunks: &[ChunkRow]) -> String {
    let mut body = String::new();

    for chunk in chunks {
        let label_key = if chunk.entity_id.is_empty() {
            match chunk.tier.as_str() {
                "user_doc" => "tier_fact",
                _ => "",
            }
        } else {
            chunk.entity_id.as_str()
        };

        let tier_label = match label_key {
            "tier_fact" => "ANCHOR TRUTH — do not propose modifications to these facts:\n",
            "tier_wisdom" => "CURATED WISDOM — may be updated via Wisdom proposals:\n",
            _ => "WORKING CONTEXT — summarize patterns and flag contradictions only:\n",
        };

        let header = match &chunk.symbol_name {
            Some(sym) => format!(
                "[source: {} | symbol: {} | lines {}-{}]\n",
                chunk.path, sym, chunk.start_line, chunk.end_line
            ),
            None => format!(
                "[source: {} | lines {}-{}]\n",
                chunk.path, chunk.start_line, chunk.end_line
            ),
        };

        body.push_str(tier_label);
        body.push_str(&header);
        body.push_str(&chunk.text);
        body.push_str("\n\n");
    }

    body
}

struct StructuralNeighbor {
    chunk_text: String,
    symbol_name: Option<String>,
    path: String,
    rel_type: String,
    depth: i64,
}

pub(crate) fn build_structural_context(conn: &Connection, source_chunks: &[ChunkRow]) -> String {
    let source_ids: Vec<i64> = source_chunks.iter().map(|c| c.id).collect();
    if source_ids.is_empty() {
        return String::new();
    }

    let entity_id = source_chunks
        .first()
        .map(|c| c.entity_id.as_str())
        .unwrap_or("");
    if entity_id.is_empty() {
        return String::new();
    }

    let mut neighbor_map: std::collections::HashMap<i64, (String, i64)> =
        std::collections::HashMap::new();
    for chunk_id in &source_ids {
        if let Ok(neighbors) = crate::graph::get_both(conn, *chunk_id, entity_id, 1) {
            for n in neighbors {
                let entry = neighbor_map
                    .entry(n.chunk_id)
                    .or_insert((n.rel_type.clone(), n.depth));
                if n.depth < entry.1 {
                    *entry = (n.rel_type, n.depth);
                }
            }
        }
    }

    let source_id_set: std::collections::HashSet<i64> = source_ids.into_iter().collect();
    neighbor_map.retain(|id, _| !source_id_set.contains(id));

    if neighbor_map.is_empty() {
        return String::new();
    }

    let mut sorted_ids: Vec<(i64, String, i64)> = neighbor_map
        .into_iter()
        .map(|(id, (rel, depth))| (id, rel, depth))
        .collect();
    sorted_ids.sort_by_key(|(_, _, depth)| *depth);
    sorted_ids.truncate(5);

    let mut neighbors: Vec<StructuralNeighbor> = Vec::new();
    for (chunk_id, rel_type, depth) in sorted_ids {
        let result = conn.query_row(
            "SELECT c.chunk_text, c.symbol_name, d.path
             FROM chunks c
             JOIN documents d ON d.id = c.doc_id
             WHERE c.id = ?1",
            rusqlite::params![chunk_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        );
        if let Ok((text, sym, path)) = result {
            neighbors.push(StructuralNeighbor {
                chunk_text: text,
                symbol_name: sym,
                path,
                rel_type,
                depth,
            });
        }
    }

    if neighbors.is_empty() {
        return String::new();
    }

    let mut section = String::from(
        "STRUCTURAL CONTEXT — linked via call graph (do not modify; use for impact analysis only):\n"
    );
    for n in &neighbors {
        let header = match &n.symbol_name {
            Some(sym) => format!(
                "[source: {} | symbol: {} | rel: {} | depth: {}]\n",
                n.path, sym, n.rel_type, n.depth
            ),
            None => format!(
                "[source: {} | rel: {} | depth: {}]\n",
                n.path, n.rel_type, n.depth
            ),
        };
        section.push_str(&header);
        section.push_str(&n.chunk_text);
        section.push_str("\n\n");
    }

    section
}

fn get_folder_mode(conn: &Connection, source_path: &str) -> (String, bool) {
    let mut p = std::path::Path::new(source_path);
    loop {
        let dir = p.parent().unwrap_or(p).to_string_lossy().to_string();
        let result = conn.query_row(
            "SELECT librarian_mode, auto_approve FROM folder_rules WHERE folder_path = ?1",
            [&dir],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
        );
        if let Ok((mode, auto)) = result {
            return (mode, auto);
        }
        match p.parent() {
            Some(parent) if parent != p => p = parent,
            _ => break,
        }
    }
    ("summarize".to_string(), false)
}

/// Watermark gate: a document is "clean" when its last successful synthesis
/// ran over the current content hash with the current active model. Such a
/// doc is skipped by synthesis (unless `--force`). Deliberately ignores
/// `ingest_runs` — the Task 1 backfill marks pre-watermark indexed docs as
/// clean via `synth_model = 'pre-watermark'`, which never matches an active
/// model name, so those stay dirty until re-synthesized.
pub fn is_doc_clean(
    synth_hash: Option<&str>,
    synth_model: Option<&str>,
    doc_hash: &str,
    active_model: &str,
) -> bool {
    synth_hash == Some(doc_hash) && synth_model == Some(active_model)
}

pub fn generate_summary(
    conn: &mut Connection,
    source_path: &str,
    model: &str,
    force: bool,
) -> Result<()> {
    let (mode, auto_approve) = get_folder_mode(conn, source_path);

    if mode == "index" {
        return Ok(());
    }

    let synthesis_mode = match mode.as_str() {
        "synthesize" => SynthesisMode::Synthesize,
        _ => SynthesisMode::Summarize,
    };

    let chunks: Vec<ChunkRow> = {
        let mut stmt = conn.prepare("PRAGMA table_info(chunks)")?;
        let mut rows = stmt.query([])?;
        let mut column_names = Vec::new();
        while let Some(row) = rows.next()? {
            column_names.push(row.get::<_, String>(1)?);
        }
        let has_extended_columns = column_names.iter().any(|name| name == "symbol_name")
            && column_names.iter().any(|name| name == "start_line")
            && column_names.iter().any(|name| name == "end_line");

        let mut v = Vec::new();
        if has_extended_columns {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.entity_id, c.chunk_text, c.symbol_name, c.start_line, c.end_line, d.tier, d.path
                 FROM chunks c
                 JOIN documents d ON d.id = c.doc_id
                 WHERE d.path = ?1
                 ORDER BY c.position",
            )?;
            let mut rows = stmt.query([source_path])?;
            while let Some(row) = rows.next()? {
                v.push(ChunkRow {
                    id: row.get(0)?,
                    entity_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    text: row.get(2)?,
                    symbol_name: row.get(3)?,
                    start_line: row.get(4)?,
                    end_line: row.get(5)?,
                    tier: row.get(6)?,
                    path: row.get(7)?,
                });
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.entity_id, c.chunk_text, d.tier, d.path
                 FROM chunks c
                 JOIN documents d ON d.id = c.doc_id
                 WHERE d.path = ?1
                 ORDER BY c.position",
            )?;
            let mut rows = stmt.query([source_path])?;
            while let Some(row) = rows.next()? {
                v.push(ChunkRow {
                    id: row.get(0)?,
                    entity_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    text: row.get(2)?,
                    symbol_name: None,
                    start_line: 1,
                    end_line: 1,
                    tier: row.get(3)?,
                    path: row.get(4)?,
                });
            }
        }
        v
    };

    if chunks.is_empty() {
        return Ok(());
    }

    let trigger_doc_id = get_document_by_path(conn, source_path)?
        .map(|d| d.id)
        .context("source document not found after ingest")?;

    let vault_root = std::path::Path::new(source_path)
        .parent()
        .and_then(|p| p.parent());

    run_synthesis(
        conn,
        source_path,
        &chunks,
        trigger_doc_id,
        model,
        synthesis_mode,
        auto_approve,
        vault_root,
        force,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    #[test]
    fn test_generate_summary_skips_when_no_chunks() {
        let mut conn = open_in_memory().unwrap();
        let result = generate_summary(
            &mut conn,
            "/vault/documents/nonexistent.md",
            "llama3.2:1b",
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_index_mode_skips_librarian() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES ('/vault/documents', 'index', 0)",
            [],
        ).unwrap();
        let result = generate_summary(&mut conn, "/vault/documents/note.md", "llama3.2:1b", false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_folder_mode_defaults_to_summarize() {
        let conn = open_in_memory().unwrap();
        let (mode, auto) = get_folder_mode(&conn, "/vault/documents/note.md");
        assert_eq!(mode, "summarize");
        assert!(!auto);
    }

    #[test]
    fn test_is_doc_clean() {
        // Clean: watermark matches both content hash and active model.
        assert!(is_doc_clean(Some("h1"), Some("m1"), "h1", "m1"));
        // Content changed since last synthesis.
        assert!(!is_doc_clean(Some("old"), Some("m1"), "h1", "m1"));
        // Model changed since last synthesis.
        assert!(!is_doc_clean(Some("h1"), Some("old-model"), "h1", "m1"));
        // Never synthesized.
        assert!(!is_doc_clean(None, None, "h1", "m1"));
        // Task 1 backfill marks pre-watermark docs clean via
        // synth_model='pre-watermark', which never equals an active model.
        assert!(!is_doc_clean(
            Some("h1"),
            Some("pre-watermark"),
            "h1",
            "llama3.2:3b"
        ));
    }

    #[test]
    fn assemble_context_labels_user_doc_as_anchor_truth() {
        let chunks = vec![ChunkRow {
            id: 0,
            entity_id: String::new(),
            text: "fn init_db() {}".to_string(),
            symbol_name: Some("init_db".to_string()),
            start_line: 1,
            end_line: 3,
            tier: "user_doc".to_string(),
            path: "documents/sqlite_docs.md".to_string(),
        }];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("ANCHOR TRUTH"),
            "expected ANCHOR TRUTH label, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_labels_legacy_wiki_tier_as_working_context() {
        let chunks = vec![ChunkRow {
            id: 0,
            entity_id: String::new(),
            text: "Auth patterns overview".to_string(),
            symbol_name: None,
            start_line: 1,
            end_line: 10,
            tier: "wiki".to_string(),
            path: "wiki/auth-patterns.md".to_string(),
        }];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("WORKING CONTEXT"),
            "post-V7 wiki tier is not curated wisdom, got:\n{context}"
        );
        assert!(
            !context.contains("CURATED WISDOM"),
            "wiki tier must not map to CURATED WISDOM, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_includes_source_header_with_line_range() {
        let chunks = vec![ChunkRow {
            id: 0,
            entity_id: String::new(),
            text: "body text".to_string(),
            symbol_name: None,
            start_line: 12,
            end_line: 34,
            tier: "user_doc".to_string(),
            path: "documents/api-ref.md".to_string(),
        }];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("[source: documents/api-ref.md | lines 12-34]"),
            "expected source header, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_includes_symbol_name_when_present() {
        let chunks = vec![ChunkRow {
            id: 0,
            entity_id: String::new(),
            text: "fn foo() {}".to_string(),
            symbol_name: Some("foo".to_string()),
            start_line: 22,
            end_line: 45,
            tier: "wiki".to_string(),
            path: "src/db/init.rs".to_string(),
        }];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("[source: src/db/init.rs | symbol: foo | lines 22-45]"),
            "expected symbol in header, got:\n{context}"
        );
    }
}
