//! Duplicate merge sweep (plan Task 6, spec r21 §2.7 / R2.7.1–R2.7.6, with
//! the R2.2.8 FINAL RULE drift gate and the r17-m3 precondition predicate).
//!
//! Library surface ONLY in this task: the precondition predicate, drift
//! gating, grouping, and the destructive pass are `tauri_app_lib` functions
//! tested as such (plan-p6-m3); the `WikiCmd::MergeDuplicates` clap
//! subcommand and its CLI rows arrive in Task 8 (`tools/src/bin/ct.rs` is
//! untouched).
//!
//! Order of operations per run:
//!   1. precondition (R2.7.1/r17-m3): the `alias_remap_completed` MARKER in
//!      `llm_wiki_meta` must be present before the destructive pass — a
//!      marker, not a live-row predicate, so report-only heal rows never
//!      block merging forever. Missing marker → apply refused with
//!      `alias_remap_not_run` (the report arm still computes groups).
//!   2. drift watermark compare (R2.2.8 FINAL RULE): an unconfirmed drift
//!      report blocks the destructive pass unless `--confirm-drift`/
//!      `--waive-drift` carries the echoed old hash. Merge NEVER writes the
//!      watermark — heal is the sole writer (R2.2.8 writer rule).
//!   3. grouping (R2.7.2): live, non-redirected entities grouped by
//!      punctuation-normalized name; archived rows (`deleted_at IS NOT
//!      NULL`) are never candidates (R2.7.5 r21).
//!   4. disposition per group: type conflict → queue (R2.7.4); summaries
//!      must be non-empty AND normalized-equal to auto-merge (R2.7.6 r21 —
//!      both-empty and empty-vs-nonempty are NOT agreement, r8-M3).
//!   5. apply: ONE `ImmediateTx` per merge GROUP (Global Constraints); the
//!      survivor is the byte-wise/BINARY lowest id (R2.7.3 — never COLLATE
//!      NOCASE or locale-aware, in Rust or SQL); redirect rows are written
//!      with path compression (r2-m6) and a cycle guard that reports,
//!      never loops.
//!
//! STDOUT CONTRACT: like `heal_ontology`, this module never prints to
//! stdout — human-readable notes go to STDERR so the Task 8 CLI can print
//! exactly one JSON object.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db::entity_gate::ImmediateTx;
use crate::db::heal_ontology::{read_watermark, DriftFlag, DriftReport, ALIAS_REMAP_MARKER_KEY};

/// Why a group was not auto-merged (review-queue disposition).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueReason {
    /// Members disagree on `entity_type` post-remap (R2.7.4).
    TypeConflict,
    /// Summaries are not both/all non-empty and normalized-equal (R2.7.6
    /// r21; includes the both-empty and empty-vs-nonempty cases, r8-M3).
    SummaryMismatch,
}

/// One duplicate group: the computed survivor, losers, and disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MergeGroup {
    /// The punctuation-normalized name the members grouped under (R2.7.2).
    pub normalized_name: String,
    /// Byte-wise sorted member ids (BINARY; R2.7.3).
    pub members: Vec<String>,
    /// The byte-wise lowest member id — the deterministic survivor.
    pub survivor: String,
    /// `members` minus the survivor.
    pub losers: Vec<String>,
    /// `Some(reason)` → queued for review; `None` → auto-merge eligible.
    pub queued: Option<QueueReason>,
}

/// The merge sweep output (report and `--yes` arms share this shape).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct MergeDuplicatesReport {
    /// The R2.2.8 drift section, when the stored watermark differs from
    /// the live config hash (echoed for `--confirm-drift`/`--waive-drift`).
    pub drift: Option<DriftReport>,
    /// Every duplicate group found this run, in byte-wise id order.
    pub groups: Vec<MergeGroup>,
    /// Groups auto-merged. Dry run: groups that WOULD merge. `--yes`:
    /// groups whose redirects actually COMMITTED this run (review finding)
    /// — 0 when the destructive pass is refused (`skipped_reason`), and a
    /// refused/rolled-back group is not counted.
    pub merged_groups: usize,
    /// Groups queued for a Kurt ruling.
    pub queued_groups: usize,
    /// Every redirect row written this run, `(loser, survivor)` — the
    /// hand-reversal handle (R2.7.5 r21: deleting a loser's row fully
    /// restores it).
    pub redirects_written: Vec<(String, String)>,
    /// Pre-existing rows REWRITTEN by path compression (r2-m6): `(entity_id,
    /// old merged_into, new merged_into)`. Part of the same hand-reversal
    /// handle (R2.7.5 r21): reverting a merge by hand must restore these
    /// rows' ORIGINAL targets, not just delete the fresh ones.
    pub redirects_rewritten: Vec<(String, String, String)>,
    /// Edges between group members that become resolved self-loops on
    /// read (R2.7.5): `(edge id, source, target)` — listed so the user
    /// can prune; never auto-hidden.
    pub self_loops: Vec<(String, String, String)>,
    /// Ids whose hand-crafted redirect chains loop (r2-m6): reported,
    /// never followed, never rewritten.
    pub cycles: Vec<String>,
    /// A fault caught here so the caller still gets one report object.
    pub error: Option<String>,
    /// `"alias_remap_not_run"` | `"unconfirmed_drift"` — set when the
    /// destructive pass was refused.
    pub skipped_reason: Option<String>,
}

