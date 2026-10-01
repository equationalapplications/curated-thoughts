//! `wisdom_*` tool dispatchers — the rule-1 sanctioned agent write path.
//!
//! Spec: `docs/superpowers/specs/2026-10-01-wisdom-deposit-tool-surface-design.md`.
//! Agents never write brain rows (INTENT rule 1): these dispatchers write
//! vault FILES under `immutable-source-files/agents/`, kick the per-document
//! pipeline (ingest + librarian) under the vault lock, and report honest
//! `pending` state from `deposit_kick_state` / the `librarian_evidence` probe.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tool_dispatch::ToolDispatchContext;

const AGENTS_DIR: &str = crate::vault::safe_path::AGENTS_DEPOSIT_DIR;
const SUPERSESSIONS_DIR: &str = "immutable-source-files/agents/supersessions";

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct WisdomDepositParams {
    /// Vault-relative deposit path under `immutable-source-files/agents/`.
    /// Must not already exist (append-only, INTENT rule 9).
    pub path: String,
    /// Fact-style title (becomes the document's H1).
    pub title: String,
    /// Fact body text.
    pub body: String,
    /// Optional tags.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct WisdomDepositStatusParams {
    /// Vault-relative deposit path (as returned by `wisdom_deposit`).
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct WisdomProposeSupersessionParams {
    /// The target fact's `librarian-…` source_ref token.
    #[serde(default)]
    pub target_source_ref: Option<String>,
    /// The target fact's brain id (`fact_…`). Supply exactly one of the two
    /// targets.
    #[serde(default)]
    pub target_fact_id: Option<String>,
    /// Replacement title.
    pub replacement_title: String,
    /// Replacement body.
    pub replacement_body: String,
    /// Why the supersession is proposed.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Deposit file rendering
// ---------------------------------------------------------------------------

fn render_deposit_file(title: &str, body: &str, tags: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {title}\n\n"));
    if !tags.is_empty() {
        let list = tags
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("{list}\n\n"));
    }
    out.push_str(body.trim_end());
    out.push('\n');
    out
}

fn render_supersession_file(
    target: &str,
    replacement_title: &str,
    replacement_body: &str,
    reason: &str,
) -> String {
    let stamp = chrono::DateTime::from_timestamp(now_ms(), 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default();
    format!(
        "# Supersession: {replacement_title}\n\n\
         - supersedes: `{target}`\n\
         - deposited_at: {stamp}\n\
         - reason: {reason}\n\n\
         # {replacement_title}\n\n{}\n",
        replacement_body.trim_end()
    )
}

// ---------------------------------------------------------------------------
// Kick-state ledger helpers
// ---------------------------------------------------------------------------

fn set_kick_state(
    conn: &rusqlite::Connection,
    path: &str,
    state: &str,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO deposit_kick_state (path, state, error, updated_ms)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET state = ?2, error = ?3, updated_ms = ?4",
        rusqlite::params![path, state, error, now_ms()],
    )
    .map_err(|e| anyhow!("kick-state write failed: {e}"))?;
    Ok(())
}

/// Status probe: librarian evidence linked to THIS document via
/// proposal sources → documents (the V18 doc link; `librarian_evidence`
/// carries no path of its own).
pub(crate) fn deposit_status_row(
    conn: &rusqlite::Connection,
    vault_relative_path: &str,
) -> Result<Value> {
    let kick: Option<(String, Option<String>, i64)> = conn
        .query_row(
            "SELECT state, error, updated_ms FROM deposit_kick_state WHERE path = ?1",
            [vault_relative_path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .map_err(|e| anyhow!("kick-state read failed: {e}"))?;

    // Fact rows linked to this document (joined through the proposal that
    // consumed its chunks). RR-4 honesty: tier/supersession enrichment only
    // when the columns are readable; ids are always returned.
    let enrich = column_exists(conn, "llm_wiki_entries", "superseded_by")?;
    let sql = if enrich {
        "SELECT e.id, e.tier, e.superseded_by FROM librarian_evidence le
           JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
           JOIN documents d ON d.id = ps.doc_id
           JOIN llm_wiki_entries e ON e.id = le.entry_id
          WHERE d.path = ?1"
    } else {
        "SELECT e.id, e.tier, NULL FROM librarian_evidence le
           JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
           JOIN documents d ON d.id = ps.doc_id
           JOIN llm_wiki_entries e ON e.id = le.entry_id
          WHERE d.path = ?1"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| anyhow!("status probe failed: {e}"))?;
    let facts: Vec<(String, Option<String>, Option<String>)> = stmt
        .query_map([vault_relative_path], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| anyhow!("status probe failed: {e}"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| anyhow!("status probe failed: {e}"))?;

    // Pending check for the D6 query shape: chunks exist for this doc?
    let chunked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks c
               JOIN documents d ON d.id = c.doc_id
              WHERE d.path = ?1",
            [vault_relative_path],
            |r| r.get(0),
        )
        .map_err(|e| anyhow!("chunks probe failed: {e}"))?;

    let state = match (&kick, facts.is_empty(), chunked > 0) {
        (Some((s, _, _)), _, _) if s != "pending" && s != "queued_watcher" && s != "no_ingest_host" => s.clone(),
        (_, false, _) => "ingested".to_string(),
        (_, true, true) => "chunked".to_string(),
        (Some((s, _, _)), _, _) => s.clone(),
        (_, true, false) => "pending".to_string(),
    };

    let mut fact_ids: Vec<Value> = Vec::new();
    for (id, tier, superseded_by) in facts {
        let mut f = json!({ "id": id });
        if let Some(t) = tier {
            f["tier"] = json!(t);
        }
        if let Some(s) = superseded_by {
            f["superseded_by"] = json!(s);
        }
        fact_ids.push(f);
    }

    let mut out = json!({ "path": vault_relative_path, "state": state, "facts": fact_ids });
    if let Some((s, err, updated)) = kick {
        out["kick"] = json!(s);
        if let Some(e) = err {
            out["error"] = json!(e);
        }
        out["kick_updated_ms"] = json!(updated);
    }
    Ok(out)
}

fn column_exists(conn: &rusqlite::Connection, table: &str, column: &str) -> Result<bool> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            rusqlite::params![table, column],
            |r| r.get(0),
        )
        .map_err(|e| anyhow!("pragma probe failed: {e}"))?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// The kick (D3): full per-document pipeline under the vault lock
// ---------------------------------------------------------------------------

fn brain_dir_of(db_path: &std::path::Path) -> PathBuf {
    db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Run ingest + librarian for one deposit. Called from `spawn_blocking`;
/// the vault lock serializes against any running `ct watch` / worker.
fn run_kick(
    db_path: PathBuf,
    vault_dir: PathBuf,
    rel_path: String,
    profile: crate::embedder::EmbedProfile,
    generation_configured: bool,
) {
    let outcome = (|| -> Result<String> {
        let _lock = crate::watcher::VaultLock::acquire(&brain_dir_of(&db_path))
            .map_err(|e| anyhow!("vault lock: {e:#}"))?;
        let mut conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("open brain {}", db_path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();

        crate::pipeline::ingest_document_with_vault_root(
            &conn,
            &profile,
            &rel_path,
            false,
            Some(vault_dir.to_string_lossy().as_ref()),
        )
        .map_err(|e| anyhow!("ingest: {e:#}"))?;
        set_kick_state(&conn, &rel_path, "chunked", None)?;

        // Librarian leg. `generate_summary` consults folder rules itself
        // (agents = synthesize + auto_approve after V25); with no configured
        // generation provider it errors — that's `chunked` (no librarian
        // host), not `failed`.
        if !generation_configured {
            set_kick_state(&conn, &rel_path, "chunked", None)?;
            return Ok("chunked".to_string());
        }
        let model = crate::librarian::active_generation_model(crate::setup::recommended_model());
        match crate::librarian::generate_summary(&mut conn, &rel_path, &model, false) {
            Ok(()) => {
                set_kick_state(&conn, &rel_path, "ingested", None)?;
                Ok("ingested".to_string())
            }
            Err(e) => {
                set_kick_state(&conn, &rel_path, "failed", Some(&format!("{e:#}")))?;
                Ok("failed".to_string())
            }
        }
    })();

    // Persist the outcome outside the closure (needs its own connection when
    // the closure failed before opening one).
    if let Err(err) = outcome {
        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
            let _ = set_kick_state(&conn, &rel_path, "failed", Some(&format!("{err:#}")));
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatchers
// ---------------------------------------------------------------------------

fn vault_dir(ctx: &ToolDispatchContext) -> Result<PathBuf> {
    ctx.vault_dir
        .clone()
        .ok_or_else(|| anyhow!("no vault configured (vault_dir unset)"))
}

pub async fn dispatch_wisdom_deposit(
    ctx: &ToolDispatchContext,
    p: WisdomDepositParams,
) -> Result<Value> {
    let vault = vault_dir(ctx)?;
    let rel = p.path.replace('\\', "/");
    if !rel.starts_with(AGENTS_DIR) || rel.starts_with(SUPERSESSIONS_DIR) {
        bail!(
            "deposit path must be under {AGENTS_DIR} (not {SUPERSESSIONS_DIR}): {}",
            p.path
        );
    }
    if p.title.trim().is_empty() || p.body.trim().is_empty() {
        bail!("title and body are required");
    }
    // Append-only (rule 9) + symlink/traversal guard — one check does both:
    // MayCreate on an existing path errors in okf::write; here we fail before
    // any bytes are written.
    let target = vault.join(&rel);
    if target.symlink_metadata().is_ok() {
        bail!("deposit_exists: {} (append-only; supersede instead)", rel);
    }

    let content = render_deposit_file(&p.title, &p.body, p.tags.as_deref().unwrap_or(&[]));
    crate::vault::safe_path::safe_vault_path(
        &vault,
        &rel,
        &[AGENTS_DIR],
        crate::vault::safe_path::PathMode::MayCreate,
    )
    .map_err(|e| anyhow!("unsafe deposit path: {e}"))?;
    std::fs::create_dir_all(target.parent().expect("parent under agents/"))?;
    crate::vault::safe_path::safe_write_bytes(&target, content.as_bytes())
        .map_err(|e| anyhow!("deposit write failed: {e}"))?;

    // Audit (fail-closed, RW connection, same contract as the removed
    // curated write tools) — `wisdom_` joins the curated audit class.
    let audit_path = rel.clone();
    let client = ctx.client.clone();
    ctx.with_rw(move |conn| {
        crate::tool_dispatch::log_agent_access_checked(
            conn,
            &client,
            "wisdom_deposit",
            Some(audit_path.as_str()),
            "write",
        )
    })
    .await?;

    // Kick (D3): full pipeline, lock-serialized. Record the intent first so a
    // crash between write and spawn leaves an honest ledger row.
    let db_path = ctx.db_path.clone();
    let kick = {
        let conn = ctx
            .conn
            .lock()
            .map_err(|_| anyhow!("conn mutex poisoned"))?;
        let generation_configured = crate::librarian::llm_generation_configured();
        let initial = if generation_configured { "pending" } else { "no_ingest_host" };
        set_kick_state(&conn, &rel, initial, None)?;
        if generation_configured {
            let db_path2 = db_path.clone();
            let vault2 = vault.clone();
            let rel2 = rel.clone();
            let profile2 = ctx.profile.clone();
            tokio::task::spawn_blocking(move || {
                run_kick(db_path2, vault2, rel2, profile2, true)
            });
        }
        initial.to_string()
    };
    let kick_label = if kick == "pending" { "started" } else { "no_ingest_host" };

    Ok(json!({
        "path": rel,
        "pending": true,
        "kick": kick_label,
    }))
}

pub async fn dispatch_wisdom_deposit_status(
    ctx: &ToolDispatchContext,
    p: WisdomDepositStatusParams,
) -> Result<Value> {
    let rel = p.path.replace('\\', "/");
    if !rel.starts_with(AGENTS_DIR) {
        bail!("not a deposit path: {}", p.path);
    }
    let audit_path = rel.clone();
    let client = ctx.client.clone();
    ctx.with_rw(move |conn| {
        crate::tool_dispatch::log_agent_access_checked(
            conn,
            &client,
            "wisdom_deposit_status",
            Some(audit_path.as_str()),
            "read",
        )
    })
    .await?;
    let conn = ctx.conn.lock().map_err(|_| anyhow!("conn mutex poisoned"))?;
    deposit_status_row(&conn, &rel)
}

pub async fn dispatch_wisdom_pending(ctx: &ToolDispatchContext) -> Result<Value> {
    let client = ctx.client.clone();
    ctx.with_rw(move |conn| {
        crate::tool_dispatch::log_agent_access_checked(conn, &client, "wisdom_pending", None, "read")
    })
    .await?;
    let conn = ctx.conn.lock().map_err(|_| anyhow!("conn mutex poisoned"))?;
    // D6: deposits without librarian evidence — including `chunked` and
    // deferred supersession deposits. Structural isolation from recall:
    // separate tool, separate code path.
    let mut stmt = conn
        .prepare(
            "SELECT path FROM deposit_kick_state
              WHERE path NOT IN (
                SELECT d.path FROM librarian_evidence le
                  JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
                  JOIN documents d ON d.id = ps.doc_id
              )
              ORDER BY path",
        )
        .map_err(|e| anyhow!("pending query failed: {e}"))?;
    let paths: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .map_err(|e| anyhow!("pending query failed: {e}"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| anyhow!("pending query failed: {e}"))?;
    let mut items = Vec::new();
    for p in paths {
        let mtime = std::fs::symlink_metadata(ctx.vault_dir.as_ref().unwrap_or(&PathBuf::from(".")).join(&p))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        items.push(json!({ "path": p, "deposited_at": mtime }));
    }
    Ok(json!({ "pending": items }))
}

pub async fn dispatch_wisdom_propose_supersession(
    ctx: &ToolDispatchContext,
    p: WisdomProposeSupersessionParams,
) -> Result<Value> {
    let target = match (p.target_source_ref.as_deref(), p.target_fact_id.as_deref()) {
        (Some(_), Some(_)) => bail!("supply exactly one of target_source_ref / target_fact_id"),
        (None, None) => bail!("supply exactly one of target_source_ref / target_fact_id"),
        (Some(r), None) => r.to_string(),
        (None, Some(id)) => {
            // Resolve the brain id to its librarian token (supersession files
            // carry the stable token; the Librarian's application spec keys
            // on it).
            let conn = ctx.conn.lock().map_err(|_| anyhow!("conn mutex poisoned"))?;
            let token: Option<String> = conn
                .query_row(
                    "SELECT source_ref FROM llm_wiki_entries WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
                .map_err(|e| anyhow!("target lookup failed: {e}"))?;
            token.ok_or_else(|| anyhow!("target_fact_id not found: {id}"))?
        }
    };

    let vault = vault_dir(ctx)?;
    let stamp = now_ms();
    let rel = format!("{SUPERSESSIONS_DIR}/supersession-{stamp}.md");
    let target_abs = vault.join(&rel);
    if target_abs.symlink_metadata().is_ok() {
        bail!("deposit_exists: {rel}");
    }

    let content = render_supersession_file(
        &target,
        &p.replacement_title,
        &p.replacement_body,
        &p.reason,
    );
    crate::vault::safe_path::safe_vault_path(
        &vault,
        &rel,
        &[SUPERSESSIONS_DIR],
        crate::vault::safe_path::PathMode::MayCreate,
    )
    .map_err(|e| anyhow!("unsafe supersession path: {e}"))?;
    std::fs::create_dir_all(vault.join(SUPERSESSIONS_DIR))?;
    crate::vault::safe_path::safe_write_bytes(&target_abs, content.as_bytes())
        .map_err(|e| anyhow!("supersession write failed: {e}"))?;

    let audit_path = rel.clone();
    let client = ctx.client.clone();
    ctx.with_rw(move |conn| {
        crate::tool_dispatch::log_agent_access_checked(
            conn,
            &client,
            "wisdom_propose_supersession",
            Some(audit_path.as_str()),
            "write",
        )
    })
    .await?;

    // Kick: supersessions/ is `summarize` — the file ingests and drains with
    // zero facts (V25 override). Librarian APPLICATION of the supersession is
    // the follow-up reconcile spec (rule 4 mechanics); until then status
    // stays pending.
    let db_path = ctx.db_path.clone();
    {
        let guard = ctx
            .conn
            .lock()
            .map_err(|_| anyhow!("conn mutex poisoned"))?;
        set_kick_state(&guard, &rel, "pending", None)?;
    }
    let vault2 = vault.clone();
    let rel2 = rel.clone();
    let profile2 = ctx.profile.clone();
    let generation_configured = crate::librarian::llm_generation_configured();
    tokio::task::spawn_blocking(move || {
        run_kick(db_path, vault2, rel2, profile2, generation_configured)
    });

    Ok(json!({
        "path": rel,
        "supersedes": target,
        "pending": true,
        "kick": "started",
    }))
}

// ---------------------------------------------------------------------------
// Tests (TDD — spec §2 subset that is unit-level; integration lives in
// mcp_integration.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Open a REAL migrated scratch brain (folder_rules, librarian_evidence,
    /// curated_proposals, deposit_kick_state all present via open_app_db) with
    /// the brain env redirected at `dir`. Must be called INSIDE
    /// [`with_brain`]'s env scope (same discipline as tool_dispatch::with_brain).
    fn wisdom_ctx(dir: &std::path::Path) -> ToolDispatchContext {
        std::fs::create_dir_all(dir.join("immutable-source-files/agents/supersessions")).unwrap();
        let config = json!({ "vault_path": dir.to_str().unwrap() });
        std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
        // Rooted open (like production): VaultRoots from config.json so V22
        // stamps — and V25, which is gated on V22, runs with it.
        let db = crate::db::connection::AppDb::open_with_config(
            &dir.join("brain.db"),
            dir.join("config.json"),
        )
        .unwrap();
        let conn = db.0;
        ToolDispatchContext {
            conn: Arc::new(Mutex::new(conn)),
            profile: crate::embedder::EmbedProfile::default(),
            vault_dir: Some(dir.to_path_buf()),
            client: "test".into(),
            db_path: dir.join("brain.db"),
            rw_conn: Arc::new(Mutex::new(None)),
        }
    }

    /// Run an async test body with brain paths redirected into `dir`
    /// (issue #178 discipline: never resolve the LIVE ~/.brain). `temp_env`
    /// is synchronous and globally serialized, so the runtime is built inside
    /// the redirected scope.
    fn with_brain<F: std::future::Future<Output = ()>>(
        dir: &std::path::Path,
        body: impl FnOnce() -> F,
    ) {
        let brain = dir.to_string_lossy().into_owned();
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_DIR", Some(brain.as_str())),
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
                ("CURATED_EMBED_STUB", Some("constant8")),
            ],
            || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("test runtime")
                    .block_on(body());
            },
        );
    }

    #[test]
    fn deposit_writes_file_and_reports_honest_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
        let ctx = wisdom_ctx(&dir);

        let v = dispatch_wisdom_deposit(
            &ctx,
            WisdomDepositParams {
                path: "immutable-source-files/agents/note-1.md".into(),
                title: "Alpha ships v2".into(),
                body: "Alpha v2 ships 2026-11-01 behind a flag.".into(),
                tags: Some(vec!["alpha".into()]),
            },
        )
        .await
        .unwrap();
        assert_eq!(v["pending"], serde_json::json!(true));
        let file = std::fs::read_to_string(&dir.join("immutable-source-files/agents/note-1.md")).unwrap();
        assert!(file.starts_with("# Alpha ships v2"));
        assert!(file.contains("Alpha v2 ships 2026-11-01"));

        // Append-only: same path again errors.
        let err = dispatch_wisdom_deposit(
            &ctx,
            WisdomDepositParams {
                path: "immutable-source-files/agents/note-1.md".into(),
                title: "x".into(),
                body: "y".into(),
                tags: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("deposit_exists"), "{err}");
        });
    }

    #[test]
    fn deposit_refuses_wrong_lanes_and_empty_fields() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
        let ctx = wisdom_ctx(&dir);

        for bad in [
            "wiki/escape.md",
            "immutable-source-files/human.md",
            "immutable-source-files/agents/supersessions/smuggled.md",
        ] {
            let err = dispatch_wisdom_deposit(
                &ctx,
                WisdomDepositParams {
                    path: bad.into(),
                    title: "t".into(),
                    body: "b".into(),
                    tags: None,
                },
            )
            .await
            .unwrap_err();
            assert!(
                err.to_string().contains("must be under"),
                "{bad}: {err}"
            );
        }
        let err = dispatch_wisdom_deposit(
            &ctx,
            WisdomDepositParams {
                path: "immutable-source-files/agents/ok.md".into(),
                title: " ".into(),
                body: "b".into(),
                tags: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("required"));
        });
    }

    #[test]
    fn status_reports_pending_then_chunked_without_librarian_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
        let ctx = wisdom_ctx(&dir);

        let v = dispatch_wisdom_deposit(
            &ctx,
            WisdomDepositParams {
                path: "immutable-source-files/agents/note-2.md".into(),
                title: "Beta facts".into(),
                body: "Beta is written in Rust.".into(),
                tags: None,
            },
        )
        .await
        .unwrap();
        let path = v["path"].as_str().unwrap();

        let s = dispatch_wisdom_deposit_status(
            &ctx,
            WisdomDepositStatusParams { path: path.into() },
        )
        .await
        .unwrap();
        // No LLM configured in the test env → honest `chunked`/`pending`
        // family, never `ingested`, zero facts.
        let state = s["state"].as_str().unwrap();
        assert!(
            matches!(state, "chunked" | "pending" | "failed" | "no_ingest_host"),
            "honest non-ingested state expected, got {state}"
        );
        assert!(s["facts"].as_array().unwrap().is_empty());
        });
    }

    #[test]
    fn supersession_writes_lane_file_with_token_and_exactly_one_target() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
        let ctx = wisdom_ctx(&dir);

        let err = dispatch_wisdom_propose_supersession(
            &ctx,
            WisdomProposeSupersessionParams {
                target_source_ref: Some("librarian-abc".into()),
                target_fact_id: Some("fact_x".into()),
                replacement_title: "t".into(),
                replacement_body: "b".into(),
                reason: "r".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("exactly one"), "{err}");
        let err = dispatch_wisdom_propose_supersession(
            &ctx,
            WisdomProposeSupersessionParams {
                target_source_ref: None,
                target_fact_id: None,
                replacement_title: "t".into(),
                replacement_body: "b".into(),
                reason: "r".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("exactly one"), "{err}");

        let v = dispatch_wisdom_propose_supersession(
            &ctx,
            WisdomProposeSupersessionParams {
                target_source_ref: Some("librarian-abc".into()),
                target_fact_id: None,
                replacement_title: "Gamma v3".into(),
                replacement_body: "Gamma v3 replaced v2.".into(),
                reason: "stale version".into(),
            },
        )
        .await
        .unwrap();
        let path = v["path"].as_str().unwrap();
        assert!(path.starts_with("immutable-source-files/agents/supersessions/"));
        assert_eq!(v["supersedes"], serde_json::json!("librarian-abc"));
        let file = std::fs::read_to_string(&dir.join(path)).unwrap();
        assert!(file.contains("supersedes: `librarian-abc`"));
        assert!(file.contains("# Gamma v3"));
        });
    }

    #[test]
    fn supersession_by_fact_id_resolves_token() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
        let ctx = wisdom_ctx(&dir);
        ctx.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO llm_wiki_entries (id, entity_id, title, body, source_ref, created_at, updated_at)
                 VALUES ('fact_z', 'ent', 'T', 'B', 'librarian-deadbeef', 0, 0)",
                [],
            )
            .unwrap();
        let v = dispatch_wisdom_propose_supersession(
            &ctx,
            WisdomProposeSupersessionParams {
                target_source_ref: None,
                target_fact_id: Some("fact_z".into()),
                replacement_title: "t".into(),
                replacement_body: "b".into(),
                reason: "r".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(v["supersedes"], serde_json::json!("librarian-deadbeef"));
        });
    }

    #[test]
    fn pending_lists_deposits_without_librarian_rows_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
        let ctx = wisdom_ctx(&dir);
        // Seed: one document WITH librarian evidence, one without.
        ctx.conn.lock().unwrap().execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('immutable-source-files/agents/done.md', 'h1', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        ctx.conn.lock().unwrap().execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('immutable-source-files/agents/waiting.md', 'h2', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        ctx.conn.lock().unwrap().execute(
            "INSERT INTO curated_proposals (id, kind, model, status, created_at)
             VALUES ('p1', 'new_entity', 'm', 'approved', 0)",
            [],
        )
        .unwrap();
        ctx.conn.lock().unwrap().execute(
            "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
             VALUES ('p1', 1, 'evidence')",
            [],
        )
        .unwrap();
        ctx.conn.lock().unwrap().execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, source_ref, created_at, updated_at)
             VALUES ('fact_1', 'ent', 'T', 'B', 'librarian-aabb', 0, 0)",
            [],
        )
        .unwrap();
        ctx.conn.lock().unwrap().execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, unanchored, created_at)
             VALUES ('fact_1', 'p1', '[]', 0, 0)",
            [],
        )
        .unwrap();
        ctx.conn.lock().unwrap().execute(
            "INSERT INTO deposit_kick_state (path, state, updated_ms) VALUES
             ('immutable-source-files/agents/done.md', 'ingested', 0),
             ('immutable-source-files/agents/waiting.md', 'pending', 0)",
            [],
        )
        .unwrap();

        let v = dispatch_wisdom_pending(&ctx).await.unwrap();
        let paths: Vec<&str> = v["pending"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            vec!["immutable-source-files/agents/waiting.md"],
            "evidence-backed deposits must NOT be listed (got {paths:?})"
        );
        });
    }
}
