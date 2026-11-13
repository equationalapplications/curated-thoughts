//! Tauri commands for the human confirmation queue, Path B capture, and the
//! Active Verification Prompting drain (user-confirmation-queue plan,
//! Task 9 / spec §3.2-§3.4, §9). Confirm/deny/undeny/capture are APP-ONLY —
//! no MCP/CLI wrapper is registered beside them (spec §3.3, D2).

use crate::db::queue::QueueRow;
use crate::DbState;
use serde::Serialize;
use tauri::State;

/// One `queue_denials` row for the queue screen's denied section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DenialRow {
    pub document_path: String,
    pub chunk_hash: String,
    pub fact_text_hash: String,
    pub denied_by: String,
    pub denied_at: String,
}

#[tauri::command]
pub fn queue_list_cmd(db_state: State<'_, DbState>) -> Result<Vec<QueueRow>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    Ok(queue_list_rows(&guard.0))
}

/// The confirmation queue rows, gated by `confirmations.enabled` (issue #344,
/// spec §7.4): when the surface is off, the queue panel renders nothing — an
/// empty list, zero DB work. The testable core of [`queue_list_cmd`] (a
/// `tauri::State` cannot be constructed in a unit test).
pub fn queue_list_rows(conn: &rusqlite::Connection) -> Vec<QueueRow> {
    if !crate::config::confirmation_enabled_for_db(conn.path()) {
        return Vec::new();
    }
    crate::db::queue::queue_rows(conn)
}

/// Confirm a queue fact (spec §3.3). App-only human stamp.
#[tauri::command]
pub fn queue_confirm_cmd(fact_id: String, db_state: State<'_, DbState>) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    crate::db::queue::confirm_fact(&guard.0, &fact_id).map_err(|e| e.to_string())
}