/// Result of the R2.7.1/r17-m3 precondition predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergePrecondition {
    /// The `alias_remap_completed` marker is present.
    RemapDone,
    /// The signed-alias remap has not run for this vault — the destructive
    /// pass must refuse (pre-remap groups would queue as false type
    /// conflicts, R2.6.4); the caller offers to run the remap first.
    RemapNotRun,
}

/// Consult the `alias_remap_completed` MARKER (r17-m3: marker presence
/// only — never a live-row predicate, so report-only heal rows cannot
/// block merging forever). The marker is deleted by `clear_vault_tables`.
pub fn merge_precondition(conn: &Connection) -> MergePrecondition {
    let present: Option<String> = conn
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [ALIAS_REMAP_MARKER_KEY],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    if present.is_some() {
        MergePrecondition::RemapDone
    } else {
        MergePrecondition::RemapNotRun
    }
}

/// Run the duplicate merge sweep. `apply == false` is the read-only report:
/// every group and disposition computed, NOTHING written. Errors are caught
/// into [`MergeDuplicatesReport::error`].
pub fn merge_duplicates_pass(
    conn: &mut Connection,
    flag: DriftFlag,
    apply: bool,
) -> MergeDuplicatesReport {
    let mut report = MergeDuplicatesReport::default();
    if let Err(e) = run(conn, flag, apply, &mut report) {
        report.error = Some(format!("{e:#}"));
    }
    report
}

fn run(
    conn: &mut Connection,
    flag: DriftFlag,
    apply: bool,
    report: &mut MergeDuplicatesReport,
) -> Result<()> {
    // Old-schema database on a read-only path: report, never fail — the
    // same posture as `ontology_heal_pass` (plan-p9-M3).
    let has = |name: &str| crate::db::heal_ontology::table_exists(conn, name);
    if !has("llm_wiki_meta")? || !has("entity_redirects")? || !has("curated_entities")? {
        eprintln!(
            "merge-duplicates: schema pending (read-only) — run `ct heal --yes` once \
             to migrate"
        );
        return Ok(());
    }

    // 1. Drift compare (R2.2.8). The live hash is computed exactly as heal
    //    computes it; first run (no row) → no report (r7-m4).
    let policy = crate::config::ingest_policy_for_db(conn.path());
    let live_hash = crate::config::ontology_config_watermark_hash(
        &policy.tiers,
        policy.ontology_selection,
        policy.ontology_unparseable,
    );
    if let Some((old_hash, old_ts)) = read_watermark(conn)? {
        if old_hash != live_hash {
            report.drift = Some(DriftReport {
                old_hash: old_hash.clone(),
                old_stamped_at: old_ts,
                new_hash: live_hash.clone(),
                confirmed: false,
                waived: false,
            });
            eprintln!(
                "merge-duplicates: ontology drift pending (old {old_hash} stamped_at \
                 {old_ts} vs live {live_hash}); confirm with --confirm-drift {old_hash} \
                 or acknowledge with --waive-drift {old_hash}"
            );
        }
    }

    // 2. Grouping + disposition (identical for report and apply arms —
    //    the report must show exactly what --yes would do).
    report.groups = find_duplicate_groups(conn)?;
    for g in &report.groups {
        if g.queued.is_some() {
            report.queued_groups += 1;
        } else {
            report.merged_groups += 1;
        }
    }

    // 3. Precondition + FINAL RULE drift gate — both block ONLY the
    //    destructive pass (the report above still prints).
    if !apply {
        return Ok(());
    }
    // From here `merged_groups` means APPLIED: a refusal below must not
    // report the planned merges as done (the groups list still shows them).
    report.merged_groups = 0;
    if matches!(merge_precondition(conn), MergePrecondition::RemapNotRun) {
        report.skipped_reason = Some("alias_remap_not_run".into());
        eprintln!(
            "merge-duplicates: the signed-alias remap has not run for this vault — \
             run `ct heal --yes` once first (R2.7.1), else pre-remap groups queue as \
             false type conflicts"
        );
        return Ok(());
    }
    match (&report.drift, &flag) {
        (Some(_), DriftFlag::None) => {
            report.skipped_reason = Some("unconfirmed_drift".into());
            eprintln!(
                "merge-duplicates: unconfirmed drift report — destructive pass \
                 refused this run (R2.2.8 FINAL RULE)"
            );
            return Ok(());
        }
        (Some(d), DriftFlag::Confirm(h)) | (Some(d), DriftFlag::Waive(h)) => {
            if h != &d.old_hash {
                eprintln!(
                    "merge-duplicates: drift flag {h:?} does not match the echoed old \
                     hash {} — echoed pair NOT confirmed",
                    d.old_hash
                );
                report.skipped_reason = Some("unconfirmed_drift".into());
                return Ok(());
            }
            // Both flags clear the FINAL-RULE block for MERGES (R2.2.8):
            // drift concerns the ontology config, not name grouping. The
            // watermark is NEVER written here — heal is the sole writer.
            if let DriftFlag::Waive(_) = flag {
                report.drift = report.drift.take().map(|mut d| {
                    d.waived = true;
                    d
                });
            } else {
                report.drift = report.drift.take().map(|mut d| {
                    d.confirmed = true;
                    d
                });
            }
        }
        (None, DriftFlag::Confirm(_)) | (None, DriftFlag::Waive(_)) => {
            eprintln!(
                "merge-duplicates: no drift report fired this run — \
                 --confirm-drift/--waive-drift ignored"
            );
        }
        (None, DriftFlag::None) => {}
    }

    // 4. Apply: ONE ImmediateTx per merge GROUP (never one per sweep).
    //    `merged_groups` counts only groups that COMMITTED.
    for group in report.groups.clone() {
        if group.queued.is_some() {
            continue;
        }
        if apply_group(conn, &group, report) {
            report.merged_groups += 1;
        }
    }
    Ok(())
}

