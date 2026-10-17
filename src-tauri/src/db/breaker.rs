//! Cross-process bulk-deletion circuit breaker (issue #271 spec §8(b) + §10).
//!
//! Three heal/destructive writers share ONE budget in `llm_wiki_meta`:
//! the scheduler + `ct heal` (via `db::heal::heal_invalid_sources_conn`),
//! the GUI "Heal Database" button (`lib.rs::heal_lost_librarian_inferred`),
//! and regrade's hard-delete purge (`db::evidence_regrade::purge_doomed_rows`).
//! The counter lives in the database, so concurrent processes (desktop app,
//! `ct` CLI, MCP sidecar) share the same spent budget with no TOCTOU —
//! every increment happens inside an IMMEDIATE transaction.
//!
//! Key layout in `llm_wiki_meta` (values are decimal strings):
//!
//! - `breaker_epoch_ts` — epoch START (unix ms). Healthy epochs roll over on
//!   a 24h cadence; an epoch that TRIPS never rolls over (rev 5, Opus r3 M5:
//!   rolling epochs with shrinking L would let each epoch spend a full budget
//!   and feed rows to the 7-day prune).
//! - `breaker_spent` — the SHARED spend counter (all writers, both
//!   thresholds).
//! - `breaker_L_heal` / `breaker_L_regrade` — each writer's denominator L
//!   snapshotted under its OWN key (rev 5, Opus r3 m1), written whenever
//!   that writer runs a check.
//!
//! Threshold math is `max(⌈0.05·L⌉, 10)` for BOTH writers (plan Global
//! Constraints pin), each computed from its OWN denominator:
//! heal's selection (`source_type='librarian_inferred' AND source_ref IS NOT
//! NULL AND deleted_at IS NULL`) and regrade's `librarian_evidence JOIN live
//! entries` count. Denominators stay separate per writer exactly as pinned.
//!
//! Refusal rules, in order:
//! 1. **Tripped epoch** — `spent >= own_threshold` refuses everything until
//!    an operator resets (`ct heal --reset-breaker`).
//! 2. **Full population (rev-34 Small-L rule)** — `|doomed| >= L` refuses
//!    outright whatever the budget says: a batch that would remove the
//!    ENTIRE selected population is the brain-wipe shape this breaker exists
//!    to stop, and the threshold floor does NOT protect small populations
//!    (for L ≤ 10 the budget is ≥ L).
//! 3. **Budget** — `|doomed| > remaining` refuses the whole batch
//!    (all-or-nothing; never a partial delete).
//!
//! The sanctioned escape is the pinned one-run `--allow-bulk` flag, which
//! skips the CHECK for that run — the trip/snapshot bookkeeping is not
//! evaluated and no refusal can fire. Deletes that proceed under
//! `--allow-bulk` still SPEND the shared counter (`breaker_spend` runs on
//! the successful delete path regardless of the flag) — one shared budget,
//! even for sanctioned passes (pr283-review finding 8: the earlier wording
//! "never touches these keys" was wrong; the spec pins the flag as a
//! one-run CHECK skip, not a budget exemption).

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

/// Meta key holding heal's snapshotted denominator L.
pub const BREAKER_L_HEAL: &str = "breaker_L_heal";
/// Meta key holding regrade's snapshotted denominator L.
pub const BREAKER_L_REGRADE: &str = "breaker_L_regrade";
/// Meta key holding the SHARED spend counter (decimal string).
const BREAKER_SPENT_KEY: &str = "breaker_spent";
/// Meta key holding the epoch start (unix ms, decimal string).
const BREAKER_EPOCH_TS_KEY: &str = "breaker_epoch_ts";
/// Persisted `1` once ANY writer's refusal trips the epoch. Fix round (rev of
/// task 10): trip-ness used to be inferred per-writer from
/// `spent_before < own_threshold`, which let a HIGHER-threshold writer roll a
/// tripped epoch >24h later and reset spent — erasing the trip and reopening
/// a fresh budget. The marker lives in the same shared `llm_wiki_meta` table,
/// so it is visible to every writer and cleared only by [`breaker_reset`].
const BREAKER_TRIPPED_KEY: &str = "breaker_tripped";
/// Healthy-epoch rollover cadence (spec §10: 24h; tripped epochs never roll).
const EPOCH_ROLLOVER_MS: i64 = 24 * 60 * 60 * 1000;
/// Threshold floor — pinned by the plan's Global Constraints.
const THRESHOLD_FLOOR: i64 = 10;