/// Deny a queue fact (spec §3.3: archive + suppress). The UI must surface a
/// denial error rather than swallow it (Task 8 review note).
#[tauri::command]
pub fn queue_deny_cmd(fact_id: String, db_state: State<'_, DbState>) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    crate::db::queue::deny_fact(&guard.0, &fact_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn queue_undeny_cmd(
    key: crate::db::queue::DenialKey,
    db_state: State<'_, DbState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    crate::db::queue::undeny_fact(&guard.0, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn queue_denied_cmd(db_state: State<'_, DbState>) -> Result<Vec<DenialRow>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = guard
        .0
        .prepare(
            "SELECT document_path, chunk_hash, fact_text_hash, denied_by, denied_at
               FROM queue_denials
              ORDER BY denied_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(DenialRow {
                document_path: r.get(0)?,
                chunk_hash: r.get(1)?,
                fact_text_hash: r.get(2)?,
                denied_by: r.get(3)?,
                denied_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Capture a fresh statement typed into the queue view (Rule 15 / spec §9):
/// Path B — stated/ file, attest, kick. Runs the whole kick on a blocking
/// thread (instant confirm: human tier when it returns).
#[tauri::command]
pub async fn queue_capture_cmd(
    text: String,
    db_state: State<'_, DbState>,
    embed_profile: State<'_, crate::EmbedProfileState>,
) -> Result<String, String> {
    let ctx = capture_context(&db_state, &embed_profile)?;
    tokio::task::spawn_blocking(move || {
        crate::db::wisdom::capture_statement(&ctx, &text).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("capture task join error: {e}"))?
}

/// The queue-level confirm (Rule 15 / spec §9): a live fact_id confirms;
/// a confirm-with-no-fact (or `factId: null` with fresh text) routes through
/// capture_statement.
#[tauri::command]
pub async fn queue_confirm_or_capture_cmd(
    fact_id: Option<String>,
    fresh_text: Option<String>,
    db_state: State<'_, DbState>,
    embed_profile: State<'_, crate::EmbedProfileState>,
) -> Result<String, String> {
    let ctx = capture_context(&db_state, &embed_profile)?;
    tokio::task::spawn_blocking(move || {
        crate::db::wisdom::confirm_or_capture(&ctx, fact_id.as_deref(), fresh_text.as_deref())
            .map(|outcome| match outcome {
                crate::db::wisdom::QueueStatementOutcome::Confirmed => "confirmed".to_string(),
                crate::db::wisdom::QueueStatementOutcome::Captured(rel) => rel,
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("capture task join error: {e}"))?
}

/// M1 contract (anti-payload-spoof): the prompting modal re-reads the fact BY
/// ID from the brain and renders THAT, never the drop-file payload text. The
/// queue view's row carries the evidence quote join, so the modal shows the
/// same provenance the queue shows.
#[tauri::command]
pub fn queue_fact_get_cmd(
    fact_id: String,
    db_state: State<'_, DbState>,
) -> Result<Option<QueueRow>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    Ok(crate::db::queue::queue_rows(&guard.0)
        .into_iter()
        .find(|r| r.fact_id == fact_id))
}

/// Drain the Active Verification Prompting drop file (spec §3.4): oldest
/// unacked first, capped at [`crate::prompt_drop::PROMPT_DRAIN_CAP`] per
/// drain, plus the remaining count for the modal's digest.
#[tauri::command]
pub fn unverified_prompts_drain_cmd() -> Result<crate::prompt_drop::DrainReport, String> {
    // The config path resolves the same way the vault root below does —
    // `resolve_brain_paths`, so the env overrides agree (spec §7.5).
    let config_path = crate::retrieval::resolve_brain_paths().config_path;
    drain_prompts_at(&config_path, crate::prompt_drop::PROMPT_DRAIN_CAP)
}

/// The gated, testable core of [`unverified_prompts_drain_cmd`] (issue #344,
/// spec §7.4): when `confirmations.enabled` is false, return an EMPTY report
/// and leave `unverified-match.jsonl` untouched — stale lines are not drained,
/// not deleted, not rendered.
pub fn drain_prompts_at(
    config_path: &std::path::Path,
    cap: usize,
) -> Result<crate::prompt_drop::DrainReport, String> {
    if !crate::config::confirmation_enabled_at(config_path) {
        return Ok(crate::prompt_drop::DrainReport {
            prompts: Vec::new(),
            total_unacked: 0,
        });
    }
    let vault_root = crate::db::stated_ledger::resolve_vault_root().map_err(|e| e.to_string())?;
    // The drain touches only a vault file (the DB lock was never needed — the
    // vestigial `DbState` parameter went away with the Tier-1.5 high MINOR-5
    // cleanup).
    crate::prompt_drop::drain_unverified_matches(&vault_root, cap).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_drop::{UnverifiedMatch, BRAIN_DIR, UNVERIFIED_MATCH_FILE};

    fn ev(fact_id: &str) -> UnverifiedMatch {
        UnverifiedMatch {
            fact_id: fact_id.to_string(),
            title: Some("Deploy checklist".to_string()),
            body: Some("Run the gates before shipping.".to_string()),
            quote: Some("docs/ops.md".to_string()),
            query: "shipping checklist".to_string(),
            surface: "wiki_search".to_string(),
        }
    }

    /// Temp vault + brain: config.json at `<vault>/.brain/config.json` with an
    /// optional `confirmations` block (`vault_path` points at the temp vault,
    /// as a real brain does), plus one unacked drop line.
    fn gated_fixture(enabled: Option<bool>) -> (tempfile::TempDir, std::path::PathBuf) {
        let vault = tempfile::TempDir::new().unwrap();
        let brain = vault.path().join(BRAIN_DIR);
        std::fs::create_dir_all(&brain).unwrap();
        let confirmations = match enabled {
            Some(false) => r#","confirmations":{"enabled":false}"#,
            Some(true) => r#","confirmations":{"enabled":true}"#,
            None => "",
        };
        let config = format!(
            r#"{{"vault_path":"{}",{}}}"#,
            vault.path().display(),
            confirmations.trim_start_matches(',')
        );
        let config_path = brain.join("config.json");
        std::fs::write(&config_path, config).unwrap();
        (vault, config_path)
    }

    /// flag OFF: the drain command returns an EMPTY report and leaves stale
    /// `unverified-match.jsonl` lines untouched (plan Task 5 Step 4, spec §7.4).
    #[test]
    fn drain_returns_empty_and_leaves_the_file_when_disabled() {
        let (vault, config_path) = gated_fixture(Some(false));
        // A stale line the flag-off drain must NOT delete.
        crate::prompt_drop::emit_unverified_match(vault.path(), ev("w9"));
        let file = vault.path().join(BRAIN_DIR).join(UNVERIFIED_MATCH_FILE);
        let before = std::fs::read_to_string(&file).unwrap();

        let report = drain_prompts_at(&config_path, crate::prompt_drop::PROMPT_DRAIN_CAP).unwrap();
        assert!(
            report.prompts.is_empty() && report.total_unacked == 0,
            "disabled surface drains nothing: {report:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            before,
            "stale lines are left byte-identical"
        );
    }

    /// flag ON (and absent): the drain returns the prompt as today. The env
    /// override points `resolve_vault_root` at the fixture's config — the same
    /// agreement the production command relies on (spec §7.5).
    #[test]
    fn drain_still_works_when_enabled_or_absent() {
        let (vault, config_path) = gated_fixture(Some(true));
        let cfg_str = config_path.to_str().unwrap().to_string();
        let db_str = vault.path().join(BRAIN_DIR).join("brain.db");
        let db_str = db_str.to_str().unwrap().to_string();
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_CONFIG", Some(cfg_str.as_str())),
                ("CURATED_BRAIN_DB", Some(db_str.as_str())),
            ],
            || {
                crate::prompt_drop::emit_unverified_match(vault.path(), ev("w9"));
                let report =
                    drain_prompts_at(&config_path, crate::prompt_drop::PROMPT_DRAIN_CAP).unwrap();
                assert_eq!(report.prompts.len(), 1);
                assert_eq!(report.prompts[0].fact_id, "w9");
            },
        );
    }

    /// flag OFF: the queue-list gate returns an empty list without touching
    /// the DB (the ON branch is covered by the `db::queue` row tests).
    #[test]
    fn queue_list_gate_returns_empty_when_disabled() {
        let (vault, _cfg) = gated_fixture(Some(false));
        let conn =
            rusqlite::Connection::open(vault.path().join(BRAIN_DIR).join("brain.db")).unwrap();
        // No queue DDL at all — the gate must short-circuit before SQL.
        assert!(queue_list_rows(&conn).is_empty());
    }
}

/// Build the capture context from app state. The db path comes from the open
/// app db; the vault root from the configured brain config — the same
/// resolution every *_at backend function accepts explicitly.
fn capture_context(
    db_state: &State<'_, DbState>,
    embed_profile: &State<'_, crate::EmbedProfileState>,
) -> Result<crate::db::wisdom::CaptureContext, String> {
    let db_path = {
        let guard = db_state.0.lock().map_err(|e| e.to_string())?;
        guard
            .0
            .path()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "brain db has no path".to_string())?
    };
    let profile = embed_profile.0.lock().map_err(|e| e.to_string())?.clone();
    Ok(crate::db::wisdom::CaptureContext {
        db_path,
        vault_root: crate::db::stated_ledger::resolve_vault_root().map_err(|e| e.to_string())?,
        profile,
    })
}