/// R2.7.2 normalization: fold punctuation/parentheses variants together
/// before matching (also the R2.7.6 summary-comparison normalization —
/// "same normalization as §2.7.2"). ASCII-lowercase, drop every char that
/// is not alphanumeric or whitespace, collapse whitespace runs, trim. This
/// is NOT used for survivor selection — ids compare byte-wise (R2.7.3).
pub fn normalize_merge_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            // Full Unicode lowercase (`char::to_lowercase`), not
            // `to_ascii_lowercase`: the key's own doc example is
            // " Café—Olga! ", and ASCII-only filtering made "Zoë" and "Zo"
            // collide into ONE duplicate group — a deterministic WRONG
            // auto-merge with agreeing summaries (final-review
            // fix-before-merge). Multi-char lowercases (İ → i̇) are
            // handled by `extend`.
            out.extend(ch.to_lowercase());
        } else if ch.is_whitespace() {
            pending_space = true;
        } else {
            // Punctuation (including parentheses) is dropped entirely; a
            // run of drops must not glue the surrounding words together
            // ("Intent (x)" → "intent x", not "intentx"), so it also
            // implies a separator when something was already emitted.
            pending_space = true;
        }
    }
    out
}

/// One candidate row from the grouping scan.
#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    name: String,
    entity_type: String,
    summary: String,
}

/// Group live, non-redirected entities by normalized name (R2.7.1/R2.7.2):
/// archived members (`deleted_at IS NOT NULL`) are never candidates
/// (R2.7.5 r21 — an archived cluster is never re-merged), and rows with an
/// `entity_redirects` row are excluded so the sweep does not re-detect a
/// merged-away loser by name every run. The SQL orders and the survivor
/// selection compares byte-wise/BINARY (R2.7.3) — deliberately NO
/// `COLLATE NOCASE` anywhere (that is display-order only, `bundle_io.rs`).
fn find_duplicate_groups(conn: &Connection) -> Result<Vec<MergeGroup>> {
    let candidates: Vec<Candidate> = {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.entity_type, e.summary
             FROM curated_entities e
             WHERE e.deleted_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM entity_redirects r WHERE r.entity_id = e.id
               )
             ORDER BY e.id",
        )?;
        let mut rs = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rs.next()? {
            out.push(Candidate {
                id: row.get(0)?,
                name: row.get(1)?,
                entity_type: row.get(2)?,
                summary: row.get(3)?,
            });
        }
        out
    };

    let mut by_key: HashMap<String, Vec<Candidate>> = HashMap::new();
    for c in candidates {
        by_key
            .entry(normalize_merge_key(&c.name))
            .or_default()
            .push(c);
    }

    let mut groups: Vec<MergeGroup> = by_key
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .map(|(key, mut members)| {
            // Rust `str` ordering IS byte-wise (lexicographic on UTF-8
            // bytes) and the SQL above already ordered BINARY — the sort
            // here is belt-and-braces for the HashMap shuffle.
            members.sort_by(|a, b| a.id.cmp(&b.id));
            let survivor = members[0].id.clone();
            let losers = members[1..].iter().map(|m| m.id.clone()).collect();
            let queued = disposition(&members);
            MergeGroup {
                normalized_name: key,
                members: members.iter().map(|m| m.id.clone()).collect(),
                survivor,
                losers,
                queued,
            }
        })
        .collect();
    groups.sort_by(|a, b| a.survivor.cmp(&b.survivor));
    Ok(groups)
}

/// R2.7.4 + R2.7.6 (r21 wording): auto-merge only when every member's
/// `entity_type` agrees AND every summary is non-empty and
/// normalized-equal (same normalization as the name key). Anything else
/// queues — both-empty and empty-vs-nonempty are NOT agreement (r8-M3).
fn disposition(members: &[Candidate]) -> Option<QueueReason> {
    let types: HashSet<&str> = members.iter().map(|m| m.entity_type.as_str()).collect();
    if types.len() > 1 {
        return Some(QueueReason::TypeConflict);
    }
    let summaries: Vec<String> = members
        .iter()
        .map(|m| normalize_merge_key(&m.summary))
        .collect();
    if summaries.iter().any(|s| s.is_empty()) {
        return Some(QueueReason::SummaryMismatch);
    }
    if summaries.iter().any(|s| *s != summaries[0]) {
        return Some(QueueReason::SummaryMismatch);
    }
    None
}

