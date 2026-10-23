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
/// `SUPERSESSIONS_DIR` with its trailing separator, so a prefix test cannot
/// match a sibling directory — the same property
/// [`crate::vault::safe_path::AGENTS_DEPOSIT_PREFIX`] already guarantees for
/// the deposit lane; prefix checks here reuse that constant directly instead
/// of re-deriving `format!("{AGENTS_DIR}/")` at every call site.
const SUPERSESSIONS_PREFIX: &str = "immutable-source-files/agents/supersessions/";

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
    /// Optional attribution carrier (Task 1, spec rev 4.1 §1): rendered as an
    /// OKF frontmatter block and indexed at deposit time. `None` renders
    /// byte-identically to the pre-attribution format.
    #[serde(default)]
    pub attribution: Option<DepositAttribution>,
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
// Attribution carrier (user-confirmation-queue Task 1, spec rev 4.1 §1/D4)
// ---------------------------------------------------------------------------

/// Who said what a deposit records, supplied by the calling agent at deposit
/// time and rendered into the note's OKF frontmatter — the FILE is the
/// authoritative carrier; the brain-side `deposit_attribution` row is a
/// derived cache keyed by document path. The agent may attribute only what
/// Kurt said (`Verbatim`) or faithfully restated at his direction
/// (`RestatedAtDirection`) — never its own paraphrase or librarian output
/// (S2). The validation here checks SHAPE only (speaker non-empty, source
/// handle form); S2 itself is NOT machine-enforced — attribution is
/// agent-asserted and its mitigation is the human confirm step
/// (comment corrected, Tier-1.5 xhigh MINOR-8).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct DepositAttribution {
    /// Speaker handle (e.g. `kurt`).
    pub speaker: String,
    /// Vocabulary kind — see [`AttributionKind`].
    pub kind: AttributionKind,
    /// Source handle for the attribution, e.g. `discord:1562137256498569278`.
    /// Validated against [`validate_handle`] — these values are rendered into
    /// YAML frontmatter and parsed back, so free text is rejected at the door.
    pub source: String,
}

/// Attribution vocabulary: `verbatim` and `restated_at_direction` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AttributionKind {
    /// Kurt said exactly this.
    Verbatim,
    /// The agent restated this at Kurt's direction.
    RestatedAtDirection,
}

impl AttributionKind {
    /// Frontmatter token (the pinned kebab-lowercase vocabulary).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Verbatim => "verbatim",
            Self::RestatedAtDirection => "restated_at_direction",
        }
    }
}

impl TryFrom<&str> for AttributionKind {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "verbatim" => Ok(Self::Verbatim),
            "restated_at_direction" => Ok(Self::RestatedAtDirection),
            other => Err(format!(
                "unknown attribution kind {other:?} (expected \"verbatim\" or \
                 \"restated_at_direction\")"
            )),
        }
    }
}

/// Handle values (`attribution_source`, and later `approval_instruction_source`)
/// are rendered into YAML frontmatter and parsed back, so they must match
/// `^[a-z]+:[A-Za-z0-9_.-]+$` — free text is rejected at the door (rev 4 m8).
pub fn validate_handle(handle: &str) -> Result<(), String> {
    let ok = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-');
    match handle.split_once(':') {
        Some((scheme, rest))
            if !scheme.is_empty()
                && scheme.bytes().all(|b| b.is_ascii_lowercase())
                && !rest.is_empty()
                && rest.bytes().all(ok) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "invalid handle {handle:?} (must match ^[a-z]+:[A-Za-z0-9_.-]+$)"
        )),
    }
}

// ---------------------------------------------------------------------------
// Deposit file rendering
// ---------------------------------------------------------------------------

/// Titles, tags, and reasons are single-line slots in the rendered file
/// (H1, inline-code list item, bullet). A newline inside one would inject
/// structure — a second `# ` heading, or a line the supersession readers
/// parse as file framing rather than content — so any line break is
/// collapsed to a space before interpolation. Only `body` is multi-line.
fn flatten_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

