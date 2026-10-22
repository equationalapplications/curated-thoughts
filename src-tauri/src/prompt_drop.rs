//! Active Verification Prompting drop-file emitter (user-confirmation-queue
//! plan Task 6, spec rev 4.1 §4 / R-A).
//!
//! When an agent-facing recall tool (`wiki_context`, `wiki_search`,
//! `curated_recall_context`) matches a fact whose provenance is not
//! human-tier, the recall path appends one JSONL event line to
//! `<vault>/.brain/unverified-match.jsonl`; the desktop app watches the file
//! and renders the Confirm/Reject modal. Recall emits NOTHING over Tauri —
//! this module holds no tauri import (the sidecar links this shape) and
//! talks only to the filesystem and the read-only brain connection.
//!
//! # Drop-file format
//!
//! One self-contained JSON object per line, single-line serde-serialized
//! (never hand-concatenated), fields pinned by the plan:
//!
//! ```json
//! {"fact_id":"…","title":"…","body":"…","quote":"…","query":"…","surface":"wiki_search","ts":1761000000000}
//! ```
//!
//! `quote` carries the evidence source reference (`llm_wiki_entries.source_ref`)
//! and `surface` the calling tool — together the "why relevant now" trigger
//! context (spec §4). Anti-spoof (M1): the modal NEVER renders payload text as
//! the fact; the app re-reads fact, evidence and queue state from the brain by
//! `fact_id`. A poisoned drop file can at worst cause a nuisance modal.
//!
//! # Ack contract
//!
//! The app is the SOLE pruner: it drains the file (read lines, render modals —
//! capped and de-duped per drain cycle per spec §4 — then truncate). A line is
//! UNACKED iff it has no `acked_at` field; the app MAY mark lines in place with
//! `acked_at` instead of truncating. The emitter never rewrites or truncates
//! the file: it appends ONLY when no UNACKED line for the same `fact_id`
//! already exists (advisory read-before-append, spec §4 M2-rev2). The file is
//! ephemeral operational state — excluded from brain sync/backup.
//!
//! # Emitter infallibility (spec §4 rev-4 m7)
//!
//! [`emit_unverified_match`] returns `()` — every drop-file I/O error is
//! logged and swallowed. The recall path must NEVER fail because the prompt
//! drop could not be written; a read-only `.brain/` degrades to "no prompts"
//! (pinned by `read_only_brain_dir_never_errors_the_call`). The UNACKED
//! dedupe read follows the ledger disciplines (`db::stated_ledger`): single
//! serde-serialized appends, and a malformed line is never SILENTLY skipped —
//! it is logged; since the emitter cannot propagate, a malformed line is then
//! treated as non-matching for the advisory dedupe (nuisance-modal risk at
//! worst, per the spec's poisoned-file bound).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

/// Brain directory inside the vault root — the drop file lives under it
/// (NOT under `stated/`; controller note 1).
pub const BRAIN_DIR: &str = ".brain";

/// The unverified-match drop file name.
pub const UNVERIFIED_MATCH_FILE: &str = "unverified-match.jsonl";

/// Human-tier provenance values (plan Global Constraints: human tier =
/// `user_stated` + `user_confirmed` ONLY). Membership is decided by running
/// the stored `source_type` through `wisdom_match::provenance_for` (vocabulary
/// filter, `wisdom_match.rs:264-268`) and comparing against this set — tier
/// logic is NOT re-implemented here.
const HUMAN_TIER: &[&str] = &["user_stated", "user_confirmed"];

/// One unverified-match prompt event (spec §4 payload: fact id, title/body,
/// evidence quote / source reference, trigger context).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnverifiedMatch {
    pub fact_id: String,
    pub title: Option<String>,
    pub body: Option<String>,
    /// Evidence quote / source document reference.
    pub quote: Option<String>,
    /// The recall query — "why relevant now" (trigger context).
    pub query: String,
    /// The calling surface (which recall tool fired).
    pub surface: String,
}

/// One serialized drop-file line (`ts` is stamped by the emitter at write
/// time, ms since the Unix epoch).
#[derive(Serialize)]
struct DropLine<'a> {
    fact_id: &'a str,
    title: &'a Option<String>,
    body: &'a Option<String>,
    quote: &'a Option<String>,
    query: &'a str,
    surface: &'a str,
    ts: i64,
}

