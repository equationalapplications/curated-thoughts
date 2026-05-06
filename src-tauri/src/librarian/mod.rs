use anyhow::Result;
use rusqlite::Connection;

pub fn generate_summary(
    conn: &Connection,
    source_path: &str,
    model: &str,
) -> Result<()> {
    // Load source chunks from DB
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

    // Store proposed page in wiki_pages table
    let source_ids = serde_json::json!([source_path]).to_string();
    conn.execute(
        "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status)
         VALUES (?1, ?2, ?3, 'pending_review')
         ON CONFLICT(path) DO UPDATE SET
           source_doc_ids = ?2,
           generated_by = ?3,
           status = 'pending_review',
           last_synced = unixepoch()",
        rusqlite::params![source_file, source_ids, model],
    )?;

    // Write proposed content to .brain/proposed/ for retrieval
    let proposed_dir = std::path::Path::new(source_path)
        .parent()
        .and_then(|p| p.parent())
        .map(|vault| vault.join(".brain").join("proposed"));

    if let Some(dir) = proposed_dir {
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join(&source_file), &wiki_content).ok();
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
        // No chunks for this path → returns Ok without calling Ollama
        let result = generate_summary(&conn, "/vault/documents/nonexistent.md", "llama3.2:1b");
        assert!(result.is_ok());
    }
}