/// Cycle-aware chain resolution (r2-m6): follow `merged_into` hops until a
/// row with no redirect; a revisit (hand-crafted cycle) returns `Cycle`
/// with an id on the loop — the caller reports it, never loops. Merge-time
/// path compression means healthy chains are at most one hop deep, but the
/// walk is bounded by the visited set regardless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainResolution {
    /// No redirect row exists for the input id; it is its own final id.
    None,
    /// The final survivor at the end of the (compressed) chain.
    Survivor(String),
    /// A cycle was detected; the String is an id on the loop.
    Cycle(String),
}

pub fn resolve_redirect_chain(conn: &Connection, entity_id: &str) -> Result<ChainResolution> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut cur = entity_id.to_string();
    loop {
        if !visited.insert(cur.clone()) {
            return Ok(ChainResolution::Cycle(cur));
        }
        let next: Option<String> = conn
            .query_row(
                "SELECT merged_into FROM entity_redirects WHERE entity_id = ?1",
                [&cur],
                |r| r.get(0),
            )
            .optional()?;
        match next {
            None => {
                return Ok(if cur == entity_id {
                    ChainResolution::None
                } else {
                    ChainResolution::Survivor(cur)
                });
            }
            Some(n) => cur = n,
        }
    }
}

/// Apply ONE auto-merge group in ONE `ImmediateTx` (Global Constraints):
/// write `loser → survivor` redirect rows with path compression (r2-m6 —
/// when a rewrite target is itself a loser, its chain is resolved first
/// and the final survivor stored), compress every pre-existing row whose
/// chain now passes through this group's losers, and collect the group's
/// resolved self-loop edges for the report (R2.7.5). A hand-crafted cycle
/// touching this group's chains is recorded in `report.cycles` and left
/// untouched — it must not loop or fail the group's transaction.
/// Returns whether the group's redirects COMMITTED (`false` = refused or
/// rolled back; the reason is on `report`).
fn apply_group(
    conn: &mut Connection,
    group: &MergeGroup,
    report: &mut MergeDuplicatesReport,
) -> bool {
    // Every failure is contained to THIS group (one tx per group — a
    // failure must not poison unrelated groups) and recorded, never
    // overwritten: `report.error` accumulates one entry per failed group.
    let fail = |report: &mut MergeDuplicatesReport, msg: String| {
        report.error = Some(match report.error.take() {
            Some(prev) => format!("{prev}; {msg}"),
            None => msg,
        });
        false
    };

    // Defensive cycle guard: the grouping scan excludes redirected rows,
    // so the survivor cannot carry a redirect row — if hand-edited data
    // made it one, refuse this group loudly rather than write a loop.
    match resolve_redirect_chain(conn, &group.survivor) {
        Err(e) => return fail(report, format!("group {} failed: {e:#}", group.survivor)),
        Ok(ChainResolution::Cycle(id)) => {
            report.cycles.push(id.clone());
            eprintln!(
                "merge-duplicates: survivor {} sits on a redirect cycle ({id}) — \
                 group refused",
                group.survivor
            );
            return false;
        }
        Ok(ChainResolution::None) => {}
        Ok(ChainResolution::Survivor(_)) => {
            eprintln!(
                "merge-duplicates: survivor {} already redirected — group refused \
                 (re-run the sweep)",
                group.survivor
            );
            return false;
        }
    }

    let tx = match ImmediateTx::begin(conn) {
        Ok(tx) => tx,
        Err(e) => {
            return fail(
                report,
                format!("group {} failed to begin: {e:#}", group.survivor),
            )
        }
    };
    let now = crate::db::commit::now_timestamps().0;
    let mut wrote: Vec<(String, String)> = Vec::new();
    let mut rewritten: Vec<(String, String, String)> = Vec::new();
    let mut cycles: Vec<String> = Vec::new();
    let mut self_loops: Vec<(String, String, String)> = Vec::new();
    let result = write_group_redirects(
        &tx,
        group,
        now,
        &mut wrote,
        &mut rewritten,
        &mut cycles,
        &mut self_loops,
    );
    if let Err(e) = result {
        let mut msg = format!("group {} failed: {e:#}", group.survivor);
        if let Err(rb) = tx.rollback() {
            msg.push_str(&format!("; rollback failed: {rb:#}"));
        }
        return fail(report, msg);
    }
    if let Err(e) = tx.commit() {
        return fail(
            report,
            format!("group {} failed to commit: {e:#}", group.survivor),
        );
    }
    report.redirects_written.extend(wrote);
    report.redirects_rewritten.extend(rewritten);
    report.cycles.extend(cycles);
    report.self_loops.extend(self_loops);
    true
}

