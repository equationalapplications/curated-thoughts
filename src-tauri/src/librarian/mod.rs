use anyhow::Result;
use rusqlite::Connection;

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

pub fn generate_summary(
    conn: &Connection,
    source_path: &str,
    model: &str,
) -> Result<()> {
    let (mode, auto_approve) = get_folder_mode(conn, source_path);

    if mode == "index" {
        return Ok(());
    }

    let chunks: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT c.chunk_text FROM chunks c
             JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1
             ORDER BY c.position",
        )?;
        let mut rows = stmt.query([source_path])?;
        let mut texts = Vec::new();
        while let Some(row) = rows.next()? {
            texts.push(row.get::<_, String>(0)?);
        }
        texts
    };

    if chunks.is_empty() {
        return Ok(());
    }

    let source_text = chunks.join("\n\n");
    let truncated = &source_text[..source_text.len().min(4000)];

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model,
            "system": "You are a knowledge librarian. Summarize the document into a concise wiki page in markdown format. Use headings and bullet points, keep under 400 words. Output only markdown.",
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

    let initial_status = if auto_approve { "approved" } else { "pending_review" };
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
}