/// Verdict of one [`breaker_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakerVerdict {
    /// The batch may proceed (and should spend via [`breaker_spend`]).
    Allow,
    /// The whole batch is refused; `reason` names the threshold that was hit
    /// (the tripping writer's refusal line must say which one).
    Refuse { reason: String },
}

fn read_meta(conn: &Connection, key: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [key],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.parse::<i64>().ok()))
}

fn write_meta(conn: &Connection, key: &str, value: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value.to_string()],
    )?;
    Ok(())
}

/// The pinned threshold math: `max(⌈0.05·L⌉, 10)` for ALL writers.
fn threshold_for(l: i64) -> i64 {
    // (l*5+99)/100 == ceil(0.05*l) for l >= 0 without the unstable
    // `int_roundings` feature (rust 1.73 pin).
    let five_pct = (l * 5 + 99) / 100;
    five_pct.max(THRESHOLD_FLOOR)
}

/// Heal's denominator L: the pass's own selection — live
/// `librarian_inferred` rows carrying a source_ref (spec §10 pins the SQL).
pub fn heal_denominator(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entries
          WHERE deleted_at IS NULL
            AND source_ref IS NOT NULL
            AND source_type = 'librarian_inferred'",
        [],
        |r| r.get(0),
    )?)
}

/// Heal's threshold from ITS denominator.
pub fn heal_threshold(conn: &Connection) -> Result<i64> {
    Ok(threshold_for(heal_denominator(conn)?))
}

/// Regrade's denominator L: `librarian_evidence` rows joined to LIVE entries
/// (a different table than heal's — spec §10 pins the SQL verbatim).
pub fn regrade_denominator(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM librarian_evidence le
          JOIN llm_wiki_entries e ON e.id = le.entry_id
          WHERE e.deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?)
}

/// Regrade's threshold from ITS denominator.
pub fn regrade_threshold(conn: &Connection) -> Result<i64> {
    Ok(threshold_for(regrade_denominator(conn)?))
}

/// The shared spent counter (`breaker_spent`), 0 when absent.
pub fn breaker_spent(conn: &Connection) -> Result<i64> {
    Ok(read_meta(conn, BREAKER_SPENT_KEY)?.unwrap_or(0))
}

/// Increment the SHARED spend counter. MUST be called inside each writer's
/// per-row/per-batch IMMEDIATE transaction so concurrent processes cannot
/// race the read-then-write (spec §10: "incremented inside each per-row
/// IMMEDIATE transaction").
pub fn breaker_spend(conn: &Connection, n: i64) -> Result<()> {
    let spent = breaker_spent(conn)?;
    write_meta(conn, BREAKER_SPENT_KEY, spent + n)
}

/// Snapshot the caller's OWN denominator L under its OWN key (rev 5, Opus
/// r3 m1). A writer with nothing selected (L = 0) must not clobber a real
/// snapshot.
fn snapshot_l(conn: &Connection, key: &str, own_l: i64) -> Result<()> {
    if own_l > 0 {
        write_meta(conn, key, own_l)?;
    }
    Ok(())
}

