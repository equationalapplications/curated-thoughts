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
    // now_ms() is milliseconds, so the _millis variant is required: the
    // plain seconds-based `from_timestamp` fed this value would render a
    // year-~56000 stamp.
    let stamp = chrono::DateTime::from_timestamp_millis(now_ms())
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
    //
    // `e.deleted_at IS NULL`: `archive_wisdom` soft-deletes but keeps the
    // evidence row (evidence is only hard-deleted alongside its entry), so an
    // unfiltered join would keep reporting `ingested` for a deposit whose
    // facts have all been archived. No proposal-status filter is needed:
    // `librarian_evidence` rows are written only by `commit_fact_add` inside
    // proposal resolution (same transaction), so evidence implies the
    // proposal was approved.
    let enrich = column_exists(conn, "llm_wiki_entries", "superseded_by")?;
    let sql = if enrich {
        "SELECT e.id, e.tier, e.superseded_by FROM librarian_evidence le
           JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
           JOIN documents d ON d.id = ps.doc_id
           JOIN llm_wiki_entries e ON e.id = le.entry_id AND e.deleted_at IS NULL
          WHERE d.path = ?1"
    } else {
        "SELECT e.id, e.tier, NULL FROM librarian_evidence le
           JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
           JOIN documents d ON d.id = ps.doc_id
           JOIN llm_wiki_entries e ON e.id = le.entry_id AND e.deleted_at IS NULL
          WHERE d.path = ?1"
    };
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| anyhow!("status probe failed: {e}"))?;
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

    // Live librarian evidence outranks the kick ledger: a recorded 'failed'
    // from a transient kick error must not permanently misreport a deposit
    // the Librarian actually ingested (Opus impl-review nit 2). The inverse
    // holds too: an 'ingested' ledger row must NOT read as recall presence
    // once every fact backed by this deposit has been archived. The one
    // exception is the supersession lane, whose deposits drain to zero facts
    // BY DESIGN (V25: summarize only): its terminal ledger state still reads
    // `pending` — application is the Active Librarian's follow-up spec (D5),
    // so no supersession is 'ingested' until that lands. The raw ledger
    // state stays visible in the `kick` field.
    let supersession_lane = vault_relative_path.starts_with(&format!("{SUPERSESSIONS_DIR}/"));
    let state = match (&kick, facts.is_empty(), chunked > 0) {
        (_, false, _) => "ingested".to_string(),
        (Some((s, _, _)), _, _) if s == "ingested" && supersession_lane => "pending".to_string(),
        (Some((s, _, _)), _, _)
            if s != "pending"
                && s != "queued_watcher"
                && s != "no_ingest_host"
                && s != "failed"
                && s != "ingested" =>
        {
            s.clone()
        }
        (_, true, true) => "chunked".to_string(),
        (Some((s, _, _)), _, _)
            if matches!(
                s.as_str(),
                "pending" | "queued_watcher" | "no_ingest_host" | "failed"
            ) =>
        {
            s.clone()
        }
        (_, _, _) => "pending".to_string(),
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
/// Bound on lock-contention retries. `VaultLock::acquire` is a try-lock;
/// depositing while `ct watch` is mid-pass is transient contention (the pass
/// releases the lock when it finishes), so we wait-and-retry before deciding
/// the kick failed. Deliberately short: a deposit must resolve quickly.
const KICK_LOCK_RETRIES: usize = 5;
const KICK_LOCK_RETRY_MS: u64 = 500;

/// Serializes kicks within this process. Without it, two back-to-back
/// deposits from one sidecar contend on the (per-open-file) vault lock with
/// each other, and the loser would misreport itself as `queued_watcher` when
/// no watcher is running. Cross-process contention still reaches the
/// VaultLock retry loop below.
static KICK_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_kick(
    db_path: PathBuf,
    vault_dir: PathBuf,
    rel_path: String,
    profile: crate::embedder::EmbedProfile,
) {
    let _serial = KICK_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let outcome = (|| -> Result<String> {
        // Retry only on genuine contention ("already locked"), not on
        // real acquisition errors (permissions etc.) — those fail fast.
        let mut lock = Err(anyhow!("vault lock: not attempted"));
        for attempt in 0..=KICK_LOCK_RETRIES {
            lock = crate::watcher::VaultLock::acquire(&brain_dir_of(&db_path));
            match &lock {
                Ok(_) => break,
                Err(e) => {
                    let contended = format!("{e:#}").contains("already locked");
                    if !contended {
                        return Err(anyhow!("vault lock: {e:#}"));
                    }
                    if attempt == KICK_LOCK_RETRIES {
                        // Still held by another process (`ct watch` / the app
                        // worker): its file event owns this deposit (spec D3).
                        // Not a failure.
                        let conn = rusqlite::Connection::open_with_flags(
                            &db_path,
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                        )
                        .with_context(|| format!("open brain {}", db_path.display()))?;
                        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
                        set_kick_state(&conn, &rel_path, "queued_watcher", None)?;
                        return Ok("queued_watcher".to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(KICK_LOCK_RETRY_MS));
                }
            }
        }
        let _lock = lock?;
        let mut conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .with_context(|| format!("open brain {}", db_path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();

        // `rel_path` stays the document key (status probe, V25 folder rules);
        // the bytes are read from the vault, not the process CWD.
        let read_path = vault_dir.join(&rel_path).to_string_lossy().into_owned();
        crate::pipeline::ingest_document_virtual(
            &conn,
            &profile,
            &rel_path,
            &read_path,
            false,
            Some(vault_dir.to_string_lossy().as_ref()),
        )
        .map_err(|e| anyhow!("ingest: {e:#}"))?;
        set_kick_state(&conn, &rel_path, "chunked", None)?;

        // Librarian leg. `generate_summary` consults folder rules itself
        // (agents = synthesize + auto_approve after V25); with no configured
        // generation provider it errors — that's `chunked` (no librarian
        // host), not `failed`. Config is re-checked HERE rather than passed
        // in from `start_kick` so the ledger decision always matches the
        // config this kick actually runs under.
        if !crate::librarian::llm_generation_configured() {
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
        // No-create open (with_rw contract): a vanished brain must not be
        // resurrected as an empty database just to record a kick outcome.
        if let Ok(conn) = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        ) {
            let _ = set_kick_state(&conn, &rel_path, "failed", Some(&format!("{err:#}")));
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatchers
// ---------------------------------------------------------------------------

type KickHandle = tokio::task::JoinHandle<()>;

/// Record the initial ledger state and spawn the kick when a generation host
/// is configured. Returns the reply label (`started` | `no_ingest_host`) and
/// the kick's handle. The ledger write goes through with_rw: ctx.conn is
/// READ-ONLY in the MCP sidecar.
async fn start_kick(
    ctx: &ToolDispatchContext,
    vault: &std::path::Path,
    rel: &str,
) -> Result<(&'static str, Option<KickHandle>)> {
    let generation_configured = crate::librarian::llm_generation_configured();
    let initial = if generation_configured {
        "pending"
    } else {
        "no_ingest_host"
    };
    let kick_rel = rel.to_string();
    ctx.with_rw(move |conn| set_kick_state(conn, &kick_rel, initial, None))
        .await
        .map_err(|e| anyhow!("kick-state write failed: {e:#}"))?;
    if !generation_configured {
        return Ok(("no_ingest_host", None));
    }
    let (db_path, vault, rel, profile) = (
        ctx.db_path.clone(),
        vault.to_path_buf(),
        rel.to_string(),
        ctx.profile.clone(),
    );
    let handle = tokio::task::spawn_blocking(move || run_kick(db_path, vault, rel, profile));
    Ok(("started", Some(handle)))
}

/// Await a kick; an aborted/panicked kick must not strand the ledger at
/// 'pending' with no trace (Opus impl-review nit 4).
async fn observe_kick(handle: KickHandle, db_path: PathBuf, rel: String) {
    if let Err(join_err) = handle.await {
        if let Ok(conn) = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        ) {
            let _ = set_kick_state(
                &conn,
                &rel,
                "failed",
                Some(&format!("kick task aborted: {join_err}")),
            );
        }
    }
}

fn reply_path(v: &Value) -> String {
    v["path"].as_str().unwrap_or_default().to_string()
}

fn detach_kick(ctx: &ToolDispatchContext, v: &Value, handle: Option<KickHandle>) {
    if let Some(h) = handle {
        tokio::spawn(observe_kick(h, ctx.db_path.clone(), reply_path(v)));
    }
}

async fn await_kick(ctx: &ToolDispatchContext, v: &Value, handle: Option<KickHandle>) {
    if let Some(h) = handle {
        observe_kick(h, ctx.db_path.clone(), reply_path(v)).await;
    }
}

fn vault_dir(ctx: &ToolDispatchContext) -> Result<PathBuf> {
    ctx.vault_dir
        .clone()
        .ok_or_else(|| anyhow!("no vault configured (vault_dir unset)"))
}

/// Deposit (MCP surface): the kick runs in the background, observed so an
/// aborted task still lands in the ledger.
pub async fn dispatch_wisdom_deposit(
    ctx: &ToolDispatchContext,
    p: WisdomDepositParams,
) -> Result<Value> {
    let (v, handle) = deposit_inner(ctx, p).await?;
    detach_kick(ctx, &v, handle);
    Ok(v)
}

/// Deposit, then wait for the kick to finish. For short-lived callers (`ct`)
/// whose runtime is dropped on return: a queued `spawn_blocking` kick may
/// never start during runtime shutdown, stranding the ledger at `pending`.
pub async fn dispatch_wisdom_deposit_awaiting_kick(
    ctx: &ToolDispatchContext,
    p: WisdomDepositParams,
) -> Result<Value> {
    let (v, handle) = deposit_inner(ctx, p).await?;
    await_kick(ctx, &v, handle).await;
    Ok(v)
}

async fn deposit_inner(
    ctx: &ToolDispatchContext,
    p: WisdomDepositParams,
) -> Result<(Value, Option<KickHandle>)> {
    let vault = vault_dir(ctx)?;
    let rel = p.path.replace('\\', "/");
    // Delimiter-anchored prefix: `{AGENTS_DIR}/` exactly (the constant is the
    // full lane path `immutable-source-files/agents`, so the check carries
    // the `immutable-source-files/` parent too) — a byte-prefix check would
    // admit sibling directories like `agents-archive/` (safe via safe_path's
    // canonical containment, but the wrong error class). Canonical
    // containment in safe_vault_path remains the decisive guard; this check
    // just routes misuse to the clearest message.
    let in_agents = rel.starts_with(&format!("{AGENTS_DIR}/"));
    let in_supersessions = rel.starts_with(&format!("{SUPERSESSIONS_DIR}/"));
    if !in_agents || in_supersessions {
        bail!(
            "deposit path must be under {AGENTS_DIR}/ (not {SUPERSESSIONS_DIR}/): {}",
            p.path
        );
    }
    if p.title.trim().is_empty() || p.body.trim().is_empty() {
        bail!("title and body are required");
    }
    let target = vault.join(&rel);
    if target.symlink_metadata().is_ok() {
        bail!("deposit_exists: {} (append-only; supersede instead)", rel);
    }

    let content = render_deposit_file(&p.title, &p.body, p.tags.as_deref().unwrap_or(&[]));
    // The sanctioned lane directory must exist before validation:
    // safe_vault_path's MayCreate canonicalizes the parent and reports
    // "parent directory not found" for a fresh vault (CI caught this —
    // immutable-source-files/agents/ does not exist yet there). Creating the
    // lane dir itself is safe: it is a fixed constant, not user input.
    std::fs::create_dir_all(vault.join(AGENTS_DIR))?;
    // A deposit may live deeper than the lane dir (e.g. agents/topic/note.md).
    // Those intermediate dirs must ALSO exist before validation, for the same
    // MayCreate reason. They are user input, so: plain names only (no `..`,
    // no root — nothing may be created outside the lane before safe_vault_path
    // runs), created one component at a time refusing symlinked components.
    let sub_parent = std::path::Path::new(&rel)
        .strip_prefix(AGENTS_DIR)
        .ok()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| std::path::Path::new(""));
    if !sub_parent
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        bail!("unsafe deposit path: traversal component in {}", p.path);
    }
    crate::okf::write::create_parents_no_symlink(&vault.join(AGENTS_DIR), sub_parent)
        .map_err(|e| anyhow!("unsafe deposit path: {e}"))?;
    // Validate, then write to the *validated* path (not the raw join):
    // safe_vault_path canonicalizes every parent and rejects symlinked or
    // traversal parents (fail-closed), so its return value is the only path
    // bytes may land on.
    let validated = crate::vault::safe_path::safe_vault_path(
        &vault,
        &rel,
        &[AGENTS_DIR],
        crate::vault::safe_path::PathMode::MayCreate,
    )
    .map_err(|e| anyhow!("unsafe deposit path: {e}"))?;
    // Exclusive create closes the pre-check/rename TOCTOU: two concurrent
    // same-path deposits can no longer both pass and silently overwrite each
    // other (append-only, INTENT rule 9). temp+rename replace would.
    let write_result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&validated)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(content.as_bytes())?;
            f.sync_all()
        });
    match write_result {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("deposit_exists: {} (append-only; supersede instead)", rel)
        }
        Err(e) => return Err(anyhow!("deposit write failed: {e}")),
    }

    // Audit (fail-closed, RW connection, same contract as the removed
    // curated write tools) — `wisdom_` joins the curated audit class.
    //
    // The file write above cannot share the audit's transaction, so a failed
    // audit — or kick-ledger write below — compensates by removing the
    // just-created file: the caller must be able to retry the same path
    // instead of wedging on `deposit_exists` forever.
    let audit_path = rel.clone();
    let client = ctx.client.clone();
    if let Err(e) = ctx
        .with_rw(move |conn| {
            crate::tool_dispatch::log_agent_access_checked(
                conn,
                &client,
                "wisdom_deposit",
                Some(audit_path.as_str()),
                "write",
            )
        })
        .await
    {
        let _ = std::fs::remove_file(&validated);
        return Err(e);
    }

    // Kick (D3): full pipeline, lock-serialized. The ledger row is recorded
    // before the spawn so a crash in between leaves an honest trace.
    let (kick_label, handle) = match start_kick(ctx, &vault, &rel).await {
        Ok(ok) => ok,
        Err(e) => {
            let _ = std::fs::remove_file(&validated);
            return Err(e);
        }
    };

    Ok((
        json!({
            "path": rel,
            "pending": true,
            "kick": kick_label,
        }),
        handle,
    ))
}

pub async fn dispatch_wisdom_deposit_status(
    ctx: &ToolDispatchContext,
    p: WisdomDepositStatusParams,
) -> Result<Value> {
    let rel = p.path.replace('\\', "/");
    if !rel.starts_with(&format!("{AGENTS_DIR}/")) {
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
    let conn = ctx
        .conn
        .lock()
        .map_err(|_| anyhow!("conn mutex poisoned"))?;
    deposit_status_row(&conn, &rel)
}

pub async fn dispatch_wisdom_pending(ctx: &ToolDispatchContext) -> Result<Value> {
    let client = ctx.client.clone();
    ctx.with_rw(move |conn| {
        crate::tool_dispatch::log_agent_access_checked(
            conn,
            &client,
            "wisdom_pending",
            None,
            "read",
        )
    })
    .await?;
    let conn = ctx
        .conn
        .lock()
        .map_err(|_| anyhow!("conn mutex poisoned"))?;
    // D6: deposits without LIVE librarian evidence — including `chunked`,
    // deferred supersession deposits, and deposits whose facts were later
    // archived (the same `deleted_at IS NULL` filter as the status probe, so
    // the two views of "in recall" cannot disagree). Structural isolation
    // from recall: separate tool, separate code path.
    let mut stmt = conn
        .prepare(
            "SELECT path FROM deposit_kick_state
              WHERE path NOT IN (
                SELECT d.path FROM librarian_evidence le
                  JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
                  JOIN documents d ON d.id = ps.doc_id
                  JOIN llm_wiki_entries e ON e.id = le.entry_id AND e.deleted_at IS NULL
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
        // mtime comes from the vault root only: with no configured
        // vault_dir the fallback must NOT stat vault-relative paths against
        // the process CWD (a sibling file there would answer with a real
        // mtime from the wrong tree).
        let mtime = ctx.vault_dir.as_deref().and_then(|root| {
            std::fs::symlink_metadata(root.join(&p))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
        });
        items.push(json!({ "path": p, "deposited_at": mtime }));
    }
    Ok(json!({ "pending": items }))
}

/// Supersession (MCP surface): background kick, see [`dispatch_wisdom_deposit`].
pub async fn dispatch_wisdom_propose_supersession(
    ctx: &ToolDispatchContext,
    p: WisdomProposeSupersessionParams,
) -> Result<Value> {
    let (v, handle) = supersession_inner(ctx, p).await?;
    detach_kick(ctx, &v, handle);
    Ok(v)
}

/// Supersession, then wait for the kick (see
/// [`dispatch_wisdom_deposit_awaiting_kick`]).
pub async fn dispatch_wisdom_propose_supersession_awaiting_kick(
    ctx: &ToolDispatchContext,
    p: WisdomProposeSupersessionParams,
) -> Result<Value> {
    let (v, handle) = supersession_inner(ctx, p).await?;
    await_kick(ctx, &v, handle).await;
    Ok(v)
}

async fn supersession_inner(
    ctx: &ToolDispatchContext,
    p: WisdomProposeSupersessionParams,
) -> Result<(Value, Option<KickHandle>)> {
    let target = match (p.target_source_ref.as_deref(), p.target_fact_id.as_deref()) {
        (Some(_), Some(_)) => bail!("supply exactly one of target_source_ref / target_fact_id"),
        (None, None) => bail!("supply exactly one of target_source_ref / target_fact_id"),
        // Shape-check only (plan: `^librarian-[0-9a-f]{32}$`): application is
        // deferred to the Active Librarian, so a valid-shaped token with no
        // live row today is NOT rejected here — the file must still land.
        (Some(r), None) => {
            if !crate::db::commit::is_librarian_source_ref_token(r) {
                bail!("target_source_ref must match ^librarian-[0-9a-f]{{32}}$");
            }
            r.to_string()
        }
        (None, Some(id)) => {
            // Resolve the brain id to its librarian token (supersession files
            // carry the stable token; the Librarian's application spec keys
            // on it). Archived (soft-deleted) facts are excluded: their
            // tokens no longer resolve to live entries downstream, so a
            // supersession file naming one would dead-end the reconcile.
            let conn = ctx
                .conn
                .lock()
                .map_err(|_| anyhow!("conn mutex poisoned"))?;
            let token: Option<String> = conn
                .query_row(
                    "SELECT source_ref FROM llm_wiki_entries
                      WHERE id = ?1 AND deleted_at IS NULL",
                    [id],
                    |r| r.get(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
                .map_err(|e| anyhow!("target lookup failed: {e}"))?;
            token.ok_or_else(|| anyhow!("target_fact_id not found or archived: {id}"))?
        }
    };

    let vault = vault_dir(ctx)?;
    let content = render_supersession_file(
        &target,
        &p.replacement_title,
        &p.replacement_body,
        &p.reason,
    );
    // Lane dir first (fixed constant, not user input), then validate, then
    // exclusive-create on the validated path — same contract as deposits.
    std::fs::create_dir_all(vault.join(SUPERSESSIONS_DIR))?;

    // The filename is server-generated (`supersession-{now_ms}.md`): two
    // proposals inside one millisecond would collide on the exclusive
    // create, and the second caller would get a bogus `deposit_exists` for a
    // path it never chose. Retry with a fresh stamp plus a sequence suffix
    // instead — the real collision guard stays the exclusive create.
    let mut landed: Option<(String, PathBuf)> = None;
    for attempt in 0..8u32 {
        let stamp = now_ms();
        let candidate = if attempt == 0 {
            format!("{SUPERSESSIONS_DIR}/supersession-{stamp}.md")
        } else {
            format!("{SUPERSESSIONS_DIR}/supersession-{stamp}-{attempt}.md")
        };
        let validated = crate::vault::safe_path::safe_vault_path(
            &vault,
            &candidate,
            &[SUPERSESSIONS_DIR],
            crate::vault::safe_path::PathMode::MayCreate,
        )
        .map_err(|e| anyhow!("unsafe supersession path: {e}"))?;
        let write_result = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&validated)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(content.as_bytes())?;
                f.sync_all()
            });
        match write_result {
            Ok(()) => {
                landed = Some((candidate, validated));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(anyhow!("supersession write failed: {e}")),
        }
    }
    let (rel, validated) = landed
        .ok_or_else(|| anyhow!("supersession filename collision: 8 attempts in one millisecond"))?;

    // Audit, with the same compensating file removal as the deposit lane: a
    // failed audit (or kick-ledger write below) must leave the caller able
    // to retry rather than wedged on a path it never chose.
    let audit_path = rel.clone();
    let client = ctx.client.clone();
    if let Err(e) = ctx
        .with_rw(move |conn| {
            crate::tool_dispatch::log_agent_access_checked(
                conn,
                &client,
                "wisdom_propose_supersession",
                Some(audit_path.as_str()),
                "write",
            )
        })
        .await
    {
        let _ = std::fs::remove_file(&validated);
        return Err(e);
    }

    // Kick: supersessions/ is `summarize` — the file ingests and drains with
    // zero facts (V25 override). Librarian APPLICATION of the supersession is
    // the follow-up reconcile spec (rule 4 mechanics); until then status
    // stays pending. Same kick contract as the deposit lane, so the reply
    // label and the ledger always agree.
    let (kick_label, handle) = match start_kick(ctx, &vault, &rel).await {
        Ok(ok) => ok,
        Err(e) => {
            let _ = std::fs::remove_file(&validated);
            return Err(e);
        }
    };

    Ok((
        json!({
            "path": rel,
            "supersedes": target,
            "pending": true,
            "kick": kick_label,
        }),
        handle,
    ))
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
            let file = std::fs::read_to_string(dir.join("immutable-source-files/agents/note-1.md"))
                .unwrap();
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
    fn deposit_rejects_agents_sibling_dirs_with_lane_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let err = dispatch_wisdom_deposit(
                &ctx,
                WisdomDepositParams {
                    path: "immutable-source-files/agents-archive/note.md".into(),
                    title: "x".into(),
                    body: "y".into(),
                    tags: None,
                },
            )
            .await
            .unwrap_err();
            assert!(
                err.to_string().contains("deposit path must be under"),
                "lane error, got: {err}"
            );
        });
    }

    #[test]
    fn status_failed_kick_does_not_latch_over_live_evidence() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            // Deposit (kick will fail or stall without generation, fine).
            let rel = "immutable-source-files/agents/evidence-note.md";
            dispatch_wisdom_deposit(
                &ctx,
                WisdomDepositParams {
                    path: rel.into(),
                    title: "Evidence note".into(),
                    body: "body".into(),
                    tags: None,
                },
            )
            .await
            .unwrap();
            // Simulate a transient kick failure stamped in the ledger...
            {
                let conn = ctx.conn.lock().unwrap();
                set_kick_state(&conn, rel, "failed", Some("synthetic failure")).unwrap();
            }
            // ...then the Librarian actually landing evidence for the doc.
            {
                let conn = ctx.conn.lock().unwrap();
                conn.execute_batch(
                    "INSERT INTO llm_wiki_entries (
                         id, entity_id, title, body, tags, confidence, source_type,
                         source_hash, source_ref, created_at, updated_at,
                         last_accessed_at, access_count, deleted_at,
                         embedding_blob, embedding
                     ) VALUES ('e1', 'ent1', 't', 'b', '[]', 'inferred',
                               'librarian_inferred', NULL,
                               'librarian-abc123def456abc123def456abc12345',
                               100, 100, NULL, 0, NULL, NULL, NULL);
                     INSERT INTO documents (path, hash, tier, status) VALUES
                         ('immutable-source-files/agents/evidence-note.md', 'h1',
                          'user_doc', 'indexed');
                     INSERT INTO librarian_evidence
                         (entry_id, proposal_id, evidence_json, created_at)
                         VALUES ('e1', 'p1',
                                 '{\"quote\":\"q\",\"source_kind\":\"document\"}', 0);
                     INSERT INTO curated_proposals (id, kind, model, status, created_at)
                         VALUES ('p1', 'new_entity', 'test', 'pending', 0);
                     INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
                         VALUES ('p1', (SELECT id FROM documents LIMIT 1), 'evidence');",
                )
                .unwrap();
            }
            let v = dispatch_wisdom_deposit_status(
                &ctx,
                WisdomDepositStatusParams { path: rel.into() },
            )
            .await
            .unwrap();
            assert_eq!(
                v["state"],
                serde_json::json!("ingested"),
                "live evidence outranks the failed ledger row: {v}"
            );
        });
    }

    /// Spec D5: until the Librarian application spec lands, a supersession
    /// file's status reads `pending` even after its kick reached `ingested`
    /// — the raw ledger state stays visible in the `kick` field.
    #[test]
    fn status_supersession_lane_stays_pending_after_ingested_kick() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let v = dispatch_wisdom_propose_supersession(
                &ctx,
                WisdomProposeSupersessionParams {
                    target_source_ref: Some("librarian-abc123def456abc123def456abc12345".into()),
                    target_fact_id: None,
                    replacement_title: "t".into(),
                    replacement_body: "b".into(),
                    reason: "r".into(),
                },
            )
            .await
            .unwrap();
            let rel = v["path"].as_str().unwrap().to_string();
            // Simulate a fully-drained kick (V25 folder rule: zero facts, so
            // only the ledger can carry this state).
            {
                let conn = ctx.conn.lock().unwrap();
                set_kick_state(&conn, &rel, "ingested", None).unwrap();
            }
            let s = dispatch_wisdom_deposit_status(
                &ctx,
                WisdomDepositStatusParams { path: rel.clone() },
            )
            .await
            .unwrap();
            assert_eq!(s["state"], serde_json::json!("pending"), "{s}");
            assert_eq!(s["kick"], serde_json::json!("ingested"), "{s}");
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
                assert!(err.to_string().contains("must be under"), "{bad}: {err}");
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
                    target_source_ref: Some("librarian-abc123def456abc123def456abc12345".into()),
                    target_fact_id: Some("fact_x".into()),
                    replacement_title: "t".into(),
                    replacement_body: "b".into(),
                    reason: "r".into(),
                },
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("exactly one"), "{err}");
            // Malformed tokens are refused before any file lands (plan:
            // `^librarian-[0-9a-f]{32}$`; readers classify librarian refs by
            // this exact shape).
            for bad in [
                "librarian-abc",
                "librarian-notes.md",
                "librarian-ABC123DEF456ABC123DEF456ABC12345",
                "",
            ] {
                let err = dispatch_wisdom_propose_supersession(
                    &ctx,
                    WisdomProposeSupersessionParams {
                        target_source_ref: Some(bad.into()),
                        target_fact_id: None,
                        replacement_title: "t".into(),
                        replacement_body: "b".into(),
                        reason: "r".into(),
                    },
                )
                .await
                .unwrap_err();
                assert!(
                    err.to_string().contains("target_source_ref must match"),
                    "bad token {bad:?} refused: {err}"
                );
            }
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
                    target_source_ref: Some("librarian-abc123def456abc123def456abc12345".into()),
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
            assert_eq!(
                v["supersedes"],
                serde_json::json!("librarian-abc123def456abc123def456abc12345")
            );
            let file = std::fs::read_to_string(dir.join(path)).unwrap();
            assert!(file.contains("supersedes: `librarian-abc123def456abc123def456abc12345`"));
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

    /// Seeded WITHOUT a dispatch call on purpose: a detached kick runs on the
    /// blocking pool concurrently with the test body, so the ledger state
    /// must be hand-set here to stay deterministic.
    #[test]
    fn archived_facts_evidence_does_not_read_as_ingested() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let rel = "immutable-source-files/agents/arch-note.md";
            // Librarian lands a live fact for the doc (approved proposal —
            // the only production evidence writer runs inside resolution),
            // the doc is chunked, and the ledger records the completed pass.
            {
                let conn = ctx.conn.lock().unwrap();
                conn.execute_batch(
                    "INSERT INTO llm_wiki_entries (
                         id, entity_id, title, body, tags, confidence, source_type,
                         source_hash, source_ref, created_at, updated_at,
                         last_accessed_at, access_count, deleted_at,
                         embedding_blob, embedding
                     ) VALUES ('e1', 'ent1', 't', 'b', '[]', 'inferred',
                               'librarian_inferred', NULL,
                               'librarian-abc123def456abc123def456abc12345',
                               100, 100, NULL, 0, NULL, NULL, NULL);
                     INSERT INTO documents (path, hash, tier, status) VALUES
                         ('immutable-source-files/agents/arch-note.md', 'h1',
                          'user_doc', 'indexed');
                     INSERT INTO chunks (doc_id, chunk_text, position)
                         SELECT id, 'body text', 0 FROM documents
                          WHERE path = 'immutable-source-files/agents/arch-note.md';
                     INSERT INTO librarian_evidence
                         (entry_id, proposal_id, evidence_json, created_at)
                         VALUES ('e1', 'p1',
                                 '{\"quote\":\"q\",\"source_kind\":\"document\"}', 0);
                     INSERT INTO curated_proposals (id, kind, model, status, created_at)
                         VALUES ('p1', 'new_entity', 'test', 'approved', 0);
                     INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
                         VALUES ('p1', (SELECT id FROM documents LIMIT 1), 'evidence');",
                )
                .unwrap();
                set_kick_state(&conn, rel, "ingested", None).unwrap();
            }
            let v = dispatch_wisdom_deposit_status(
                &ctx,
                WisdomDepositStatusParams { path: rel.into() },
            )
            .await
            .unwrap();
            assert_eq!(v["state"], serde_json::json!("ingested"), "{v}");

            // ...then the human archives the fact. archive_wisdom soft-deletes
            // (deleted_at set) but the evidence row survives — live-evidence
            // truth must win over both the join and the terminal ledger row.
            ctx.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'e1'",
                    [],
                )
                .unwrap();
            let v = dispatch_wisdom_deposit_status(
                &ctx,
                WisdomDepositStatusParams { path: rel.into() },
            )
            .await
            .unwrap();
            assert_eq!(
                v["state"],
                serde_json::json!("chunked"),
                "archived facts must not read as recall presence: {v}"
            );
            assert!(v["facts"].as_array().unwrap().is_empty(), "{v}");
            // And the pending listing agrees with the probe.
            let p = dispatch_wisdom_pending(&ctx).await.unwrap();
            let paths: Vec<&str> = p["pending"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["path"].as_str().unwrap())
                .collect();
            assert!(
                paths.contains(&rel),
                "pending must re-list the archived deposit: {paths:?}"
            );
        });
    }

    #[test]
    fn supersession_by_fact_id_rejects_archived_facts() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            ctx.conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO llm_wiki_entries (id, entity_id, title, body, source_ref,
                         created_at, updated_at, deleted_at)
                     VALUES ('fact_arch', 'ent', 'T', 'B', 'librarian-archived', 0, 0, 1)",
                    [],
                )
                .unwrap();
            let err = dispatch_wisdom_propose_supersession(
                &ctx,
                WisdomProposeSupersessionParams {
                    target_source_ref: None,
                    target_fact_id: Some("fact_arch".into()),
                    replacement_title: "t".into(),
                    replacement_body: "b".into(),
                    reason: "r".into(),
                },
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("not found or archived"), "{err}");
        });
    }

    #[test]
    fn supersession_same_millisecond_collisions_retry_to_distinct_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let mut paths = Vec::new();
            for _ in 0..5 {
                let v = dispatch_wisdom_propose_supersession(
                    &ctx,
                    WisdomProposeSupersessionParams {
                        target_source_ref: Some(
                            "librarian-abc123def456abc123def456abc12345".into(),
                        ),
                        target_fact_id: None,
                        replacement_title: "t".into(),
                        replacement_body: "b".into(),
                        reason: "r".into(),
                    },
                )
                .await
                .unwrap();
                paths.push(v["path"].as_str().unwrap().to_string());
            }
            let unique: std::collections::HashSet<&String> = paths.iter().collect();
            assert_eq!(
                unique.len(),
                paths.len(),
                "back-to-back proposals must never collide into deposit_exists: {paths:?}"
            );
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
            ctx.conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO documents (path, hash, tier, status)
             VALUES ('immutable-source-files/agents/done.md', 'h1', 'user_doc', 'indexed')",
                    [],
                )
                .unwrap();
            ctx.conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO documents (path, hash, tier, status)
             VALUES ('immutable-source-files/agents/waiting.md', 'h2', 'user_doc', 'indexed')",
                    [],
                )
                .unwrap();
            ctx.conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO curated_proposals (id, kind, model, status, created_at)
             VALUES ('p1', 'new_entity', 'm', 'approved', 0)",
                    [],
                )
                .unwrap();
            ctx.conn
                .lock()
                .unwrap()
                .execute(
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
            ctx.conn
                .lock()
                .unwrap()
                .execute(
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

    #[test]
    fn deposit_creates_nested_topic_dirs_under_the_lane() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let v = dispatch_wisdom_deposit(
                &ctx,
                WisdomDepositParams {
                    path: "immutable-source-files/agents/topic/sub/note.md".into(),
                    title: "Nested".into(),
                    body: "Nested deposits land in fresh topic dirs.".into(),
                    tags: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(v["pending"], serde_json::json!(true));
            assert!(dir
                .join("immutable-source-files/agents/topic/sub/note.md")
                .is_file());
        });
    }

    #[test]
    fn deposit_traversal_creates_nothing_outside_the_lane() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let err = dispatch_wisdom_deposit(
                &ctx,
                WisdomDepositParams {
                    path: "immutable-source-files/agents/../../escaped/note.md".into(),
                    title: "t".into(),
                    body: "b".into(),
                    tags: None,
                },
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("unsafe deposit path"), "{err}");
            assert!(
                !dir.join("escaped").exists(),
                "no dir may be created outside the lane"
            );
        });
    }

    #[test]
    fn supersession_stamp_is_a_current_utc_date() {
        let text = render_supersession_file("librarian-x", "T", "B", "r");
        let year = chrono::Utc::now().format("%Y").to_string();
        assert!(
            text.contains(&format!("- deposited_at: {year}-")),
            "stamp must be this year's ISO date, got:\n{text}"
        );
    }

    #[test]
    fn supersession_ledger_matches_reply_label() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let v = dispatch_wisdom_propose_supersession_awaiting_kick(
                &ctx,
                WisdomProposeSupersessionParams {
                    target_source_ref: Some("librarian-abc123def456abc123def456abc12345".into()),
                    target_fact_id: None,
                    replacement_title: "t".into(),
                    replacement_body: "b".into(),
                    reason: "r".into(),
                },
            )
            .await
            .unwrap();
            if v["kick"] == serde_json::json!("no_ingest_host") {
                let conn = ctx.conn.lock().unwrap();
                let state: String = conn
                    .query_row(
                        "SELECT state FROM deposit_kick_state WHERE path = ?1",
                        [v["path"].as_str().unwrap()],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(state, "no_ingest_host");
            }
        });
    }

    /// The kick reads the deposit from the vault, not the process CWD (cargo
    /// runs tests from the crate dir, never the tempdir vault).
    #[test]
    fn kick_ingests_from_vault_not_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let rel = "immutable-source-files/agents/kick-cwd.md";
            assert_ne!(std::env::current_dir().unwrap(), dir);
            std::fs::write(dir.join(rel), "# Kick\n\nRead from the vault root.\n").unwrap();
            run_kick(
                ctx.db_path.clone(),
                dir.clone(),
                rel.to_string(),
                ctx.profile.clone(),
            );
            let conn = ctx.conn.lock().unwrap();
            let (state, error): (String, Option<String>) = conn
                .query_row(
                    "SELECT state, error FROM deposit_kick_state WHERE path = ?1",
                    [rel],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(state, "chunked", "kick error: {error:?}");
        });
    }
}
