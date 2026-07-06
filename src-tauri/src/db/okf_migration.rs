//! V7 data conversion: approved wiki pages → curated_entities, pending proposals dropped,
//! wiki-tier document purge. Idempotent via `llm_wiki_meta.okf_migrated_at`.

use crate::db::okf_ddl::LLM_WIKI_META_TABLE;
use crate::hasher::hash_bytes;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Component, Path};

pub const OKF_MIGRATED_META_KEY: &str = "okf_migrated_at";

fn normalize_wiki_relative_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let already_wiki = matches!(
        Path::new(&normalized).components().next(),
        Some(Component::Normal(seg)) if seg == std::ffi::OsStr::new("wiki")
    );
    if already_wiki {
        normalized
    } else {
        format!("wiki/{}", normalized)
    }
}

/// Deterministic entity id from a wiki page path (spec §4 step 2).
pub fn entity_id_from_wiki_path(page_path: &str) -> String {
    let normalized = normalize_wiki_relative_path(page_path);
    format!("entity::{}", &hash_bytes(normalized.as_bytes())[..16])
}

fn wiki_page_entity_name(page_path: &str, body: &str) -> String {
    if let Some(h1) = body.lines().find_map(|line| {
        let t = line.trim();
        t.strip_prefix("# ").map(str::trim)
    }) {
        if !h1.is_empty() {
            return h1.to_string();
        }
    }
    Path::new(page_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(page_path)
        .to_string()
}

fn read_wiki_page_body(vault_root: &Path, page_path: &str) -> (String, bool) {
    let rel = normalize_wiki_relative_path(page_path);
    let file_path = if rel.starts_with("wiki/") {
        vault_root.join(&rel)
    } else {
        vault_root.join("wiki").join(&rel)
    };
    match std::fs::read_to_string(&file_path) {
        Ok(body) => (body, true),
        Err(_) => (String::new(), false),
    }
}

fn okf_migration_complete(conn: &Connection) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [LLM_WIKI_META_TABLE],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(false);
    }
    let migrated: Option<String> = conn
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [OKF_MIGRATED_META_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(migrated.is_some())
}

fn migrate_approved_wiki_pages(conn: &Connection, vault_root: &Path, now: i64) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, path FROM wiki_pages WHERE status = 'approved' ORDER BY id",
    )?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut count = 0usize;
    for (_page_id, path) in rows {
        let entity_id = entity_id_from_wiki_path(&path);
        let (body, file_found) = read_wiki_page_body(vault_root, &path);
        let name = wiki_page_entity_name(&path, &body);
        let summary = body;

        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, 'concept', ?3, NULL, ?4, ?4, NULL)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               summary = excluded.summary,
               updated_at = excluded.updated_at",
            params![entity_id, name, summary, now],
        )?;

        let event_id = format!("evt-migrate-{}", &hash_bytes(entity_id.as_bytes())[..12]);
        let summary_text = if file_found {
            format!("Migrated from wiki page *{path}*")
        } else {
            format!("Migrated from wiki page *{path}* (file missing)")
        };
        conn.execute(
            "INSERT OR IGNORE INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
             VALUES (?1, ?2, 'imported', ?3, NULL, ?4)",
            params![event_id, entity_id, summary_text, now],
        )?;
        count += 1;
    }
    Ok(count)
}

fn drop_pending_wiki_proposals(conn: &Connection, vault_root: &Path) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, path, source_doc_ids FROM wiki_pages WHERE status = 'pending_review'",
    )?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (page_id, path, source_doc_ids) in rows {
        conn.execute(
            "UPDATE wiki_pages SET status = 'orphaned' WHERE id = ?1",
            [page_id],
        )?;

        let proposed_path = vault_root.join(".brain").join("proposed").join(&path);
        let _ = std::fs::remove_file(&proposed_path);

        if let Ok(doc_ids) = serde_json::from_str::<Vec<i64>>(&source_doc_ids) {
            for doc_id in doc_ids {
                conn.execute(
                    "UPDATE documents SET status = 'pending' WHERE id = ?1",
                    [doc_id],
                )?;
            }
        }
    }
    Ok(())
}

fn purge_wiki_tier_documents(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM documents WHERE tier = 'wiki'", [])?;
    Ok(())
}

fn mark_okf_migrated(conn: &Connection, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO llm_wiki_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![OKF_MIGRATED_META_KEY, now.to_string()],
    )?;
    Ok(())
}

/// Run V7 data conversion when vault path is known. Safe to call repeatedly.
pub fn run_okf_migration(conn: &Connection, vault_root: &Path) -> Result<()> {
    if okf_migration_complete(conn)? {
        return Ok(());
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_secs() as i64;

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<()> {
        migrate_approved_wiki_pages(conn, vault_root, now)?;
        drop_pending_wiki_proposals(conn, vault_root)?;
        purge_wiki_tier_documents(conn)?;
        mark_okf_migrated(conn, now)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK;");
        return result;
    }
    conn.execute_batch("COMMIT;")?;

    conn.execute_batch("VACUUM;")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::db::okf_ddl::{CURATED_TABLES_DDL, LLM_WIKI_PACKAGE_DDL};

    fn open_v7_db() -> Connection {
        let conn = open_in_memory().unwrap();
        conn.execute_batch(LLM_WIKI_PACKAGE_DDL).unwrap();
        conn.execute_batch(CURATED_TABLES_DDL).unwrap();
        conn
    }

    #[test]
    fn entity_id_is_deterministic() {
        assert_eq!(
            entity_id_from_wiki_path("note.md"),
            entity_id_from_wiki_path("note.md")
        );
        assert_ne!(
            entity_id_from_wiki_path("a.md"),
            entity_id_from_wiki_path("b.md")
        );
    }

    #[test]
    fn wiki_page_entity_name_uses_h1_or_stem() {
        assert_eq!(
            wiki_page_entity_name("foo.md", "# My Title\n\nbody"),
            "My Title"
        );
        assert_eq!(wiki_page_entity_name("bar.md", "no heading"), "bar");
    }

    #[test]
    fn migration_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault = tmp.path();
        std::fs::create_dir_all(vault.join("wiki")).unwrap();
        std::fs::write(vault.join("wiki/page.md"), "# Page\n\nBody.").unwrap();

        let conn = open_v7_db();
        conn.execute(
            "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status)
             VALUES ('page.md', '[]', 'test', 'approved')",
            [],
        )
        .unwrap();

        run_okf_migration(&conn, vault).unwrap();
        let count1: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();

        run_okf_migration(&conn, vault).unwrap();
        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();

        assert_eq!(count1, 1);
        assert_eq!(count2, 1);
    }
}