#[allow(clippy::type_complexity)]
fn write_group_redirects(
    tx: &ImmediateTx<'_>,
    group: &MergeGroup,
    now: i64,
    wrote: &mut Vec<(String, String)>,
    // Compression rewrites, `(entity_id, old merged_into, new merged_into)`
    // — reported so hand-reversal can restore the ORIGINAL target
    // (R2.7.5 r21: the report lists every redirect row it touched).
    rewritten: &mut Vec<(String, String, String)>,
    cycles: &mut Vec<String>,
    self_loops: &mut Vec<(String, String, String)>,
) -> Result<()> {
    // 1. loser → survivor rows. `merged_into` is always the FINAL
    //    survivor: the survivor is unredirected (guard above), so no
    //    further resolution is needed for the fresh rows themselves.
    for loser in &group.losers {
        tx.execute(
            "INSERT OR REPLACE INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES (?1, ?2, ?3)",
            params![loser, group.survivor, now],
        )?;
        wrote.push((loser.clone(), group.survivor.clone()));
    }

    // 2. Self-loop census (R2.7.5): edges between DISTINCT group members
    //    resolve to survivor→survivor on read — real pre-merge provenance
    //    that is SHOWN with its original attributes, but listed here so
    //    the user can prune. Never auto-hidden, never dropped.
    {
        let placeholders = vec!["?"; group.members.len()].join(",");
        let sql = format!(
            "SELECT id, source_id, target_id FROM llm_wiki_edges
             WHERE source_id != target_id
               AND source_id IN ({placeholders})
               AND target_id IN ({placeholders})",
        );
        let mut stmt = tx.prepare(&sql)?;
        let params: Vec<&str> = group
            .members
            .iter()
            .chain(group.members.iter())
            .map(String::as_str)
            .collect();
        let mut rs = stmt.query(rusqlite::params_from_iter(params))?;
        while let Some(row) = rs.next()? {
            self_loops.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
    }

    // 3. Path compression (r2-m6): EVERY pre-existing row is re-resolved
    //    through the fresh rows (visible inside this tx) and rewritten to
    //    its FINAL survivor (A→B, B merges into C ⇒ A rewrites to A→C;
    //    deeper chains flatten in the same pass). A chain that loops is
    //    recorded in `cycles` and left untouched — reported, never
    //    looped, never rewritten onto itself.
    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT entity_id, merged_into FROM entity_redirects")?;
        let mut rs = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rs.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        out
    };
    for (entity_id, merged_into) in rows {
        match resolve_redirect_chain(tx, &merged_into)? {
            ChainResolution::Survivor(final_id) => {
                if final_id != merged_into && final_id != entity_id {
                    tx.execute(
                        "UPDATE entity_redirects SET merged_into = ?1 WHERE entity_id = ?2",
                        params![final_id, entity_id],
                    )?;
                    rewritten.push((entity_id, merged_into, final_id));
                } else if final_id == entity_id {
                    // Compressing onto itself would forge a self-loop
                    // cycle — refuse and surface instead.
                    cycles.push(entity_id.clone());
                }
            }
            ChainResolution::Cycle(id) => cycles.push(id),
            ChainResolution::None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::db::heal_ontology::{INITIAL_WATERMARK_KEY, WATERMARK_KEY};

    fn seed(conn: &Connection, id: &str, name: &str, entity_type: &str, summary: &str) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, 1)",
            params![id, name, entity_type, summary],
        )
        .unwrap();
    }

    fn archive(conn: &Connection, id: &str) {
        conn.execute(
            "UPDATE curated_entities SET deleted_at = 9 WHERE id = ?1",
            [id],
        )
        .unwrap();
    }

    fn redirect(conn: &Connection, loser: &str, survivor: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES (?1, ?2, 1)",
            params![loser, survivor],
        )
        .unwrap();
    }

    fn merged_into(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT merged_into FROM entity_redirects WHERE entity_id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
    }

    fn redirect_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM entity_redirects", [], |r| r.get(0))
            .unwrap()
    }

    /// Marker present → precondition passes; no drift row (first run,
    /// r7-m4) → no drift report. The baseline for merge-apply tests.
    fn armed(conn: &Connection) {
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES ('alias_remap_completed', '1')",
            [],
        )
        .unwrap();
    }

    fn set_watermark(conn: &Connection, v: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES (?1, ?2)",
            params![WATERMARK_KEY, v],
        )
        .unwrap();
    }

    // ---- §6 item 4, merge-side rows -------------------------------------

    /// Deterministic BINARY survivor: byte-wise lowest id, with the stated
    /// `ent_*` < `entity::*` (0x5F < 0x69) bias pinned.
    #[test]
    fn binary_survivor_is_byte_wise_lowest_id() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "entity::aaa", "Adrian", "concept", "same summary");
        seed(&conn, "ent_zzz", "Adrian", "concept", "same summary");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.error, None, "{r:?}");
        assert_eq!(r.merged_groups, 1, "{r:?}");
        let g = &r.groups[0];
        assert_eq!(g.survivor, "ent_zzz", "ent_* must sort below entity::*");
        assert_eq!(g.losers, vec!["entity::aaa".to_string()]);
        assert_eq!(
            merged_into(&conn, "entity::aaa").as_deref(),
            Some("ent_zzz")
        );
        assert_eq!(
            r.redirects_written,
            vec![("entity::aaa".to_string(), "ent_zzz".to_string())],
            "the report lists every redirect row written"
        );
    }

    /// A failing group is contained and RECORDED: with every group's
    /// redirect write failing, nothing merges and `report.error` names
    /// EVERY failed group, not just the last one.
    #[test]
    fn every_failed_group_is_recorded() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "ent_a1", "Adrian", "concept", "same");
        seed(&conn, "ent_a2", "Adrian", "concept", "same");
        seed(&conn, "ent_b1", "Bianca", "concept", "same");
        seed(&conn, "ent_b2", "Bianca", "concept", "same");
        conn.execute_batch(
            "CREATE TRIGGER boom BEFORE INSERT ON entity_redirects
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 0, "{r:?}");
        let err = r.error.expect("failures recorded");
        assert!(err.contains("ent_a1") && err.contains("ent_b1"), "{err}");
        assert_eq!(redirect_count(&conn), 0, "each failed group rolled back");
    }

    /// Punctuation/parentheses variants group (R2.7.2 live-census case).
    #[test]
    fn punctuation_normalized_matching_groups_parentheses() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(
            &conn,
            "e1",
            "Memory Architecture Intent 2026-09-01",
            "concept",
            "the intent memo",
        );
        seed(
            &conn,
            "e2",
            "Memory Architecture Intent (2026-09-01)",
            "concept",
            "the intent memo",
        );
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 1, "{r:?}");
        assert_eq!(merged_into(&conn, "e2").as_deref(), Some("e1"));
        // Punctuation must not glue words: "Intent (x)" ≠ "Intentx" but
        // equals "Intent x".
        assert_eq!(normalize_merge_key("Intent (x)"), "intent x");
        assert_ne!(normalize_merge_key("Intent (x)"), "intentx");
        // The doc example, now with non-ASCII letters KEPT (the key's own
        // documented behavior — final-review fix-before-merge: ASCII-only
        // filtering made "Zoë" collide with "Zo").
        assert_eq!(normalize_merge_key(" Café—Olga! "), "café olga");
        // "Zoë" and "Zo" are DIFFERENT names and must NOT share a key…
        assert_ne!(normalize_merge_key("Zoë"), normalize_merge_key("Zo"));
        // …while "Zoë" and "Zoë" (any casing/spacing variation) must.
        assert_eq!(normalize_merge_key("Zoë"), normalize_merge_key(" zoë "));
        assert_eq!(normalize_merge_key("Zoë"), "zoë");
    }

    /// Final-review fix-before-merge: NON-ASCII names must not collapse a
    /// duplicate group. Pre-fix, "Café" and "Cafe" hashed to the same
    /// merge key ("cafe") and the pass auto-merged them with agreeing
    /// summaries — a deterministic WRONG merge.
    #[test]
    fn non_ascii_names_do_not_merge() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Café", "concept", "corner café");
        seed(&conn, "e2", "Cafe", "concept", "corner café");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 0, "Café ≠ Cafe; {r:?}");
        assert_eq!(redirect_count(&conn), 0);

        // …while the SAME non-ASCII name still merges (the fix must not
        // over-correct into never-merge).
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e3", "Café", "concept", "corner café");
        seed(&conn, "e4", "Café", "concept", "corner café");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 1, "Café = Café must still merge; {r:?}");
    }

    /// Type conflict → queue, survivor must not silently win (R2.7.4).
    #[test]
    fn type_conflict_queues_group() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "person", "same");
        seed(&conn, "e2", "Adrian", "concept", "same");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 0);
        assert_eq!(r.queued_groups, 1);
        assert_eq!(r.groups[0].queued, Some(QueueReason::TypeConflict));
        assert_eq!(redirect_count(&conn), 0, "queued group writes nothing");
    }

    /// Both-empty summaries → queued, never auto-merged (r8-M3 matrix
    /// case: Adrian ×10 may be several people).
    #[test]
    fn both_empty_summaries_queue() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "");
        seed(&conn, "e2", "Adrian", "concept", "");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.queued_groups, 1, "{r:?}");
        assert_eq!(r.groups[0].queued, Some(QueueReason::SummaryMismatch));
        assert_eq!(redirect_count(&conn), 0);
    }

    /// Empty-vs-nonempty is NOT agreement either (r8-M3).
    #[test]
    fn empty_vs_nonempty_summary_queues() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "real summary");
        seed(&conn, "e2", "Adrian", "concept", "");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.queued_groups, 1);
        assert_eq!(redirect_count(&conn), 0);
    }

    /// Summaries that differ (normalized) → queue; normalized-EQUAL
    /// (punctuation variants) → auto-merge (R2.7.6).
    #[test]
    fn summary_gate_uses_normalized_equality() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "works on, pipelines");
        seed(&conn, "e2", "Adrian", "concept", "works on pipelines!");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 1, "{r:?}");
        seed(&conn, "e3", "Zed", "concept", "alpha");
        seed(&conn, "e4", "Zed", "concept", "beta");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.queued_groups, 1, "{r:?}");
    }

    /// The sweep excludes already-redirected rows (R2.7.1: else it
    /// re-detects the live loser by name every run).
    #[test]
    fn sweep_excludes_redirected_rows() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "s");
        seed(&conn, "e2", "Adrian", "concept", "s");
        redirect(&conn, "e2", "e1");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert!(r.groups.is_empty(), "{r:?}");
        assert_eq!(merged_into(&conn, "e2").as_deref(), Some("e1"));
    }

    /// Archived members (`deleted_at IS NOT NULL`) are never grouping
    /// candidates (R2.7.5 r21 — an archived cluster is never re-merged).
    #[test]
    fn archived_members_are_not_grouping_candidates() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "s");
        seed(&conn, "e2", "Adrian", "concept", "s");
        archive(&conn, "e2");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert!(r.groups.is_empty(), "{r:?}");
        // Same for a fully archived cluster.
        seed(&conn, "e3", "Zed", "concept", "t");
        archive(&conn, "e3");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert!(r.groups.is_empty(), "{r:?}");
    }

    /// Re-entrancy (r9-m1): a survivor can be demoted by a later import
    /// that brings a lower id.
    #[test]
    fn reentrant_survivor_demoted_by_later_import() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "bbb", "Adrian", "concept", "s");
        seed(&conn, "ccc", "Adrian", "concept", "s");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.groups[0].survivor, "bbb");
        // Later peer import keeps its source-host id, lower than the
        // standing survivor.
        seed(&conn, "aaa", "Adrian", "concept", "s");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 1, "{r:?}");
        assert_eq!(merged_into(&conn, "bbb").as_deref(), Some("aaa"));
        assert!(r
            .redirects_written
            .contains(&("bbb".to_string(), "aaa".to_string())));
    }

    /// 2-hop chain compresses to the final survivor (r2-m6): A→B exists,
    /// B merges into C ⇒ A's row rewrites to A→C.
    #[test]
    fn two_hop_chain_compresses_to_final_survivor() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "m_y", "Kurt", "concept", "s");
        seed(&conn, "m_x", "Kurt", "concept", "s");
        redirect(&conn, "m_a", "m_y");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 1, "{r:?}");
        assert_eq!(r.groups[0].survivor, "m_x");
        assert_eq!(merged_into(&conn, "m_y").as_deref(), Some("m_x"));
        assert_eq!(
            merged_into(&conn, "m_a").as_deref(),
            Some("m_x"),
            "A's row must rewrite to the FINAL survivor"
        );
        // The rewrite is part of the hand-reversal handle (R2.7.5 r21):
        // the report must record (id, OLD target, NEW target) so a row
        // can be restored by hand, not just deleted.
        assert_eq!(
            r.redirects_rewritten,
            vec![("m_a".to_string(), "m_y".to_string(), "m_x".to_string())],
            "{r:?}"
        );
    }

    /// A hand-crafted cycle is detected and reported, never looped (r2-m6).
    #[test]
    fn handcrafted_cycle_detected_and_reported() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        redirect(&conn, "cyc_a", "cyc_b");
        redirect(&conn, "cyc_b", "cyc_a");
        redirect(&conn, "d", "cyc_a");
        // Direct resolver: cycle, not a hang.
        assert_eq!(
            resolve_redirect_chain(&conn, "cyc_a").unwrap(),
            ChainResolution::Cycle("cyc_a".to_string())
        );
        // Through the sweep: an unrelated eligible group still merges and
        // the cycle surfaces in the report; rows on it are untouched.
        seed(&conn, "x1", "Zed", "concept", "s");
        seed(&conn, "x2", "Zed", "concept", "s");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 1, "{r:?}");
        assert!(!r.cycles.is_empty(), "cycle must be reported: {r:?}");
        assert_eq!(merged_into(&conn, "cyc_a").as_deref(), Some("cyc_b"));
        assert_eq!(merged_into(&conn, "cyc_b").as_deref(), Some("cyc_a"));
        assert_eq!(merged_into(&conn, "d").as_deref(), Some("cyc_a"));
    }

    /// Report arm writes nothing (Report + `--yes`, never auto).
    #[test]
    fn report_arm_writes_nothing() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "s");
        seed(&conn, "e2", "Adrian", "concept", "s");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, false);
        assert_eq!(r.merged_groups, 1, "would-merge count is reported");
        assert_eq!(redirect_count(&conn), 0);
        assert_eq!(merged_into(&conn, "e2"), None);
    }

    /// Edges between distinct group members are listed as resolved
    /// self-loops in the report (R2.7.5 — the prune handle).
    #[test]
    fn merge_report_lists_resolved_self_loops() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "s");
        seed(&conn, "e2", "Adrian", "concept", "s");
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('ed1', 'e1', 'e1', 'e2', 'related', 1)",
            [],
        )
        .unwrap();
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.merged_groups, 1, "{r:?}");
        assert_eq!(
            r.self_loops,
            vec![("ed1".to_string(), "e1".to_string(), "e2".to_string())]
        );
    }

    // ---- Precondition (r17-m3) + drift gate (R2.2.8 FINAL RULE) --------

    /// Missing `alias_remap_completed` marker: apply refused, report arm
    /// still computes; the marker is a MARKER, not a live-row predicate.
    #[test]
    fn missing_alias_remap_marker_refuses_apply() {
        let mut conn = open_in_memory().unwrap();
        assert_eq!(merge_precondition(&conn), MergePrecondition::RemapNotRun);
        seed(&conn, "e1", "Adrian", "concept", "s");
        seed(&conn, "e2", "Adrian", "concept", "s");
        // Report arm: groups computed, nothing written, not blocked.
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, false);
        assert_eq!(r.groups.len(), 1);
        assert_eq!(r.skipped_reason, None);
        // Apply arm: refused.
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.skipped_reason.as_deref(), Some("alias_remap_not_run"));
        assert_eq!(redirect_count(&conn), 0);
        assert_eq!(merge_precondition(&conn), MergePrecondition::RemapNotRun);
    }

    /// Unconfirmed drift blocks the destructive pass (FINAL RULE: merges
    /// are a destructive `--yes` action); a mismatched flag hash does not
    /// clear it; a matching Confirm or Waive does; and merge NEVER writes
    /// the watermark (heal is the sole writer).
    #[test]
    fn drift_gate_blocks_and_never_writes_the_watermark() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        let wm = r#"{"hash":"deadbeef","stamped_at":7}"#;
        set_watermark(&conn, wm);
        seed(&conn, "e1", "Adrian", "concept", "s");
        seed(&conn, "e2", "Adrian", "concept", "s");

        // Report arm: merged_groups is the PLANNED count.
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, false);
        assert_eq!(r.merged_groups, 1, "{r:?}");

        // No flag → refused; nothing merged, so nothing counted as merged.
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.skipped_reason.as_deref(), Some("unconfirmed_drift"));
        assert_eq!(r.drift.as_ref().unwrap().old_hash, "deadbeef");
        assert_eq!(redirect_count(&conn), 0);
        assert_eq!(r.merged_groups, 0, "a refused pass applied no merge: {r:?}");
        assert_eq!(r.groups.len(), 1, "the planned group is still listed");

        // Wrong hash → still unconfirmed.
        let r = merge_duplicates_pass(&mut conn, DriftFlag::Confirm("beef".into()), true);
        assert_eq!(r.skipped_reason.as_deref(), Some("unconfirmed_drift"));
        assert_eq!(redirect_count(&conn), 0);

        // Matching confirm → proceeds, watermark untouched.
        let r = merge_duplicates_pass(&mut conn, DriftFlag::Confirm("deadbeef".into()), true);
        assert_eq!(r.skipped_reason, None, "{r:?}");
        assert!(r.drift.as_ref().unwrap().confirmed);
        assert_eq!(redirect_count(&conn), 1);
        assert_eq!(r.merged_groups, 1, "the applied group is counted");
        let v: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = ?1",
                [WATERMARK_KEY],
                |x| x.get(0),
            )
            .unwrap();
        assert_eq!(v, wm, "merge must never write the watermark");

        // Waive path on a fresh brain: proceeds, watermark untouched.
        let mut conn2 = open_in_memory().unwrap();
        armed(&conn2);
        set_watermark(&conn2, wm);
        seed(&conn2, "e1", "Adrian", "concept", "s");
        seed(&conn2, "e2", "Adrian", "concept", "s");
        let r = merge_duplicates_pass(&mut conn2, DriftFlag::Waive("deadbeef".into()), true);
        assert_eq!(r.skipped_reason, None, "{r:?}");
        assert!(r.drift.as_ref().unwrap().waived);
        assert_eq!(redirect_count(&conn2), 1);
        let v: String = conn2
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = ?1",
                [WATERMARK_KEY],
                |x| x.get(0),
            )
            .unwrap();
        assert_eq!(v, wm);
    }

    /// First run (no watermark row at all) → no drift report, proceed
    /// (r7-m4); flags without a drift report are ignored with a note.
    #[test]
    fn no_watermark_no_drift_and_flags_ignored() {
        let mut conn = open_in_memory().unwrap();
        armed(&conn);
        seed(&conn, "e1", "Adrian", "concept", "s");
        seed(&conn, "e2", "Adrian", "concept", "s");
        let r = merge_duplicates_pass(&mut conn, DriftFlag::Confirm("zzz".into()), true);
        assert_eq!(r.drift, None, "{r:?}");
        assert_eq!(r.skipped_reason, None);
        assert_eq!(redirect_count(&conn), 1);
        // The initial-watermark fallback is consulted as the old hash.
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES (?1, 'deadbeef')",
            params![INITIAL_WATERMARK_KEY],
        )
        .unwrap();
        let r = merge_duplicates_pass(&mut conn, DriftFlag::None, false);
        assert_eq!(r.drift.as_ref().unwrap().old_hash, "deadbeef", "{r:?}");
    }
}