fn render_deposit_file(
    title: &str,
    body: &str,
    tags: &[String],
    attribution: Option<&DepositAttribution>,
) -> String {
    let title = flatten_line(title);
    let mut out = String::new();
    // Attribution frontmatter rides FIRST (D4): the deposit file itself is the
    // authoritative carrier, downstream copies inherit it. No attribution →
    // no frontmatter, byte-identical to the pre-attribution render (pinned by
    // test). The speaker is line-flattened so a newline cannot forge
    // frontmatter lines; `source` is regex-validated at argument validation.
    if let Some(a) = attribution {
        out.push_str("---\n");
        out.push_str(&format!("attributed_speaker: {}\n", flatten_line(&a.speaker)));
        out.push_str(&format!("attribution_kind: {}\n", a.kind.as_str()));
        out.push_str(&format!("attribution_source: {}\n", a.source));
        out.push_str("---\n");
    }
    out.push_str(&format!("# {title}\n\n"));
    if !tags.is_empty() {
        let list = tags
            .iter()
            .map(|t| format!("`{}`", flatten_line(t)))
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
    let replacement_title = flatten_line(replacement_title);
    let reason = flatten_line(reason);
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

pub(crate) fn set_kick_state(
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

    // A path with no kick row, no document row, and no chunks was never
    // deposited (typo, wrong vault): reporting `pending` here would send the
    // caller into an endless poll for an ingest that will never happen. Error
    // instead — a hand-placed file the watcher already ingested still has a
    // documents/chunks row and flows through the state machine below.
    if kick.is_none() && chunked == 0 {
        let known_doc: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                [vault_relative_path],
                |r| r.get(0),
            )
            .map_err(|e| anyhow!("documents probe failed: {e}"))?;
        if known_doc == 0 {
            bail!("no deposit record for path: {vault_relative_path}");
        }
    }

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
    let supersession_lane = vault_relative_path.starts_with(SUPERSESSIONS_PREFIX);
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

/// App-side kick registration (spec §3.7.3): the APP — never an agent kick —
/// writes the initial kick-ledger row when it creates a `stated/` file. The
/// initial state is `pending`: unlike the MCP deposit path there is no
/// spawn/await split to reflect (the capture call runs the kick itself).
pub(crate) fn register_app_kick(conn: &rusqlite::Connection, rel_path: &str) -> Result<()> {
    set_kick_state(conn, rel_path, "pending", None)
}

fn run_kick(
    db_path: PathBuf,
    vault_dir: PathBuf,
    rel_path: String,
    profile: crate::embedder::EmbedProfile,
) {
    run_kick_with(db_path, vault_dir, rel_path, profile, None)
}

/// [`run_kick`] with an injected LLM completer for the librarian leg —
/// Task 9's stated/ capture test seam (spec §3.7.4: the extraction pipeline
/// itself is unchanged; only the completion call site is substituted).
/// `Some(_)` skips the `llm_generation_configured` gate, since the caller is
/// the host. Not an agent surface: nothing outside `#[cfg(test)]` passes a
/// completer.
pub(crate) fn run_kick_with(
    db_path: PathBuf,
    vault_dir: PathBuf,
    rel_path: String,
    profile: crate::embedder::EmbedProfile,
    completer: Option<&dyn crate::librarian::LlmCompleter>,
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

        // B1 round 2 (mechanism v2): the ingest TABLE KEY is the SAME
        // canonical-root derivation every other regime uses — the
        // vault-relative `rel_path` would stage `documents.path = rel`,
        // missing the virtual-absolute attestation/attribution joins
        // (commit.rs, queue.rs). `rel_path` remains the kick-ledger/status
        // handle and the read handle below.
        let doc_key =
            crate::db::queue::virtual_path_key_at_root(&vault_dir, std::path::Path::new(&rel_path))
                .map_err(|e| anyhow!("canonical vault root: {e:#}"))?;
        // The bytes are read from the vault, not the process CWD.
        let read_path = vault_dir.join(&rel_path).to_string_lossy().into_owned();
        // §8(c) caller contract (Task 9, belt-and-suspenders): force is false
        // here, so the funnel never refuses today — but if a future reason
        // ever produces a typed refusal, treat it as success and still record
        // `chunked`, NEVER as a kick failure. Downcast BEFORE the map_err.
        crate::pipeline::ingest_document_virtual(
            &conn,
            &profile,
            &doc_key,
            &read_path,
            false,
            Some(vault_dir.to_string_lossy().as_ref()),
        )
        .or_else(|e| {
            if e.downcast_ref::<crate::pipeline::IngestRefused>().is_some() {
                Ok(())
            } else {
                Err(e)
            }
        })
        .map_err(|e| anyhow!("ingest: {e:#}"))?;
        set_kick_state(&conn, &rel_path, "chunked", None)?;

        // Librarian leg. `generate_summary` consults folder rules itself
        // (agents = synthesize + auto_approve after V25; stated = Task 9's
        // `synthesize` + auto-approve row); with no configured generation
        // provider it errors — that's `chunked` (no librarian host), not
        // `failed`. Config is re-checked HERE rather than passed in from the
        // spawn site so the ledger decision always matches the config this
        // kick actually runs under. An injected completer (test seam) skips
        // the config gate — the caller IS the host.
        if completer.is_none() && !crate::librarian::llm_generation_configured() {
            // The ledger already reads `chunked` from the ingest leg above —
            // this check only decides whether the librarian leg runs.
            return Ok("chunked".to_string());
        }
        let model = crate::librarian::active_generation_model(crate::setup::recommended_model());
        let gen_result = match completer {
            Some(c) => {
                crate::librarian::generate_summary_with(&mut conn, &doc_key, &model, false, c)
            }
            // B1 round 2: the librarian leg resolves the document under the
            // SAME canonical `documents.path` key the ingest staged above;
            // `get_folder_mode` relativizes it against the configured root
            // for V25's vault-relative folder rules.
            None => crate::librarian::generate_summary(&mut conn, &doc_key, &model, false),
        };
        match gen_result {
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

/// Fail-closed audit row + initial kick-ledger row in ONE transaction.
///
/// The ledger write goes through with_rw (ctx.conn is READ-ONLY in the MCP
/// sidecar). Committing both rows atomically matters in both directions: two
/// separate writes could leave an audit row whose ledger write failed right
/// after it — an audit trail asserting a durable write that was compensated
/// away — or, had the order been reversed, a ledger row with no audit. If
/// this fails, the caller removes the file, so file, audit, and ledger land
/// or vanish together.
async fn audit_and_seed_kick(
    ctx: &ToolDispatchContext,
    tool: &'static str,
    path: String,
    initial_state: &'static str,
    attribution: Option<DepositAttribution>,
    vault: &std::path::Path,
) -> Result<()> {
    let client = ctx.client.clone();
    let audit_path = path.clone();
    let kick_rel = path;
    // Deposit-attribution TABLE KEY: the watcher's virtual-absolute
    // `documents.path` form (`virtual_path_key`, defect B1) — the queue's
    // attribution join (`da.document_path = d.path`, queue.rs:307) matches
    // against the watcher-staged row. `kick_rel` stays the ledger/status
    // handle (vault-relative).
    // B1 round 2 (mechanism v2): the attribution TABLE KEY is derived
    // through the CANONICAL root — `vault` may be a symlinked/relative
    // configured path, and the queue's attribution join
    // (`da.document_path = d.path`, queue.rs:307) matches against rows
    // staged/ingested under the canonical form in EVERY regime (watcher and
    // kick alike). `kick_rel` stays the ledger/status handle
    // (vault-relative).
    let attribution_key = if attribution.is_some() {
        Some(crate::db::queue::virtual_path_key_at_root(
            vault,
            std::path::Path::new(&kick_rel),
        )?)
    } else {
        None
    };
    ctx.with_rw(move |conn| {
        let tx = conn
            .transaction()
            .map_err(|e| anyhow!("kick ledger tx open failed: {e}"))?;
        crate::tool_dispatch::log_agent_access_checked(
            &tx,
            &client,
            tool,
            Some(audit_path.as_str()),
            "write",
        )?;
        set_kick_state(&tx, &kick_rel, initial_state, None)?;
        // Deposit-time attribution index (Task 1, spec rev 4.1 §1): the
        // frontmatter is parsed HERE — inside the same transaction as the
        // audit + kick-ledger writes — so file, audit, ledger, and index all
        // land or vanish together. The table is a derived cache keyed by
        // document path; the FILE remains authoritative (a re-ingest
        // repopulates the index from it).
        if let Some(a) = attribution {
            tx.execute(
                "INSERT OR REPLACE INTO deposit_attribution
                     (document_path, speaker, kind, source, deposited_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    attribution_key.expect("key derived whenever attribution is present"),
                    a.speaker,
                    a.kind.as_str(),
                    a.source,
                    chrono::Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| anyhow!("deposit_attribution insert failed: {e}"))?;
        }
        tx.commit()
            .map_err(|e| anyhow!("kick ledger tx commit failed: {e}"))
    })
    .await
    .map_err(|e| anyhow!("kick-state write failed: {e:#}"))
}

/// The initial ledger state recorded before the kick spawns (D3): a crash
/// between the ledger commit and the spawn leaves an honest trace.
fn initial_kick_state(generation_configured: bool) -> &'static str {
    if generation_configured {
        "pending"
    } else {
        "no_ingest_host"
    }
}

/// Spawn the pipeline kick when a generation host is configured. The caller
/// already committed the initial ledger row (see [`audit_and_seed_kick`]);
/// spawning cannot fail, so file, audit row, and ledger row cannot diverge.
fn spawn_kick_if(
    generation_configured: bool,
    ctx: &ToolDispatchContext,
    vault: &std::path::Path,
    rel: &str,
) -> Option<KickHandle> {
    if !generation_configured {
        return None;
    }
    let (db_path, vault, rel, profile) = (
        ctx.db_path.clone(),
        vault.to_path_buf(),
        rel.to_string(),
        ctx.profile.clone(),
    );
    Some(tokio::task::spawn_blocking(move || {
        run_kick(db_path, vault, rel, profile)
    }))
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
    // Delimiter-anchored prefixes (`safe_path::AGENTS_DEPOSIT_PREFIX` /
    // `SUPERSESSIONS_PREFIX` carry the trailing separator, so the checks
    // cannot match sibling directories like `agents-archive/`; canonical
    // containment in safe_vault_path remains the decisive guard — this
    // routes misuse to the clearest message).
    let in_agents = rel.starts_with(crate::vault::safe_path::AGENTS_DEPOSIT_PREFIX);
    let in_supersessions = rel.starts_with(SUPERSESSIONS_PREFIX);
    if !in_agents || in_supersessions {
        bail!(
            "deposit path must be under {AGENTS_DIR}/ (not {SUPERSESSIONS_DIR}/): {}",
            p.path
        );
    }
    if p.title.trim().is_empty() || p.body.trim().is_empty() {
        bail!("title and body are required");
    }
    // Argument validation (spec rev 4.1 §1, rev 4 m8): attribution handles are
    // rendered into YAML frontmatter and parsed back, so free text is rejected
    // at the door — before any file is created.
    if let Some(a) = &p.attribution {
        if a.speaker.trim().is_empty() {
            bail!("attribution speaker is required");
        }
        validate_handle(&a.source)
            .map_err(|e| anyhow!("invalid attribution_source: {e}"))?;
    }
    let target = vault.join(&rel);
    if target.symlink_metadata().is_ok() {
        bail!("deposit_exists: {} (append-only; supersede instead)", rel);
    }

    let content = render_deposit_file(
        &p.title,
        &p.body,
        p.tags.as_deref().unwrap_or(&[]),
        p.attribution.as_ref(),
    );
    // The whole lane chain down to the deposit's parent must exist before
    // validation: safe_vault_path's MayCreate canonicalizes the parent and
    // reports "parent directory not found" for a fresh vault (CI caught this
    // — immutable-source-files/agents/ does not exist yet there). It is
    // created one component at a time FROM THE VAULT ROOT refusing symlinked
    // components: `create_dir_all` would follow a symlink planted at
    // `immutable-source-files` (or `agents`) and materialize the lane tree
    // outside the vault before safe_vault_path rejects the write. The
    // user-controlled sub-parent (a deposit may live deeper, e.g.
    // agents/topic/note.md) is vetted first: plain names only — no `..`, no
    // root — so nothing is created outside the lane before validation runs.
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
    let lane_rel = std::path::Path::new(AGENTS_DIR).join(sub_parent);
    crate::okf::write::create_parents_no_symlink(&vault, &lane_rel)
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

    // Audit + initial kick-ledger row (fail-closed, ONE RW transaction, same
    // contract as the removed curated write tools) — `wisdom_` joins the
    // curated audit class.
    //
    // The file write above cannot share the audit's transaction, so a failure
    // here compensates by removing the just-created file: the caller must be
    // able to retry the same path instead of wedging on `deposit_exists`
    // forever. Seeding the ledger in the same transaction closes the inverse
    // hole: an audit row can no longer outlive a deposit whose kick-ledger
    // write failed right after the audit commit.
    let generation_configured = crate::librarian::llm_generation_configured();
    if let Err(e) = audit_and_seed_kick(
        ctx,
        "wisdom_deposit",
        rel.clone(),
        initial_kick_state(generation_configured),
        p.attribution.clone(),
        &vault,
    )
    .await
    {
        let _ = std::fs::remove_file(&validated);
        return Err(e);
    }

    // Kick (D3): full pipeline, lock-serialized. The ledger row is committed
    // above, before the spawn, so a crash in between leaves an honest trace.
    // Reply label per spec D3: `started` when a kick spawned.
    let handle = spawn_kick_if(generation_configured, ctx, &vault, &rel);
    let kick_label: &str = if generation_configured {
        "started"
    } else {
        "no_ingest_host"
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
    if !rel.starts_with(crate::vault::safe_path::AGENTS_DEPOSIT_PREFIX) {
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
            let token =
                token.ok_or_else(|| anyhow!("target_fact_id not found or archived: {id}"))?;
            // Same shape gate as the target_source_ref path: a legacy or
            // path-shaped source_ref would render a `supersedes:` line the
            // Active Librarian's application pass (which keys on the exact
            // token shape, D5) can never match — the supersession dead-ends
            // while status honestly reads pending. Refuse instead.
            if !crate::db::commit::is_librarian_source_ref_token(&token) {
                bail!(
                    "target_fact_id {id} resolves to a source_ref that is not a \
                     librarian token: {token}"
                );
            }
            token
        }
    };

    let vault = vault_dir(ctx)?;
    let content = render_supersession_file(
        &target,
        &p.replacement_title,
        &p.replacement_body,
        &p.reason,
    );
    // Lane chain first (fixed constant, not user input), created one
    // component at a time from the vault root refusing symlinked components
    // (a symlinked `immutable-source-files`/`agents` must not be followed
    // outside the vault), then validate, then exclusive-create on the
    // validated path — same contract as deposits.
    crate::okf::write::create_parents_no_symlink(&vault, std::path::Path::new(SUPERSESSIONS_DIR))
        .map_err(|e| anyhow!("unsafe supersession path: {e}"))?;

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

    // Audit + initial kick-ledger row in one transaction, with the same
    // compensating file removal as the deposit lane: a failure must leave
    // the caller able to retry rather than wedged on a path it never chose.
    let generation_configured = crate::librarian::llm_generation_configured();
    if let Err(e) = audit_and_seed_kick(
        ctx,
        "wisdom_propose_supersession",
        rel.clone(),
        initial_kick_state(generation_configured),
        None,
        &vault,
    )
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
    let handle = spawn_kick_if(generation_configured, ctx, &vault, &rel);
    let kick_label: &str = if generation_configured {
        "started"
    } else {
        "no_ingest_host"
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
                    attribution: None,
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
                    attribution: None,
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
                    attribution: None,
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
                    attribution: None,
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
                        attribution: None,
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
                    attribution: None,
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
                    attribution: None,
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
                 VALUES ('fact_z', 'ent', 'T', 'B', 'librarian-abc123def456abc123def456abc12345', 0, 0)",
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
            assert_eq!(
                v["supersedes"],
                serde_json::json!("librarian-abc123def456abc123def456abc12345")
            );
        });
    }

    /// The fact-id path must apply the SAME token-shape gate as the
    /// target_source_ref path: a legacy/short source_ref would render a
    /// `supersedes:` line the Active Librarian application pass (D5) can
    /// never match, dead-ending the supersession while status reads pending.
    #[test]
    fn supersession_by_fact_id_rejects_legacy_source_ref_shapes() {
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
                     VALUES ('fact_legacy', 'ent', 'T', 'B', 'librarian-deadbeef', 0, 0)",
                    [],
                )
                .unwrap();
            let err = dispatch_wisdom_propose_supersession(
                &ctx,
                WisdomProposeSupersessionParams {
                    target_source_ref: None,
                    target_fact_id: Some("fact_legacy".into()),
                    replacement_title: "t".into(),
                    replacement_body: "b".into(),
                    reason: "r".into(),
                },
            )
            .await
            .unwrap_err();
            assert!(
                err.to_string().contains("not a librarian token"),
                "legacy-shaped source_ref must be refused, got: {err}"
            );
        });
    }

    /// A path that was never deposited (typo, wrong vault) must ERROR, not
    /// report `pending` — an agent would otherwise poll forever for an
    /// ingest that will never happen.
    #[test]
    fn status_errors_for_never_deposited_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let err = dispatch_wisdom_deposit_status(
                &ctx,
                WisdomDepositStatusParams {
                    path: "immutable-source-files/agents/note-l.md".into(),
                },
            )
            .await
            .unwrap_err();
            assert!(
                err.to_string().contains("no deposit record"),
                "unknown path must surface a not-found error, got: {err}"
            );
        });
    }

    /// A symlink planted on a lane path component must not be followed: no
    /// directory may be created outside the vault as a side effect of a
    /// deposit/supersession that safe_vault_path then rejects.
    #[test]
    #[cfg(unix)]
    fn deposit_and_supersession_refuse_symlinked_lane_components() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let outside = tempfile::TempDir::new().unwrap();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            // Re-point the `agents` lane at an outside directory AFTER the
            // ctx helper built the real tree.
            std::fs::remove_dir_all(dir.join(AGENTS_DIR)).unwrap();
            symlink(outside.path(), dir.join(AGENTS_DIR)).unwrap();

            let err = dispatch_wisdom_deposit(
                &ctx,
                WisdomDepositParams {
                    path: "immutable-source-files/agents/sneaky.md".into(),
                    title: "t".into(),
                    body: "b".into(),
                    tags: None,
                    attribution: None,
                },
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("unsafe deposit path"), "{err}");
            let err = dispatch_wisdom_propose_supersession(
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
            .unwrap_err();
            assert!(
                err.to_string().contains("unsafe supersession path"),
                "{err}"
            );
            assert!(
                std::fs::read_dir(outside.path()).unwrap().next().is_none(),
                "nothing may be created outside the vault"
            );
        });
    }

    /// Titles/reasons are single-line slots: embedded newlines are collapsed
    /// so a title can never inject a second H1 into the rendered file.
    #[test]
    fn render_flattens_newlines_in_single_line_slots() {
        let deposit = render_deposit_file(
            "Alpha ships v2\n\n# Unrelated heading",
            "body",
            &["tag\nwith newline".to_string()],
            None,
        );
        assert_eq!(
            deposit.lines().next().unwrap(),
            "# Alpha ships v2  # Unrelated heading",
            "title newline must be collapsed, got:\n{deposit}"
        );
        assert_eq!(
            deposit.lines().filter(|l| l.starts_with("# ")).count(),
            1,
            "exactly one H1, got:\n{deposit}"
        );
        assert!(
            deposit.lines().any(|l| l == "`tag with newline`"),
            "tag newline must be collapsed, got:\n{deposit}"
        );

        let supersession = render_supersession_file(
            "librarian-abc123def456abc123def456abc12345",
            "T\n# Evil",
            "B",
            "line one\n- forged bullet",
        );
        let h1s = supersession.lines().filter(|l| l.starts_with("# ")).count();
        assert_eq!(h1s, 2, "exactly the two legit H1s, got:\n{supersession}");
        assert!(
            supersession
                .lines()
                .any(|l| l.starts_with("- reason: line one - forged bullet")),
            "reason must stay one bullet line:\n{supersession}"
        );
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
                    attribution: None,
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
                    attribution: None,
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

    // ------------------------------------------------------------------------
    // Attribution carrier (user-confirmation-queue Task 1): deposit-file
    // frontmatter (D4) + deposit-time index written inside the
    // audit_and_seed_kick transaction. The FILE is authoritative; the brain
    // table is a derived cache keyed by document path (spec rev 4.1 §1).
    // ------------------------------------------------------------------------

    /// Exact pre-change `render_deposit_file` output, captured verbatim from
    /// the code at HEAD c149b3cf (H1, blank line, inline-code tag list, blank
    /// line, `body.trim_end()`, trailing newline). The unattributed-render
    /// test proves byte-identity against it.
    const BASELINE_NOTE: &str = "# Test title\n\n`alpha`\n\nBeta is written in Rust.\n";

    /// Harness helper: renders a minimal deposit note with an optional
    /// attribution carrier.
    fn render_test_deposit(attribution: Option<&DepositAttribution>) -> String {
        render_deposit_file(
            "Test title",
            "Beta is written in Rust.",
            &["alpha".to_string()],
            attribution,
        )
    }

    #[test]
    fn attribution_frontmatter_renders_and_parses_back() {
        let a = DepositAttribution {
            speaker: "kurt".into(),
            kind: AttributionKind::RestatedAtDirection,
            source: "discord:1562137256498569278".into(),
        };
        let md = render_test_deposit(Some(&a));
        assert!(md.starts_with(
            "---\nattributed_speaker: kurt\nattribution_kind: restated_at_direction\nattribution_source: discord:1562137256498569278\n---\n"
        ));
        assert_eq!(
            AttributionKind::try_from("verbatim").unwrap(),
            AttributionKind::Verbatim
        );
        assert!(AttributionKind::try_from("paraphrase").is_err());
        assert!(validate_handle("discord:123").is_ok());
        assert!(validate_handle("has space:123").is_err());
        assert!(validate_handle("discord:").is_err());
    }

    #[test]
    fn unattributed_deposit_renders_byte_identical() {
        assert_eq!(render_test_deposit(None), BASELINE_NOTE);
    }

    #[test]
    fn attribution_index_lands_in_audit_and_seed_kick_transaction() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let rel = "immutable-source-files/agents/attributed.md";
            dispatch_wisdom_deposit_awaiting_kick(
                &ctx,
                WisdomDepositParams {
                    path: rel.into(),
                    title: "Kurt's direction".into(),
                    body: "Deploy the staging stack nightly.".into(),
                    tags: None,
                    attribution: Some(DepositAttribution {
                        speaker: "kurt".into(),
                        kind: AttributionKind::RestatedAtDirection,
                        source: "discord:1562137256498569278".into(),
                    }),
                },
            )
            .await
            .unwrap();
            // The attribution index row and the initial kick-ledger row are
            // committed by the SAME audit_and_seed_kick transaction, so both
            // are visible here, after the dispatch returned.
            let conn = ctx.conn.lock().unwrap();
            // B1: the attribution TABLE KEY is the watcher's virtual-absolute
            // documents.path form; `rel` stays the ledger/status handle.
            // Derive the expected key through the SAME canonical-root family
            // production uses — never the raw tempdir spelling (macOS:
            // /var/... is a symlink to /private/var/...).
            let attribution_key = crate::db::queue::virtual_path_key_at_root(
                ctx.vault_dir.as_deref().unwrap(),
                std::path::Path::new(rel),
            )
            .unwrap();
            let (speaker, kind, source): (String, String, String) = conn
                .query_row(
                    "SELECT speaker, kind, source FROM deposit_attribution
                      WHERE document_path = ?1",
                    [&attribution_key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(speaker, "kurt");
            assert_eq!(kind, "restated_at_direction");
            assert_eq!(source, "discord:1562137256498569278");
            let state: String = conn
                .query_row(
                    "SELECT state FROM deposit_kick_state WHERE path = ?1",
                    [rel],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                matches!(state.as_str(), "pending" | "no_ingest_host"),
                "kick ledger row must exist beside the attribution row, got {state}"
            );
        });
    }

    /// Deterministic librarian stand-in for the watcher-side deposit flow
    /// (same canned-synthesis shape the task9 capture tests use).
    struct WatcherLegCompleter {
        response: String,
    }

    impl crate::librarian::LlmCompleter for WatcherLegCompleter {
        fn complete(&self, _system: &str, _user: &str) -> anyhow::Result<String> {
            Ok(self.response.clone())
        }
    }

    /// RED (F2, B1 round 2): the NO-watcher regime (ct CLI deposit,
    /// headless brain). The kick ingests with the vault-relative key, so
    /// `documents.path` = `rel` while the round-1 B1 fix keyed
    /// `deposit_attribution` in the watcher's canonical absolute form — the
    /// queue's attribution join (`da.document_path = d.path`, queue.rs:307)
    /// misses. The single-key-form ruling applies to the kick-ingest site
    /// TOO: the ingest must stage `documents.path` through the SAME
    /// canonical key derivation; `rel` remains the ledger/kick-status
    /// handle. Here the configured vault root goes through a SYMLINK so the
    /// canonical form differs from the configured one. The kick runs via
    /// `run_kick_with`'s `#[cfg(test)]` completer seam (the dispatch
    /// dispatcher spawns no kick without a generation host — test env has
    /// none), which is the exact pinned F2 site.
    #[test]
    fn kick_ingest_stages_documents_path_in_canonical_form() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().join("vault");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("vault-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real_for_env = real.clone();
        with_brain(&real_for_env, || async move {
            // ctx.vault_dir goes through the SYMLINK; the canonical key the
            // watcher/rebuild lanes derive for the same file is the real
            // path.
            let mut ctx = wisdom_ctx(&real);
            ctx.vault_dir = Some(link.clone());
            let rel = "immutable-source-files/agents/attributed.md";
            let canonical_root = std::fs::canonicalize(&real).unwrap();
            let expected_key =
                crate::db::queue::virtual_path_key(&canonical_root.join(rel)).unwrap();

            // Deposit: the attribution row lands keyed (post-fix) in the
            // canonical form; no kick spawns (no generation host) and the
            // vault lock is NOT held — the no-watcher regime.
            dispatch_wisdom_deposit_awaiting_kick(
                &ctx,
                WisdomDepositParams {
                    path: rel.into(),
                    title: "Kurt's direction".into(),
                    body: "Deploy the staging stack nightly.".into(),
                    tags: None,
                    attribution: Some(DepositAttribution {
                        speaker: "kurt".into(),
                        kind: AttributionKind::Verbatim,
                        source: "discord:123".into(),
                    }),
                },
            )
            .await
            .unwrap();

            // The kick ingest itself — the pinned F2 site — with the
            // deterministic completer seam.
            let kick_ctx = ctx.clone();
            let kick_rel = rel.to_string();
            let handle = tokio::task::spawn_blocking(move || {
                run_kick_with(
                    kick_ctx.db_path.clone(),
                    kick_ctx.vault_dir.clone().unwrap(),
                    kick_rel,
                    kick_ctx.profile.clone(),
                    None,
                );
            });
            handle.await.unwrap();

            let conn = ctx.conn.lock().unwrap();
            let doc_path: String = conn
                .query_row(
                    "SELECT path FROM documents WHERE path = ?1",
                    [&expected_key],
                    |r| r.get(0),
                )
                .expect(
                    "RED (F2): the kick ingested documents.path under the vault-RELATIVE key, \
                     not the canonical absolute form — the attribution join misses",
                );
            let attribution_key: String = conn
                .query_row("SELECT document_path FROM deposit_attribution", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(
                attribution_key, doc_path,
                "RED (F2): attribution and documents.path must share ONE canonical key form"
            );
            // The rel handle stays the kick-ledger/status key.
            conn.query_row(
                "SELECT state FROM deposit_kick_state WHERE path = ?1",
                [rel],
                |r| r.get::<_, String>(0),
            )
            .expect("the kick ledger keeps the rel handle");
        });
    }

    /// RED (B1 regression, deposit-attribution twin): the APP's deposit
    /// regime. The watcher holds the vault lock for its lifetime, so the
    /// deposit kick falls back to `queued_watcher` and the WATCHER stages and
    /// ingests the file, keying `documents.path` VIRTUAL-ABSOLUTE (issue
    /// #204). The `deposit_attribution` row must carry the SAME key form or
    /// the queue's attribution join (`da.document_path = d.path`,
    /// queue.rs:307) never matches and the row's `attributed_speaker` is
    /// lost.
    #[test]
    fn deposit_attribution_surfaces_through_watcher_ingest() {
        let dir = tempfile::TempDir::new().unwrap();
        let dir = dir.path().to_owned();
        let dir_for_env = dir.clone();
        with_brain(&dir_for_env, || async move {
            let ctx = wisdom_ctx(&dir);
            let rel = "immutable-source-files/agents/attributed.md";

            // App regime: hold the vault lock so the synchronous kick cannot
            // ingest (queued_watcher fallback — wisdom_deposit.rs:484-491).
            let lock = crate::watcher::VaultLock::acquire(&dir).unwrap();
            dispatch_wisdom_deposit_awaiting_kick(
                &ctx,
                WisdomDepositParams {
                    path: rel.into(),
                    title: "Kurt's direction".into(),
                    body: "Deploy the staging stack nightly.".into(),
                    tags: None,
                    attribution: Some(DepositAttribution {
                        speaker: "kurt".into(),
                        kind: AttributionKind::Verbatim,
                        source: "discord:123".into(),
                    }),
                },
            )
            .await
            .unwrap();
            drop(lock);

            // Watcher leg: stage + ingest keyed by the watcher's absolute
            // `documents.path` form. The running watcher resolves the vault
            // root CANONICALLY (`canonical_vault_from_config`), so drive the
            // lane through the same canonical-root family — never the raw
            // tempdir spelling (macOS: /var/... is a symlink to
            // /private/var/...).
            let root = dir.canonicalize().unwrap();
            let abs = root.join(rel);
            let abs_key = crate::db::queue::virtual_path_key(&abs).unwrap();
            {
                let mut guard = ctx.conn.lock().unwrap();
                crate::db::queue::enqueue_vault_event(
                    &mut guard,
                    notify::EventKind::Create(notify::event::CreateKind::Any),
                    &abs,
                    Some(&root),
                )
                .unwrap();
                crate::pipeline::ingest_document_virtual(
                    &guard,
                    &ctx.profile,
                    &abs_key,
                    &abs.to_string_lossy(),
                    false,
                    Some(root.to_string_lossy().as_ref()),
                )
                .unwrap();
            }

            // Librarian + commit (the folder's auto-approve lane, matching
            // V25's intent for immutable-source-files/agents): the deposit
            // fact lands `librarian_inferred` (deposits carry no attestation,
            // agent-class commit) and shows on the human confirmation queue
            // with its attribution speaker joined on the document path.
            // Folder rules match by literal ancestor prefix, so the
            // watcher-keyed lane gets its own absolute rule.
            let completer = WatcherLegCompleter {
                response: serde_json::json!({
                    "proposals": [{
                        "target": { "existing_id": "ent-alpha" },
                        "reasoning": "Deposit capture.",
                        "summary_update": null,
                        "facts": [{
                            "op": "add",
                            "body": "Deploy the staging stack nightly.",
                            "tags": [],
                            "confidence": "inferred",
                            "evidence": ["C1"]
                        }],
                        "edges": [],
                        "tasks": []
                    }]
                })
                .to_string(),
            };
            {
                let guard = ctx.conn.lock().unwrap();
                // Seed the entity the canned proposal targets.
                guard
                    .execute(
                        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                         VALUES ('ent-alpha', 'Staging', 'concept', 'Stack notes', 100, 100)",
                        [],
                    )
                    .unwrap();
                guard
                    .execute(
                        "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve)
                         VALUES (?1, 'synthesize', 1)",
                        [root
                            .join("immutable-source-files/agents")
                            .to_string_lossy()
                            .as_ref()],
                    )
                    .unwrap();
            }
            let mut guard = ctx.conn.lock().unwrap();
            let model =
                crate::librarian::active_generation_model(crate::setup::recommended_model());
            crate::librarian::generate_summary_with(
                &mut guard, &abs_key, &model, false, &completer,
            )
            .unwrap();
            let rows = crate::db::queue::queue_rows(&guard);
            drop(guard);
            let row = rows
                .iter()
                .find(|r| r.body == "Deploy the staging stack nightly.")
                .expect("the deposit fact is on the human confirmation queue");
            assert_eq!(
                row.attributed_speaker.as_deref(),
                Some("kurt"),
                "RED (B1): deposit_attribution is keyed kick_rel (vault-relative) while \
                 documents.path is watcher-absolute — the queue's attribution join never matches"
            );
        });
    }
}
