//! MCP stdio server for vault search. Activated when the binary is launched with `--mcp`.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ServiceExt};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::dispatcher::Dispatch;

use crate::embedder::EmbedProfile;
use crate::retrieval;

#[derive(Clone)]
struct VaultMcpServer {
    conn: Arc<Mutex<Connection>>,
    profile: EmbedProfile,
    vault_dir: Option<std::path::PathBuf>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VaultSemanticSearchParams {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VaultRelatedChunksParams {
    doc_path: String,
    #[serde(default)]
    limit: Option<usize>,
}

fn lock_conn(conn: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>, rmcp::ErrorData> {
    conn.lock()
        .map_err(|_| rmcp::ErrorData::internal_error("database mutex poisoned", None))
}

fn normalize_path_lexically(path: &std::path::Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn canonicalize_absolute_path_under_vault(path: &Path, vault_canonical: &Path) -> Option<std::path::PathBuf> {
    let mut cur = path.to_path_buf();
    let mut tail = std::path::PathBuf::new();

    loop {
        match cur.canonicalize() {
            Ok(canon_prefix) => {
                if !canon_prefix.starts_with(vault_canonical) {
                    return None;
                }
                return Some(canon_prefix.join(&tail));
            }
            Err(_) => {
                let name = cur.file_name()?.to_owned();
                tail = std::path::Path::new(&name).join(&tail);
                if !cur.pop() {
                    return None;
                }
            }
        }
    }
}

fn build_path_candidates(doc_path: &str, vault_dir: Option<&Path>) -> Vec<String> {
    let p = Path::new(doc_path);
    let mut candidates: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !candidates.iter().any(|e| e == &s) {
            candidates.push(s);
        }
    };

    if let Some(vault) = vault_dir {
        let vault_canonical = vault.canonicalize().unwrap_or_else(|_| normalize_path_lexically(vault));
        let is_safe_relative = || {
            p.components().all(|c| {
                !matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_))
            })
        };

        if p.is_absolute() {
            if let Ok(rel) = crate::normalize_path_argument_to_vault_relative(doc_path, vault) {
                push(doc_path.to_string());
                let abs_candidate = vault_canonical.join(&rel);
                push(abs_candidate.to_string_lossy().into_owned());
                if !rel.is_empty() {
                    push(rel);
                }
            } else if vault.canonicalize().is_err() {
                let accepted_candidate = p
                    .canonicalize()
                    .ok()
                    .filter(|canon| canon.starts_with(&vault_canonical))
                    .or_else(|| {
                        let normalized = normalize_path_lexically(p);
                        if normalized.starts_with(&vault_canonical) {
                            Some(normalized)
                        } else {
                            canonicalize_absolute_path_under_vault(p, &vault_canonical)
                        }
                    });

                if let Some(candidate) = accepted_candidate {
                    let mut candidate_string = candidate.to_string_lossy().into_owned();
                    if candidate_string != std::path::MAIN_SEPARATOR.to_string()
                        && candidate_string.ends_with(std::path::MAIN_SEPARATOR)
                    {
                        candidate_string = candidate_string
                            .trim_end_matches(std::path::MAIN_SEPARATOR)
                            .to_string();
                    }
                    push(doc_path.to_string());
                    push(candidate_string.clone());
                    if let Ok(rel) = std::path::Path::new(&candidate_string).strip_prefix(&vault_canonical) {
                        if !rel.as_os_str().is_empty() {
                            push(rel.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        } else if is_safe_relative() {
            let joined = vault.join(p);
            if let Ok(canon) = joined.canonicalize() {
                if canon.starts_with(&vault_canonical) {
                    push(doc_path.to_string());
                    push(canon.to_string_lossy().into_owned());
                }
            } else {
                let normalized = normalize_path_lexically(&joined);
                if normalized.starts_with(&vault_canonical) {
                    push(doc_path.to_string());
                    push(normalized.to_string_lossy().into_owned());
                }
            }
        }
    } else {
        push(doc_path.to_string());
    }

    candidates
}

#[tool_router(server_handler)]
impl VaultMcpServer {
    #[tool(
        name = "vault_semantic_search",
        description = "Semantic search over vault chunks using the configured embedding profile."
    )]
    async fn vault_semantic_search(
        &self,
        args: Parameters<VaultSemanticSearchParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(VaultSemanticSearchParams { query, limit }) = args;
        let limit = limit.unwrap_or(10).clamp(1, 50);
        // Compute embedding before taking the DB lock — embed_one is CPU/network bound.
        let query_vec = tokio::task::spawn_blocking({
            let profile = self.profile.clone();
            let query = query.clone();
            move || crate::embedder::embed_one(&profile, query)
        })
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("embed task failed: {e}"), None))?
        .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;

        let hits = tokio::task::spawn_blocking({
            let conn = self.conn.clone();
            move || {
                let conn = lock_conn(&conn)?;
                crate::search::semantic_search(&conn, &query_vec, limit)
                    .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))
            }
        })
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("db task failed: {e}"), None))??;

        serde_json::to_string(&hits)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "vault_related_chunks",
        description = "List chunks related to a vault document path. Accepts vault-relative paths (e.g. `notes/meeting.md`) or absolute paths — tries multiple path spellings for maximum compatibility."
    )]
    async fn vault_related_chunks(
        &self,
        args: Parameters<VaultRelatedChunksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(VaultRelatedChunksParams { doc_path, limit }) = args;
        let limit = limit.unwrap_or(5).clamp(1, 10);
        let candidates = build_path_candidates(&doc_path, self.vault_dir.as_deref());
        let hits = tokio::task::spawn_blocking({
            let conn = self.conn.clone();
            let candidates = candidates.clone();
            move || {
                let conn = lock_conn(&conn)?;
                crate::search::related_chunks_try_paths(&conn, &candidates, limit)
                    .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))
            }
        })
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("db task failed: {e}"), None))??;
        serde_json::to_string(&hits)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }
}