/// The cross-process check. `own_l` is the CALLER's denominator and
/// `own_threshold` the threshold computed from it; `doomed_count` is the
/// batch size about to be deleted; `l_key` selects which `breaker_L_*` key
/// the caller's L snapshots under.
///
/// Ordering: tripped-epoch → full-population (`|doomed| >= L`, rev-34) →
/// budget. A refusal is ALL-OR-NOTHING — the caller must delete nothing.
/// Every production caller runs this inside its IMMEDIATE transaction, so
/// the read-then-increment budget window can neither race nor hit an
/// upgrade `SQLITE_BUSY` (spec §8(b)); the epoch bookkeeping writes ride the
/// caller's lock. On an autocommit connection each statement is still atomic
/// per statement (tests, the reset arm).
pub fn breaker_check(
    conn: &Connection,
    doomed_count: usize,
    own_l: i64,
    own_threshold: i64,
    l_key: &str,
) -> Result<BreakerVerdict> {
    let now_ms = crate::db::commit::ms_now();
    let doomed = doomed_count as i64;

    // Epoch bookkeeping. A missing epoch starts one; a HEALTHY epoch older
    // than 24h rolls over (spent resets to 0, start re-stamps, baseline L is
    // re-snapshotted). A TRIPPED epoch never rolls over — spent stays at the
    // threshold and the old start stays, so the refusal persists until an
    // operator resolves the cause and resets the keys explicitly (spec §10).
    let spent_before = breaker_spent(conn)?;
    let tripped = read_meta(conn, BREAKER_TRIPPED_KEY)?.unwrap_or(0) != 0;
    match read_meta(conn, BREAKER_EPOCH_TS_KEY)? {
        None => {
            write_meta(conn, BREAKER_EPOCH_TS_KEY, now_ms)?;
        }
        // Fix round (rev of task 10): a MARKED-tripped epoch never rolls,
        // regardless of any individual writer's threshold — trip-ness is a
        // persisted, shared fact, not something re-inferred per writer from
        // `spent_before < own_threshold`.
        Some(_) if tripped => {}
        Some(ts) if now_ms - ts >= EPOCH_ROLLOVER_MS && spent_before < own_threshold => {
            write_meta(conn, BREAKER_EPOCH_TS_KEY, now_ms)?;
            write_meta(conn, BREAKER_SPENT_KEY, 0)?;
        }
        _ => {}
    }
    snapshot_l(conn, l_key, own_l)?;

    // Rule 1: tripped epoch. Two ways in: the persisted marker (ANY writer's
    // earlier refusal — a higher-threshold writer is refused TOO, not handed
    // a fresh budget via rollover) or this writer's own budget being spent
    // out. The refusal line names THIS writer's threshold (spec §10).
    let spent = breaker_spent(conn)?;
    if tripped || spent >= own_threshold {
        // Persist the marker so every other writer sees the trip as well.
        write_meta(conn, BREAKER_TRIPPED_KEY, 1)?;
        return Ok(BreakerVerdict::Refuse {
            reason: format!(
                "bulk-deletion breaker TRIPPED: shared spent {spent} / this writer's threshold {own_threshold} (a tripped epoch does NOT roll over — investigate, then `ct heal --reset-breaker`)"
            ),
        });
    }

    // Rule 2 (rev-34 Small-L): the threshold floor does NOT protect small
    // populations — a batch removing the ENTIRE selected population is
    // refused outright, whatever L is.
    if own_l > 0 && doomed >= own_l {
        // Same shared marker: a Small-L refusal trips the epoch too (any
        // writer's refusal is a trip; spec: "the epoch is MARKED tripped
        // when ANY writer refuses").
        write_meta(conn, BREAKER_TRIPPED_KEY, 1)?;
        return Ok(BreakerVerdict::Refuse {
            reason: format!(
                "bulk-deletion breaker: |doomed| {doomed} >= L {own_l} — refusing the ENTIRE selected population (use `--allow-bulk` only after investigating)"
            ),
        });
    }

    // Rule 3: budget (all-or-nothing — never a partial delete).
    let remaining = own_threshold - spent;
    if doomed > remaining {
        write_meta(conn, BREAKER_TRIPPED_KEY, 1)?;
        return Ok(BreakerVerdict::Refuse {
            reason: format!(
                "bulk-deletion breaker: |doomed| {doomed} > remaining budget {remaining} (shared spent {spent} / threshold {own_threshold})"
            ),
        });
    }

    Ok(BreakerVerdict::Allow)
}

/// `ct heal --reset-breaker`: delete the epoch timestamp, the shared spent
/// counter, the tripped marker and both `breaker_L_*` keys in ONE IMMEDIATE
/// transaction (the transaction is opened HERE so no caller can reset
/// partially — spec §10 pins the single-txn shape).
pub fn breaker_reset(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    for key in [
        BREAKER_EPOCH_TS_KEY,
        BREAKER_SPENT_KEY,
        BREAKER_TRIPPED_KEY,
        BREAKER_L_HEAL,
        BREAKER_L_REGRADE,
    ] {
        tx.execute("DELETE FROM llm_wiki_meta WHERE key = ?1", [key])?;
    }
    tx.commit()?;
    Ok(())
}
