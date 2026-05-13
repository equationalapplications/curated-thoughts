//! Active librarian: after each successful ingest, optionally generates wiki proposals from **current**
//! chunks in SQLite. If chunking strategy (`ast_*`, prose, …) or embeddings change, rebuild chunks with
//! **`bulk_reindex`** (CLI) or the **`queue_full_reindex`** Tauri command (`force_rechunk: true`) before
//! relying on refreshed summaries.

use anyhow::Result;
use rusqlite::Connection;

pub struct ChunkRow {
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
        let tier_label = match chunk.tier.as_str() {
            "user_doc" => "ANCHOR TRUTH — do not propose modifications to these facts:\n",
            "wiki" => "CURATED WISDOM — may be updated via Wisdom proposals:\n",
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
                "SELECT c.chunk_text, c.symbol_name, c.start_line, c.end_line, d.tier, d.path
                 FROM chunks c
                 JOIN documents d ON d.id = c.doc_id
                 WHERE d.path = ?1
                 ORDER BY c.position",
            )?;
            let mut rows = stmt.query([source_path])?;
            while let Some(row) = rows.next()? {
                v.push(ChunkRow {
                    text: row.get(0)?,
                    symbol_name: row.get(1)?,
                    start_line: row.get(2)?,
                    end_line: row.get(3)?,
                    tier: row.get(4)?,
                    path: row.get(5)?,
                });
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT c.chunk_text, d.tier, d.path
                 FROM chunks c
                 JOIN documents d ON d.id = c.doc_id
                 WHERE d.path = ?1
                 ORDER BY c.position",
            )?;
            let mut rows = stmt.query([source_path])?;
            while let Some(row) = rows.next()? {
                v.push(ChunkRow {
                    text: row.get(0)?,
                    symbol_name: None,
                    start_line: 1,
                    end_line: 1,
                    tier: row.get(1)?,
                    path: row.get(2)?,
                });
            }
        }
        v
    };

    if chunks.is_empty() {
        return Ok(());
    }

    let source_text = assemble_librarian_context(&chunks);
    let byte_limit = source_text
        .char_indices()
        .nth(4000)
        .map(|(i, _)| i)
        .unwrap_or(source_text.len());
    let truncated = &source_text[..byte_limit];

    let client = reqwest::blocking::Client::new();
    let base_url =
        std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let resp = client
        .post(format!("{}/api/generate", base_url))
        .json(&serde_json::json!({
            "model": model,
            "system": "You are a knowledge librarian. Summarize the document into a concise wiki page in markdown format. Use headings and bullet points, keep under 400 words. Output only markdown.\n\nCONFLICT RESOLUTION DIRECTIVE: If Working Context contradicts Anchor Truth, do not harmonize or modify the Anchor Truth. Instead, create a new Wisdom entry titled 'Architectural Inconsistency' that states: which Working file and symbol introduced the deviation (cite source: metadata), which Anchor Truth document it violates (cite source: metadata), and a one-sentence description of the conflict. Do not emit a Wisdom proposal for any content that is consistent with the Anchor Truth.",
            "prompt": format!("Document to summarize:\n\n{}", truncated),
            "stream": false
        }))
        .send()?;

    let body: serde_json::Value = resp.json()?;
    let wiki_content = body["response"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing response from Ollama"))?
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
    use crate::db::schema::{MIGRATION_V1, MIGRATION_V2};

    #[test]
    fn test_generate_summary_skips_when_no_chunks() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        let result = generate_summary(&conn, "/vault/documents/nonexistent.md", "llama3.2:1b");
        assert!(result.is_ok());
    }

    #[test]
    fn test_index_mode_skips_librarian() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES ('/vault/documents', 'index', 0)",
            [],
        ).unwrap();
        let result = generate_summary(&conn, "/vault/documents/note.md", "llama3.2:1b");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_folder_mode_defaults_to_summarize() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        let (mode, auto) = get_folder_mode(&conn, "/vault/documents/note.md");
        assert_eq!(mode, "summarize");
        assert!(!auto);
    }

    #[test]
    fn assemble_context_labels_user_doc_as_anchor_truth() {
        let chunks = vec![
            ChunkRow {
                text: "fn init_db() {}".to_string(),
                symbol_name: Some("init_db".to_string()),
                start_line: 1,
                end_line: 3,
                tier: "user_doc".to_string(),
                path: "documents/sqlite_docs.md".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("ANCHOR TRUTH"),
            "expected ANCHOR TRUTH label, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_labels_wiki_as_curated_wisdom() {
        let chunks = vec![
            ChunkRow {
                text: "Auth patterns overview".to_string(),
                symbol_name: None,
                start_line: 1,
                end_line: 10,
                tier: "wiki".to_string(),
                path: "wiki/auth-patterns.md".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("CURATED WISDOM"),
            "expected CURATED WISDOM label, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_includes_source_header_with_line_range() {
        let chunks = vec![
            ChunkRow {
                text: "body text".to_string(),
                symbol_name: None,
                start_line: 12,
                end_line: 34,
                tier: "user_doc".to_string(),
                path: "documents/api-ref.md".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("[source: documents/api-ref.md | lines 12-34]"),
            "expected source header, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_includes_symbol_name_when_present() {
        let chunks = vec![
            ChunkRow {
                text: "fn foo() {}".to_string(),
                symbol_name: Some("foo".to_string()),
                start_line: 22,
                end_line: 45,
                tier: "wiki".to_string(),
                path: "src/db/init.rs".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("[source: src/db/init.rs | symbol: foo | lines 22-45]"),
            "expected symbol in header, got:\n{context}"
        );
    }
}