/// True when the stored `source_type` is human-tier (see [`HUMAN_TIER`]).
/// `None` / out-of-vocabulary values are NOT human tier.
pub fn is_human_tier(source_type: Option<&str>) -> bool {
    matches!(
        crate::wisdom_match::provenance_for(source_type).as_deref(),
        Some(p) if HUMAN_TIER.contains(&p)
    )
}

/// Append one unverified-match line to `<vault_root>/.brain/unverified-match.jsonl`,
/// unless an UNACKED line for `ev.fact_id` already exists. INFALLIBLE (spec §4
/// rev-4 m7): every error is logged and swallowed; `Ok`-less by design.
pub fn emit_unverified_match(vault_root: &Path, ev: UnverifiedMatch) {
    let dir = vault_root.join(BRAIN_DIR);
    let path = dir.join(UNVERIFIED_MATCH_FILE);
    if has_unacked_line(&path, &ev.fact_id) {
        return;
    }
    // Errors from here on are logged + swallowed — never propagated.
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("curated-thoughts-mcp: prompt drop skipped (mkdir): {e}");
        return;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let line = DropLine {
        fact_id: &ev.fact_id,
        title: &ev.title,
        body: &ev.body,
        quote: &ev.quote,
        query: &ev.query,
        surface: &ev.surface,
        ts,
    };
    let emit = || -> std::io::Result<()> {
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(f, "{}", serde_json::to_string(&line).unwrap_or_default())?;
        Ok(())
    };
    if let Err(e) = emit() {
        eprintln!("curated-thoughts-mcp: prompt drop skipped (append): {e}");
    }
}

/// Per-drain modal cap (spec §4: per-drain modal cap (3) + snooze preference
/// bound day-one volume).
pub const PROMPT_DRAIN_CAP: usize = 3;

/// One drained prompt: what the app needs to re-read the fact FROM THE BRAIN
/// (anti-spoof M1: the drop payload's title/body/quote are NEVER rendered —
/// the modal fetches the row by `fact_id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DrainedPrompt {
    pub fact_id: String,
    pub ts: i64,
}

/// Drain result: the ≤cap prompts to show, plus how many unacked lines REMAIN
/// (the modal's "N more" digest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DrainReport {
    pub prompts: Vec<DrainedPrompt>,
    pub total_unacked: usize,
}

/// App-side drain (Task 9's Active Verification Prompting): take the oldest
/// unacked lines (up to `cap`), mark THEM acked, and report the remaining
/// unacked count. The rewrite also PRUNES strictly-acked lines (this function
/// is the ONLY rewriter; the emitter appends but never rewrites), so the drop
/// file stays bounded by live state, not by prompting history. A missing file
/// is an empty drain; a malformed line is never silently dropped — it is
/// rewritten unchanged (the ledger discipline).
pub fn drain_unverified_matches(vault_root: &Path, cap: usize) -> Result<DrainReport> {
    let dir = vault_root.join(BRAIN_DIR);
    let path = dir.join(UNVERIFIED_MATCH_FILE);
    if !path.exists() {
        return Ok(DrainReport {
            prompts: Vec::new(),
            total_unacked: 0,
        });
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut out_lines: Vec<String> = Vec::new();
    let mut prompts: Vec<DrainedPrompt> = Vec::new();
    let mut unacked_total = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "curated-thoughts: prompt drain malformed line kept unchanged: {e}"
                );
                out_lines.push(line.to_string());
                continue;
            }
        };
        // Tier-1.5 xhigh MAJOR-2: a valid-JSON NON-object line (hand-edited or
        // corrupted file) must degrade like a malformed one — indexing a
        // non-object Value with a string key panics, and the panic would
        // repeat on every 30-second poll with no self-recovery. Keep it
        // verbatim, don't count it, never panic (spec §4 poisoned-file bound).
        let Some(obj) = v.as_object_mut() else {
            eprintln!(
                "curated-thoughts: prompt drain non-object line kept unchanged"
            );
            out_lines.push(line.to_string());
            continue;
        };
        if obj.get("acked_at").is_some() {
            // Strictly-acked lines are dead weight: the dedupe is
            // unacked-scoped and this drain is the ONLY rewriter of the file.
            // Dropping them here keeps `unverified-match.jsonl` bounded —
            // keeping them accumulated one line per prompt shown, forever,
            // and `has_unacked_line` re-parsed the whole file on every hit.
            continue;
        }
        unacked_total += 1;
        if prompts.len() < cap {
            let fact_id = obj
                .get("fact_id")
                .and_then(|f| f.as_str())
                .unwrap_or_default()
                .to_string();
            let ts = obj.get("ts").and_then(|t| t.as_i64()).unwrap_or(0);
            prompts.push(DrainedPrompt { fact_id, ts });
            obj.insert("acked_at".to_string(), serde_json::json!(now_ms));
        }
        out_lines.push(serde_json::to_string(&v)?);
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let mut body = out_lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    // Task-5 Low-1 closeout (Task-9 extension): the ack rewrite is ATOMIC —
    // write to a same-directory temp file (fsynced, original permission bits
    // preserved) and `rename` it over the ledger, so a crash mid-rewrite can
    // never leave a truncated `unverified-match.jsonl` behind.
    crate::vault::safe_path::safe_write_bytes(&path, body.as_bytes())
        .with_context(|| format!("rewriting {}", path.display()))?;
    let total_unacked = unacked_total - prompts.len();
    Ok(DrainReport {
        prompts,
        total_unacked,
    })
}

