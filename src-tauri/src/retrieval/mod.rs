//! Shared resolution of brain database/config paths and read-only semantic search façade.

use anyhow::Result;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::embedder::{embed_one, EmbedProfile};
use crate::vault::VaultConfig;

/// Re-exported for integration tests that seed a brain fixture (see `tests/retrieval_facade.rs`).
pub use crate::db::queries::{
    insert_chunk, insert_embedding, mark_document_indexed, upsert_document,
};
pub use crate::db::AppDb;

/// Canonical brain layout derived from env (`CURATED_BRAIN_DB`, `CURATED_BRAIN_CONFIG`, `CURATED_BRAIN_DIR`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrainPaths {
    pub brain_dir: PathBuf,
    pub config_path: PathBuf,
    pub db_path: PathBuf,
}

/// Resolve `brain_dir`, `config_path`, and `db_path` from environment variables.
///
/// `CURATED_BRAIN_DIR` defaults to `$HOME/.brain` via [`dirs::home_dir`].
///
/// Config: explicit `CURATED_BRAIN_CONFIG` wins; otherwise if `CURATED_BRAIN_DB` is set, use
/// `{parent(db)}/config.json`; otherwise `{brain_dir}/config.json`.
///
/// DB: `CURATED_BRAIN_DB` if set, else `{brain_dir}/brain.db`.
pub fn resolve_brain_paths() -> BrainPaths {
    let brain_dir = std::env::var_os("CURATED_BRAIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".brain")
        });

    let db_path = std::env::var_os("CURATED_BRAIN_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| brain_dir.join("brain.db"));

    let config_path = if let Some(p) = std::env::var_os("CURATED_BRAIN_CONFIG") {
        PathBuf::from(p)
    } else if std::env::var_os("CURATED_BRAIN_DB").is_some() {
        db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("config.json")
    } else {
        brain_dir.join("config.json")
    };

    BrainPaths {
        brain_dir,
        config_path,
        db_path,
    }
}

/// Resolve the brain layout for an explicitly supplied `brain_dir`.
///
/// Same env-var contract as [`resolve_brain_paths`] for `CURATED_BRAIN_DB` and
/// `CURATED_BRAIN_CONFIG`, but the caller's `brain_dir` replaces
/// `CURATED_BRAIN_DIR`, so callers that already hold a brain directory (tests
/// with a temp dir, commands that take a path argument) are honored instead of
/// silently falling back to the globally resolved `~/.brain`.
pub fn brain_paths_for(brain_dir: &Path) -> BrainPaths {
    let db_path = std::env::var_os("CURATED_BRAIN_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| brain_dir.join("brain.db"));

    let config_path = if let Some(p) = std::env::var_os("CURATED_BRAIN_CONFIG") {
        PathBuf::from(p)
    } else if std::env::var_os("CURATED_BRAIN_DB").is_some() {
        db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("config.json")
    } else {
        brain_dir.join("config.json")
    };

    BrainPaths {
        brain_dir: brain_dir.to_path_buf(),
        config_path,
        db_path,
    }
}

pub fn load_embed_profile(config_path: impl AsRef<Path>) -> Result<EmbedProfile> {
    VaultConfig::new(config_path.as_ref().to_path_buf()).get_embed_profile()
}

/// Opens the SQLite brain database read-only (`SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX`).
pub fn open_brain_readonly(db_path: &Path) -> Result<Connection> {
    if !db_path.exists() {
        anyhow::bail!(
            "brain.db not found at {}; set CURATED_BRAIN_DIR or CURATED_BRAIN_DB",
            db_path.display()
        );
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Ok(Connection::open_with_flags(db_path, flags)?)
}

/// Embed query + cosine search (`search::semantic_search`) — mirrors Tauri `search_vault` semantics.
pub fn semantic_search_chunks(
    conn: &Connection,
    profile: &EmbedProfile,
    query: &str,
    limit: usize,
) -> Result<Vec<crate::search::SearchResult>> {
    let query_vec = embed_one(profile, query.to_string())?;
    crate::search::semantic_search(conn, &query_vec, limit.clamp(1, 50))
}

pub fn related_chunks_facade(
    conn: &Connection,
    doc_path: &str,
    limit: usize,
) -> Result<Vec<crate::search::SearchResult>> {
    crate::search::related_chunks(conn, doc_path, limit.clamp(1, 10))
}

/// Maps retrieval failures to MCP-friendly text with actionable hints (missing DB paths, SQLITE_BUSY,
/// embedding connectivity).
pub fn mcp_error_hint(err: &anyhow::Error) -> String {
    let base = format!("{err:#}");
    let lowered = base.to_lowercase();
    let hint = if lowered.contains("brain.db not found")
        || lowered.contains("curated_brain_dir")
        || lowered.contains("curated_brain_db")
    {
        Some("Point CURATED_BRAIN_DIR at the folder that contains config.json and brain.db, or set CURATED_BRAIN_DB to the database file path.")
    } else if lowered.contains("database is locked")
        || lowered.contains("sqlite_busy")
        || (lowered.contains("unable to open") && lowered.contains("locked"))
    {
        Some("Another process may be writing to the database (e.g. the desktop app). Close it or retry; read-only MCP access still contends if a writer holds the journal.")
    } else if lowered.contains("/api/embed status")
        || lowered.contains("ollama")
        || ((lowered.contains("connection refused")
            || lowered.contains("error sending request for url"))
            && lowered.contains("11434"))
    {
        Some("Ensure Ollama is running (see OLLAMA_HOST) and the configured local embed model is available (e.g. ollama pull <model>).")
    } else if lowered.contains("cloud embed not implemented") {
        Some("This MCP build only resolves local Ollama embedding profiles.")
    } else if lowered.contains("error sending request for url") {
        Some("Embedding HTTP request failed; check reachability of the embedding service and TLS/network settings.")
    } else {
        None
    };

    hint.map(|h| format!("{base}\n\nHint: {h}")).unwrap_or(base)
}

#[cfg(test)]
mod hint_tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn hints_missing_db() {
        let e = anyhow!("brain.db not found at /tmp/x; set CURATED_BRAIN_DIR or CURATED_BRAIN_DB");
        assert!(mcp_error_hint(&e).contains("CURATED_BRAIN_DIR"));
    }

    #[test]
    fn hints_sqlite_busy() {
        assert!(mcp_error_hint(&anyhow!(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ErrorCode::DatabaseBusy as i32),
            Some("database is locked".into()),
        )))
        .contains("Another process"));
    }

    #[test]
    fn hint_cloud_embed() {
        let e = anyhow!("cloud embed not implemented");
        assert!(mcp_error_hint(&e).contains("Ollama"));
    }
}