/// Blocking entrypoint for `--mcp` mode. Calls into a tokio runtime internally.
/// All tracing/logging must go to stderr only — stdout carries JSON-RPC frames.
pub fn run() -> anyhow::Result<()> {
    // Redirect all tracing to stderr so it never corrupts the JSON-RPC stream.
    // NOTE: this subscriber only governs `tracing` macros. Raw `println!` calls or
    // stdout writes from C/C++ extensions (e.g. fastembed, ort) bypass it entirely.
    // If a dependency ever starts writing to stdout, pipe-based MCP clients will see
    // corrupted JSON-RPC frames. Audit new native deps for hardcoded stdout output.
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .finish();
    let dispatcher = Dispatch::new(subscriber);
    if let Err(err) = tracing::dispatcher::set_global_default(dispatcher.clone()) {
        eprintln!(
            "curated-thoughts [--mcp]: failed to set global tracing subscriber: {err}"
        );
    }
    let _subscriber_guard = tracing::dispatcher::set_default(&dispatcher);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async_run())
}

async fn async_run() -> anyhow::Result<()> {
    let p = retrieval::resolve_brain_paths();

    let profile = retrieval::load_embed_profile(&p.config_path).map_err(|e| {
        eprintln!(
            "curated-thoughts [--mcp]: failed to load embed profile from {}: {e}",
            p.config_path.display()
        );
        e
    })?;

    let conn = retrieval::open_brain_readonly(&p.db_path).map_err(|e| {
        eprintln!("curated-thoughts [--mcp]: {e}");
        e
    })?;

    let vault_dir = crate::vault::VaultConfig::new(p.config_path.clone())
        .get_vault_path()
        .ok()
        .flatten()
        .map(std::path::PathBuf::from)
        .and_then(|path| path.canonicalize().ok());

    let server = VaultMcpServer {
        conn: Arc::new(Mutex::new(conn)),
        profile,
        vault_dir,
    };

    let transport = rmcp::transport::stdio();
    let handle = server
        .serve(transport)
        .await
        .map_err(|e| anyhow::anyhow!("MCP server failed to start: {e}"))?;
    handle
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP server task ended with error: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_path_candidates;
    use tempfile::TempDir;

    #[test]
    fn relative_path_no_vault_dir() {
        let candidates = build_path_candidates("notes/meeting.md", None);
        assert_eq!(candidates, vec!["notes/meeting.md".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_with_vault_dir() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("notes/meeting.md", Some(vault));
        // canon of joined path won't exist on disk, so no canonicalized form
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], "notes/meeting.md");
        assert_eq!(candidates[1], "/home/user/vault/notes/meeting.md");
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_under_vault_dir() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("/home/user/vault/notes/meeting.md", Some(vault));
        // file doesn't exist on disk, no canonicalized form
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], "/home/user/vault/notes/meeting.md");
        assert_eq!(candidates[1], "notes/meeting.md");
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_under_vault_dir_with_canonical_prefix_alias() {
        let temp_dir = TempDir::new().unwrap();
        let vault_real = temp_dir.path().join("vault-real");
        std::fs::create_dir_all(&vault_real).unwrap();
        let vault_alias = temp_dir.path().join("vault-alias");
        std::os::unix::fs::symlink(&vault_real, &vault_alias).unwrap();

        let doc_path = vault_alias.join("notes/meeting.md");
        let expected_canonical = vault_real
            .canonicalize()
            .unwrap()
            .join("notes/meeting.md");

        let candidates = build_path_candidates(doc_path.to_string_lossy().as_ref(), Some(&vault_real));

        assert!(candidates.contains(&doc_path.to_string_lossy().into_owned()));
        assert!(candidates.contains(&expected_canonical.to_string_lossy().into_owned()));
        assert!(candidates.contains(&"notes/meeting.md".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_outside_vault_dir_no_strip() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("/tmp/other/file.md", Some(vault));
        assert!(candidates.is_empty(), "Outside-vault absolute paths should not be accepted");
    }

    #[cfg(unix)]
    #[test]
    fn no_duplicates_when_path_matches_joined() {
        let vault = std::path::Path::new("/home/user/vault");
        // When doc_path is already the absolute joined form, it should appear only once.
        let candidates = build_path_candidates("/home/user/vault/notes/meeting.md", Some(vault));
        let count = candidates
            .iter()
            .filter(|c| c.as_str() == "/home/user/vault/notes/meeting.md")
            .count();
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_with_parent_segments_outside_vault_is_rejected() {
        let vault = std::path::Path::new("/vault");
        let candidates = build_path_candidates("/vault/../outside.md", Some(vault));
        assert!(candidates.is_empty(), "Paths that normalize outside the vault must not be accepted");
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_with_parent_segments_outside_vault_is_rejected() {
        let vault = std::path::Path::new("/vault");
        let candidates = build_path_candidates("../outside.md", Some(vault));
        assert!(candidates.is_empty(), "Relative paths that resolve outside the vault must not be accepted");
    }
}