/// Advisory read-before-append: does `path` hold an UNACKED line (no
/// `acked_at` field) for `fact_id`? A missing file is "no line"; any read or
/// parse problem is logged and read as "no matching line" — the dedupe is
/// advisory (spec §4) and the emitter is infallible. A malformed line is
/// never silently skipped (logged, per the ledger discipline).
fn has_unacked_line(path: &Path, fact_id: &str) -> bool {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(e) => {
            eprintln!("curated-thoughts-mcp: prompt dedupe read skipped: {e}");
            return false;
        }
    };
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "curated-thoughts-mcp: prompt drop malformed line {} skipped: {e}",
                    n + 1
                );
                continue;
            }
        };
        if v.get("fact_id").and_then(|f| f.as_str()) == Some(fact_id)
            && v.get("acked_at").is_none()
        {
            return true;
        }
    }
    false
}

/// One recall hit with its provenance ALREADY resolved (Task 7): the caller
/// holds the hit's id and vocabulary-filtered `source_type` label (from
/// `WikiSearchHit` / `rank_wiki_entries`), so the hook never re-SELECTs per id
/// for the tier decision — the Task-6 N+1 lookup is closed here.
pub struct ResolvedHit<'a> {
    pub fact_id: &'a str,
    /// Hit-level provenance label (post-`provenance_for`). `None` outside the
    /// vocabulary — never human tier.
    pub source_type: Option<&'a str>,
}

/// One brain row's drop payload: `(title, body, quote)` — the three columns
/// the batched lookup reads (anti-spoof M1: payload text comes from the brain
/// row, never from agent-controlled input).
type RowPayload = (Option<String>, Option<String>, Option<String>);

