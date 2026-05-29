//! Active librarian: after each successful ingest, optionally generates wiki proposals from **current**
//! chunks in SQLite. If chunking strategy (`ast_*`, prose, …) or embeddings change, rebuild chunks with
//! **`bulk_reindex`** (CLI) or the **`queue_full_reindex`** Tauri command (`force_rechunk: true`) before
//! relying on refreshed summaries.

use anyhow::Result;
use crate::inference::config::{read_config, GenerationProviderKind};
use rusqlite::Connection;

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
                "wiki" => "tier_wisdom",
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

fn build_structural_context(conn: &Connection, source_chunks: &[ChunkRow]) -> String {
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

pub fn generate_summary(conn: &Connection, source_path: &str, model: &str) -> Result<()> {
    let (mode, auto_approve) = get_folder_mode(conn, source_path);

    if mode == "index" {
        return Ok(());
    }

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

    let source_text = assemble_librarian_context(&chunks);
    // Build structural context section
    let structural_text = build_structural_context(conn, &chunks);
    let full_text = if structural_text.is_empty() {
        source_text
    } else {
        format!("{}\n{}", source_text, structural_text)
    };
    let byte_limit = full_text
        .char_indices()
        .nth(4000)
        .map(|(i, _)| i)
        .unwrap_or(full_text.len());
    let truncated = &full_text[..byte_limit];

    let brain_dir_str = crate::get_brain_dir_inner();
    let brain_path = std::path::Path::new(&brain_dir_str);
    let llm_config = read_config(brain_path);
    let (endpoint_url, api_key, model_name) = match &llm_config.generation.provider {
        GenerationProviderKind::Unconfigured => {
            return Ok(());
        }
        GenerationProviderKind::Sidecar => {
            let base = std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let base = base.trim_end_matches('/');
            let chosen_model = llm_config
                .generation
                .model_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| {
                    llm_config.generation.model_path.as_deref().and_then(|path| {
                        std::path::Path::new(path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(|name| name.to_string())
                    })
                })
                .unwrap_or_else(|| model.to_string());
            (
                format!("{}/v1/chat/completions", base),
                None,
                chosen_model,
            )
        }
        GenerationProviderKind::External => {
            let base = llm_config.generation.external_url.clone().unwrap_or_default();
            let base = base.trim_end_matches('/');
            let base = base.strip_suffix("/v1").unwrap_or(base);
            (
                format!("{}/v1/chat/completions", base),
                llm_config.generation.api_key.clone(),
                llm_config
                    .generation
                    .model_name
                    .clone()
                    .unwrap_or_else(|| model.to_string()),
            )
        }
    };

    let system_prompt = "You are a knowledge librarian. Summarize the document into a concise wiki page in markdown format. Use headings and bullet points, keep under 400 words. Output only markdown.\n\nCONFLICT RESOLUTION DIRECTIVE: If Working Context contradicts Anchor Truth, do not harmonize or modify the Anchor Truth. Instead, create a new Wisdom entry titled 'Architectural Inconsistency' that states: which Working file and symbol introduced the deviation (cite source: metadata), which Anchor Truth document it violates (cite source: metadata), and a one-sentence description of the conflict. Do not emit a Wisdom proposal for any content that is consistent with the Anchor Truth.\n\nCASCADING VIOLATION DIRECTIVE: If a Structural Context chunk reveals that a violation in Working Context propagates to multiple callers, enumerate each caller file and symbol in the Wisdom proposal. Title the proposal 'Cascading Violation' and list each impacted call site under an 'Affected callers' section. Do not emit separate proposals per caller — consolidate into one.";

    let client = reqwest::blocking::Client::new();
    let mut req = client.post(&endpoint_url).json(&serde_json::json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": format!("Document to summarize:\n\n{}", truncated) }
        ],
        "stream": false,
    }));
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req.send()?;
    let body: serde_json::Value = resp.json()?;
    let wiki_content = body["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing content in /v1/chat/completions response"))?
        .to_string();

    let source_file = std::path::Path::new(source_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("summary.md")
        .to_string();

    let initial_status = if auto_approve {
        "approved"
    } else {
        "pending_review"
    };
    let source_ids = serde_json::json!([source_path]).to_string();

    conn.execute(
        "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET
           source_doc_ids = ?2,
           generated_by = ?3,
           status = ?4,
           last_synced = unixepoch()",
        rusqlite::params![source_file, source_ids, model, initial_status],
    )?;

    let proposed_dir = std::path::Path::new(source_path)
        .parent()
        .and_then(|p| p.parent())
        .map(|vault| vault.join(".brain").join("proposed"));

    if let Some(dir) = &proposed_dir {
        std::fs::create_dir_all(dir).ok();
        std::fs::write(dir.join(&source_file), &wiki_content).ok();
    }

    if auto_approve {
        if let Some(vault) = std::path::Path::new(source_path)
            .parent()
            .and_then(|p| p.parent())
        {
            let wiki_dir = vault.join("wiki");
            std::fs::create_dir_all(&wiki_dir).ok();
            std::fs::write(wiki_dir.join(&source_file), &wiki_content).ok();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    #[test]
    fn test_generate_summary_skips_when_no_chunks() {
        let conn = open_in_memory().unwrap();
        let result = generate_summary(&conn, "/vault/documents/nonexistent.md", "llama3.2:1b");
        assert!(result.is_ok());
    }

    #[test]
    fn test_index_mode_skips_librarian() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES ('/vault/documents', 'index', 0)",
            [],
        ).unwrap();
        let result = generate_summary(&conn, "/vault/documents/note.md", "llama3.2:1b");
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
    fn assemble_context_labels_wiki_as_curated_wisdom() {
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
            context.contains("CURATED WISDOM"),
            "expected CURATED WISDOM label, got:\n{context}"
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