/// Recall-path hook: for each matched fact, skip human-tier provenance (the
/// hit-level label — NO per-id re-SELECT) and emit an unverified-match prompt
/// for the rest. The payload columns (`title`, `body`, `source_ref`) still
/// come from the brain row (anti-spoof M1), read in ONE batched query for the
/// whole hit list; a fact missing from that result is skipped silently (the
/// old skip-on-missing), per-row read errors are logged and skipped.
/// INFALLIBLE end to end — `vault_root: None` (vault not configured) or any
/// lookup error degrades to fewer/no prompts, never an error.
pub fn emit_unverified_matches(
    vault_root: Option<&Path>,
    conn: &Connection,
    hits: &[ResolvedHit<'_>],
    query: &str,
    surface: &str,
) {
    use std::collections::HashMap;
    let Some(root) = vault_root else {
        return;
    };
    if hits.is_empty() {
        return;
    }
    // ONE batched lookup for the payload columns (Task 7 N+1 closure).
    let placeholders = vec!["?"; hits.len()].join(", ");
    let sql = format!(
        "SELECT id, title, body, source_ref
         FROM llm_wiki_entries
         WHERE id IN ({placeholders}) AND deleted_at IS NULL"
    );
    let ids: Vec<&str> = hits.iter().map(|h| h.fact_id).collect();
    let mut stmt = match conn.prepare(sql.as_str()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("curated-thoughts-mcp: prompt drop rows skipped: {e}");
            return;
        }
    };
    let rows = stmt.query_map(
        rusqlite::params_from_iter(ids.iter()),
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        },
    );
    let mut payload: HashMap<String, RowPayload> = HashMap::new();
    // Batch-level binding error: the WHOLE payload batch degrades to
    // all-hits-skipped, so it logs the batch (plural) token — the per-row
    // singular token below stays reserved for per-row failures (Task-7
    // Info-1/Info-2 closeout: the degradation is never silent).
    if let Err(e) = rows {
        eprintln!("curated-thoughts-mcp: prompt drop rows skipped: {e}");
        return;
    }
    for row in rows.unwrap() {
        match row {
            Ok((id, title, body, quote)) => {
                payload.insert(id, (title, body, quote));
            }
            Err(e) => {
                eprintln!("curated-thoughts-mcp: prompt drop row skipped: {e}");
            }
        }
    }
    for hit in hits {
        // Skip-on-missing: a fact deleted between the search and this emit is
        // simply absent from the batch result — skipped as before.
        let Some((title, body, quote)) = payload.get(hit.fact_id) else {
            continue;
        };
        if is_human_tier(hit.source_type) {
            continue;
        }
        emit_unverified_match(
            root,
            UnverifiedMatch {
                fact_id: hit.fact_id.to_string(),
                title: title.clone(),
                body: body.clone(),
                quote: quote.clone(),
                query: query.to_string(),
                surface: surface.to_string(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn scratch_ev(fact_id: &str) -> UnverifiedMatch {
        UnverifiedMatch {
            fact_id: fact_id.to_string(),
            title: Some("Deploy checklist".to_string()),
            body: Some("Run the gates before shipping.".to_string()),
            quote: Some("docs/ops.md".to_string()),
            query: "shipping checklist".to_string(),
            surface: "wiki_search".to_string(),
        }
    }

    fn drop_path(root: &Path) -> std::path::PathBuf {
        root.join(BRAIN_DIR).join(UNVERIFIED_MATCH_FILE)
    }

    /// Table-less connection for the hook form: per-row lookups fail (logged,
    /// skipped) and the emit degrades to no prompts — the pinned infallible
    /// surface under test is the file write, not the DB.
    fn open_memory_conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    /// Restores writable permissions on `.brain` even if the assertions panic
    /// (the plan's "restore in catch/finally" — TempDir cleanup needs it too).
    #[cfg(unix)]
    struct RestorePerm(std::path::PathBuf);
    #[cfg(unix)]
    impl Drop for RestorePerm {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        }
    }

    #[test]
    fn emitter_writes_jsonl_and_dedupes_unacked() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Two emits, one UNACKED line.
        emit_unverified_match(root, scratch_ev("w1"));
        emit_unverified_match(root, scratch_ev("w1"));
        let text = fs::read_to_string(drop_path(root)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "two emits for the same fact = one line");
        let line: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(line["fact_id"], "w1");
        assert_eq!(line["surface"], "wiki_search");
        assert_eq!(line["query"], "shipping checklist");
        assert!(line["ts"].is_i64(), "emitter stamps ts");
        // The plan's pinned field set — exactly these keys.
        let mut keys: Vec<&str> = line.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["body", "fact_id", "query", "quote", "surface", "title", "ts"]
        );

        // A different fact still appends.
        emit_unverified_match(root, scratch_ev("w2"));
        assert_eq!(fs::read_to_string(drop_path(root)).unwrap().lines().count(), 2);

        // The app acks w1's line; the NEXT emit for w1 must append again —
        // the dedupe is UNACKED-scoped, not per-file.
        let mut acked = line.clone();
        acked["acked_at"] = serde_json::json!("2026-10-21T12:00:00Z");
        fs::write(drop_path(root), format!("{acked}\n")).unwrap();
        emit_unverified_match(root, scratch_ev("w1"));
        let text = fs::read_to_string(drop_path(root)).unwrap();
        assert_eq!(text.lines().count(), 2, "acked line no longer suppresses");
        assert!(text.lines().all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()),
            "every line is a single-line JSON object");
    }

    #[test]
    fn drain_degrades_non_object_lines_without_panicking() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(drop_path(root).parent().unwrap()).unwrap();
        // A valid-JSON non-object line (hand edit / corruption) must degrade
        // like a malformed line — never panic (spec §4 poisoned-file bound).
        fs::write(drop_path(root), "42\n{\"fact_id\":\"w9\",\"ts\":1}\n").unwrap();
        let report = drain_unverified_matches(root, 3).unwrap();
        assert_eq!(report.prompts.len(), 1, "only the object line drains");
        let text = fs::read_to_string(drop_path(root)).unwrap();
        assert!(
            text.lines().any(|l| l.trim() == "42"),
            "the non-object line is kept verbatim"
        );
    }

    #[test]
    #[cfg(unix)] // read-only-dir semantics come from POSIX mode bits
    fn read_only_brain_dir_never_errors_the_call() {
        let dir = TempDir::new().unwrap();
        let brain = dir.path().join(BRAIN_DIR);
        fs::create_dir_all(&brain).unwrap();
        fs::set_permissions(&brain, fs::Permissions::from_mode(0o500)).unwrap();
        let _restore = RestorePerm(brain.clone());

        // Must return normally (no panic, no error surface) and write nothing:
        // read-only `.brain` degrades to "no prompts" (spec §4 rev-4 m7).
        emit_unverified_match(dir.path(), scratch_ev("w1"));
        assert!(
            !drop_path(dir.path()).exists(),
            "no line may appear in a read-only .brain"
        );

        // The recall-hook form over the same read-only vault also degrades.
        // (No live connection rows needed: the file-level emit is the pinned
        // failure surface, and the hook fans into it.)
        emit_unverified_matches(
            Some(dir.path()),
            &open_memory_conn(),
            &[ResolvedHit {
                fact_id: "w1",
                source_type: None,
            }],
            "q",
            "wiki_context",
        );
        assert!(!drop_path(dir.path()).exists());
    }

    /// Task-6 Low-1 closeout pin: a MALFORMED line in
    /// `unverified-match.jsonl` is logged (the ledger discipline — never
    /// silently skipped) and treated as non-matching on the dedupe read, so
    /// the next emit still appends (the dedupe read survived the garbage)
    /// and a genuinely unacked fact still suppresses.
    #[test]
    fn malformed_dedupe_line_is_logged_and_non_matching() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // A garbage line + a valid UNACKED line for a DIFFERENT fact (w9).
        fs::create_dir_all(dir.path().join(BRAIN_DIR)).unwrap();
        fs::write(
            drop_path(root),
            format!(
                "not json at all\n{}\n",
                serde_json::json!({
                    "fact_id": "w9", "title": "t", "body": "b", "quote": "q",
                    "query": "shipping checklist", "surface": "wiki_search", "ts": 1
                })
            ),
        )
        .unwrap();

        // The emit for w1 must APPEND — the malformed line is logged and
        // read as non-matching, never a silent abort of the dedupe read.
        emit_unverified_match(root, scratch_ev("w1"));
        let text = fs::read_to_string(drop_path(root)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "garbage + unacked w9 + new w1: the dedupe read survived: {lines:?}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(lines[2]).is_ok(),
            "the new w1 line is well-formed JSON"
        );

        // The unacked w9 line still suppresses its own re-emit (the read
        // treated the garbage as non-matching, not as "file unreadable").
        emit_unverified_match(root, scratch_ev("w9"));
        assert_eq!(
            fs::read_to_string(drop_path(root)).unwrap().lines().count(),
            3,
            "unacked w9 dedupes despite the malformed sibling line"
        );
    }

    #[test]
    fn sidecar_shape_has_no_tauri_dependency() {
        // The sidecar links this module's shape: it must compile without any
        // tauri import. Enforce on the source text (needles are assembled from
        // parts so this test module never contains a real tauri path token).
        let src = include_str!("prompt_drop.rs");
        let needle = format!("{}{}", "tauri", "::");
        assert!(
            !src.contains(&needle),
            "prompt_drop must not reference {needle} (sidecar links this shape)"
        );
        let use_needle = format!("use {needle}");
        assert!(!src.contains(&use_needle));
    }
}
