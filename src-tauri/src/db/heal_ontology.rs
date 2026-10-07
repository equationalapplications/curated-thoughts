//! Ontology heal pass (plan Task 5, spec r21 §2.6 + §2.2.8 + the §2.3
//! heal side).
//!
//! Runs ONLY from `ct heal` (`cmds.rs::heal_run`) AFTER the existing
//! source-heal (`db::heal::heal_invalid_sources_conn`) — the GUI path
//! (`run_wiki_heal`) and the scheduler path (`heal_invalid_sources`) are
//! deliberately untouched (plan-p4-MAJOR-3).
//!
//! Order of operations per run:
//!   1. drift watermark compare (R2.2.8) — echo old+new hash to STDERR;
//!   2. degraded/tied config refusal — scoped to the ONTOLOGY section only
//!      (plan-p7-m3): source-heal above has already run;
//!   3. FINAL RULE drift gate — unconfirmed drift blocks ONLY the ontology
//!      destructive actions (retypes/remaps; plan-p4-MAJOR-1), waive ends
//!      the reporting state without retypes or watermark storage;
//!   4. `ensure_manifest_vocabulary` for every manifest row (AFTER
//!      `migrate_open_db`, BEFORE the census — plan-p7-m2);
//!   5. incidental-off census (R2.9.3);
//!   6. remap pass (R2.6.2 signed alias table, per-row IMMEDIATE retypes
//!      with an `alias_retype` origin row in the SAME transaction);
//!   7. `alias_remap_completed` marker + watermark storage — heal is the
//!      SOLE watermark writer (R2.2.8 writer rule).
//!
//! STDOUT CONTRACT: this module never prints to stdout — human-readable
//! drift/census/queue text goes to STDERR so `ct heal --yes | jq` sees
//! exactly one JSON object (built by `tools/src/cmds.rs::CtHealOutput`).
//!
//! FAULT HANDLING (plan-p8-m1): a census DB fault propagates out of the
//! remap scan (R2.3.2a "Any DB fault → propagate / stop heal") but is
//! CAUGHT by [`ontology_heal_pass`] into `error` so the single JSON object
//! is always printed and `heal_run` can still exit non-zero.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db::entity_gate::{
    resolve_node_gate_decision, write_origin_ledger_row, GateResolutionContext, ImmediateTx,
    NodeVocabulary, ALIAS_TABLE,
};
use crate::db::schema::OriginReason;

/// `llm_wiki_meta` key holding the authoritative drift watermark
/// (`{"hash": .., "stamped_at": ..}`); heal is its only writer.
pub const WATERMARK_KEY: &str = "ontology_config_watermark";
/// Best-effort initial stamp written at the first gate/heal RESOLUTION
/// (r13-MAJOR-3); consulted as the old hash when no authoritative row
/// exists yet.
pub const INITIAL_WATERMARK_KEY: &str = "initial_drift_watermark";
/// Set at the END of a successful `heal --yes` remap pass (spec §2.7.1);
/// `ct wiki merge-duplicates` refuses until it is present. Deleted by
/// `clear_vault_tables` (plan Task 0).
pub const ALIAS_REMAP_MARKER_KEY: &str = "alias_remap_completed";

/// The drift-clearance flag pair from the CLI (R2.2.8 CLI form).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DriftFlag {
    #[default]
    None,
    /// `--confirm-drift <old-hash>`: confirms the echoed pair, proceeds,
    /// stores the new watermark.
    Confirm(String),
    /// `--waive-drift <old-hash>`: acknowledges and proceeds WITHOUT
    /// retypes, remaps, or watermark storage.
    Waive(String),
}

/// R2.9.3 incidental-off census counts (report-only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CensusReport {
    /// Entity manifest rows with `mode='off'` — reported, never migrated.
    pub incidental_off_manifests: usize,
    /// The `tier_fact` row exists but is not strict (§2.3.1 SKIP + warning).
    pub unmarked_tier_fact: bool,
}

/// The drift section (R2.2.8): old hash + timestamp vs. the live hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriftReport {
    pub old_hash: String,
    pub old_stamped_at: i64,
    pub new_hash: String,
    pub confirmed: bool,
    pub waived: bool,
}

/// The ontology section of the heal output (spec R2.6.3: retypes are
/// "logged in the heal summary" — the flattened CLI output IS that
/// summary).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OntologyHealReport {
    pub drift: Option<DriftReport>,
    pub census: CensusReport,
    /// Alias retypes applied this run (dry run: that WOULD apply).
    pub retyped: usize,
    /// Drift rows queued for a Kurt ruling (no applicable alias,
    /// off-sourced, or ledger-surfaced).
    pub queued: usize,
    /// Report-only rows (unresolved source under the R2.3.2/R2.3.5 scope).
    pub report_only: usize,
    /// A census/ensure fault caught here so the JSON object still prints.
    pub error: Option<String>,
    /// `"unconfirmed_drift"` | `"drift_waived"` | `"degraded_config"`.
    pub skipped_reason: Option<String>,
}

/// Run the ontology heal pass. `apply == false` is the read-only
/// (non-`--yes`) census: everything computed, NOTHING written (the ensure
/// is computed in memory and reported as "ensure pending (read-only)",
/// no watermark stamp). Errors are caught into [`OntologyHealReport::error`].
pub fn ontology_heal_pass(
    conn: &mut Connection,
    flag: DriftFlag,
    apply: bool,
) -> OntologyHealReport {
    let mut report = OntologyHealReport::default();
    if let Err(e) = run(conn, flag, apply, &mut report) {
        report.error = Some(format!("{e:#}"));
    }
    report
}

fn run(
    conn: &mut Connection,
    flag: DriftFlag,
    apply: bool,
    report: &mut OntologyHealReport,
) -> Result<()> {
    if !table_exists(conn, "llm_wiki_meta")? || !table_exists(conn, "entity_type_origin")? {
        // Old-schema database on a read-only path (plan-p9-M3): report,
        // never fail — `ct heal --yes` is the path that migrates.
        eprintln!(
            "ontology heal: schema pending (read-only) — new-table census unavailable; \
             run `ct heal --yes` once to migrate"
        );
        return Ok(());
    }

    let policy = crate::config::ingest_policy_for_db(conn.path());
    let degraded = policy.ontology_degraded_state();
    let ties = crate::config::ontology_ties(&policy.tiers);
    let live_hash = crate::config::ontology_config_watermark_hash(
        &policy.tiers,
        policy.ontology_selection,
        policy.ontology_unparseable,
    );

    // 1. Drift compare (R2.2.8). First run (no row) → no report (r7-m4).
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
                "ontology drift: config hash changed since the last recorded state \
                 (old {old_hash} stamped_at {old_ts} vs live {live_hash}); \
                 confirm with `ct heal --yes --confirm-drift {old_hash}` or \
                 acknowledge with `ct heal --yes --waive-drift {old_hash}`"
            );
        }
    }

    // 2. Degraded/tied config — the DESTRUCTIVE ONTOLOGY actions are
    //    refused (plan-p7-m3); source-heal above already ran.
    if degraded.is_degraded() || !ties.is_empty() {
        eprintln!(
            "ontology heal: ingest config is degraded or tied — destructive ontology \
             actions REFUSED (the source heal above was not blocked); fix config.json \
             and re-run"
        );
        report.skipped_reason = Some("degraded_config".into());
        return Ok(());
    }

    // 3. FINAL RULE drift gate (R2.2.8): blocks retypes/remaps only.
    match (&report.drift, &flag) {
        (Some(_), DriftFlag::None) => {
            report.skipped_reason = Some("unconfirmed_drift".into());
            eprintln!(
                "ontology heal: unconfirmed drift report — ontology retypes/remaps \
                 skipped this run"
            );
            return Ok(());
        }
        (Some(d), DriftFlag::Confirm(h)) | (Some(d), DriftFlag::Waive(h)) => {
            if h != &d.old_hash {
                eprintln!(
                    "ontology heal: drift flag {h:?} does not match the echoed old hash \
                     {} — echoed pair NOT confirmed",
                    d.old_hash
                );
                report.skipped_reason = Some("unconfirmed_drift".into());
                return Ok(());
            }
            if let DriftFlag::Confirm(_) = flag {
                report.drift = report.drift.take().map(|mut d| {
                    d.confirmed = true;
                    d
                });
            } else {
                // Waive (plan-p9-M2): no retypes, remaps, or watermark
                // storage; the waive event lives in this output only.
                report.drift = report.drift.take().map(|mut d| {
                    d.waived = true;
                    d
                });
                report.skipped_reason = Some("drift_waived".into());
                eprintln!(
                    "ontology heal: drift waived — no retypes, remaps, or watermark \
                     storage this run"
                );
                return Ok(());
            }
        }
        (None, DriftFlag::Confirm(_)) | (None, DriftFlag::Waive(_)) => {
            // No-drift case (plan-p14-m3): flags ignored with a note.
            eprintln!(
                "ontology heal: no drift report fired this run — \
                 --confirm-drift/--waive-drift ignored"
            );
        }
        (None, DriftFlag::None) => {}
    }

    let ctx = GateResolutionContext {
        ingest: &policy.tiers,
        degraded: &degraded,
        schema: policy.ontology_selection,
        schema_unparseable: policy.ontology_unparseable,
        vault_root: policy.vault_root.as_deref(),
    };

    if !apply {
        // Read-only census: ensure computed IN MEMORY (R2.4.4 r12-m4), no
        // watermark stamp, no writes; counts are "would" counts.
        let (pending, foreign) = ensure_pending_readonly(conn)?;
        if pending > 0 || foreign > 0 {
            eprintln!(
                "ontology heal: ensure pending (read-only): {pending} manifest(s) would \
                 be extended/ensured, {foreign} foreign manifest(s) lack a preferred \
                 fallback"
            );
        }
        report.census = census(conn)?;
        let counts = remap_pass(conn, &ctx, true)?;
        report.retyped = counts.retyped;
        report.queued = counts.queued;
        report.report_only = counts.report_only;
        return Ok(());
    }

    // 4. Ensure BEFORE census (plan-p7-m2).
    match crate::db::entity_gate::ensure_all_manifest_vocabularies(conn) {
        Ok(s) => {
            if s.extended + s.fallbacks_set + s.foreign_no_preferred_fallback + s.malformed > 0 {
                eprintln!(
                    "ontology ensure: visited={} extended={} fallbacks_set={} \
                     foreign_no_preferred_fallback={} malformed={}",
                    s.visited,
                    s.extended,
                    s.fallbacks_set,
                    s.foreign_no_preferred_fallback,
                    s.malformed
                );
            }
        }
        Err(e) => {
            report.error = Some(format!("manifest ensure failed: {e:#}"));
            return Ok(());
        }
    }

    // 5. Incidental-off census (R2.9.3).
    report.census = census(conn)?;

    // 6. Remap pass.
    let counts = remap_pass(conn, &ctx, false)?;
    report.retyped = counts.retyped;
    report.queued = counts.queued;
    report.report_only = counts.report_only;

    // 7. Marker at the END of a successful remap pass (§2.7.1), then the
    //    watermark — heal is the sole watermark writer (R2.2.8). Waive
    //    returned above, so reaching here means confirmed-or-no-drift.
    let (_, now_ms) = crate::db::commit::now_timestamps();
    conn.execute(
        "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES (?1, ?2)",
        params![ALIAS_REMAP_MARKER_KEY, now_ms.to_string()],
    )?;
    let watermark = serde_json::json!({"hash": live_hash, "stamped_at": now_ms}).to_string();
    conn.execute(
        "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES (?1, ?2)",
        params![WATERMARK_KEY, watermark],
    )?;
    Ok(())
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RemapCounts {
    retyped: usize,
    queued: usize,
    report_only: usize,
}

/// The census/remap scan (R2.6.1 detect + R2.6.2 dispositions). `dry`
/// classifies without mutating. DB faults propagate (R2.3.2a) — the
/// caller catches them into `report.error`.
fn remap_pass(
    conn: &mut Connection,
    ctx: &GateResolutionContext<'_>,
    dry: bool,
) -> Result<RemapCounts> {
    let mut counts = RemapCounts::default();
    let map_has_off = ctx
        .ingest
        .folder_ontology
        .values()
        .any(|m| *m == crate::config::OntologyMode::Off);

    // Live entities only; already-redirected losers are excluded so the
    // census does not re-surface a merged-away row (§2.7.1).
    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.entity_type FROM curated_entities e
             WHERE e.deleted_at IS NULL
               AND e.entity_type IS NOT NULL
               AND e.entity_type != ''
               AND NOT EXISTS (
                   SELECT 1 FROM entity_redirects r WHERE r.entity_id = e.id
               )",
        )?;
        let mut out = Vec::new();
        let mut rs = stmt.query([])?;
        while let Some(row) = rs.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        out
    };

    for (id, entity_type) in rows {
        // Conservative per-entity rollup (R2.3.2): resolve EVERY live
        // fact's source through the shared core.
        let facts: Vec<(String, Option<String>)> = {
            let mut stmt = conn.prepare(
                "SELECT id, source_ref FROM llm_wiki_entries
                 WHERE entity_id = ?1 AND deleted_at IS NULL",
            )?;
            let mut rs = stmt.query([&id])?;
            let mut out = Vec::new();
            while let Some(row) = rs.next()? {
                out.push((row.get(0)?, row.get(1)?));
            }
            out
        };
        let mut resolved_paths: Vec<String> = Vec::new();
        let mut any_unresolved = false;
        let mut any_off_source = false;
        for (entry_id, source_ref) in &facts {
            match crate::db::entities::resolve_source_core(conn, entry_id, source_ref.as_deref())? {
                crate::db::entities::SourceResolution::Resolved(paths) => {
                    for (path, _) in paths {
                        if let crate::config::OntologyLookup::Mode(
                            crate::config::OntologyMode::Off,
                        ) = lookup_path(ctx, &path)
                        {
                            any_off_source = true;
                        }
                        resolved_paths.push(path);
                    }
                }
                crate::db::entities::SourceResolution::HadEvidenceUnresolved => {
                    any_unresolved = true;
                }
                crate::db::entities::SourceResolution::NoEvidence => {}
            }
        }

        let ledger: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT reason, source_directory FROM entity_type_origin WHERE entity_id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let ledger_source_dir = ledger.as_ref().and_then(|(_, d)| d.clone());
        // An off-sourced mint (R2.3.5 ledger row) makes the entity
        // REPORT-or-QUEUE, never auto-retyped (R2.3.6).
        if ledger_source_dir.is_some() {
            any_off_source = true;
        }

        // R2.3.2/R2.3.5 scope rule (r21): unresolved source is report-only
        // when the map has an off (or degraded/dropped — unreachable here,
        // degraded refused the pass) entry OR the entity has a ledger row
        // carrying a source_directory. Empty map + no such row → the
        // ladder decides normally and remaps proceed.
        if any_unresolved && (map_has_off || ledger_source_dir.is_some()) {
            counts.report_only += 1;
            eprintln!(
                "ontology heal: {id} ({entity_type}) report-only — a source no longer \
                 resolves on an off-scoped brain (R2.3.2/R2.3.5)"
            );
            continue;
        }

        // Item 2a (r4-m4): an UNREADABLE entity manifest row is
        // report-or-hold for heal — the ladder would fall through to
        // rungs 2-4 and possibly Gate via tier_fact, but retyping an
        // entity whose own scoped manifest we cannot parse is exactly the
        // silent mutation the rule forbids. Report loudly and queue.
        if crate::wiki_graph::wiki_get_ontology(conn, &id).is_err() {
            counts.queued += 1;
            eprintln!(
                "ontology heal: {id} ({entity_type}) has an UNREADABLE entity manifest \
                 row — report-or-hold (r4-m4); queued, not retyped"
            );
            continue;
        }

        let mut paths = resolved_paths;
        if paths.is_empty() {
            if let Some(dir) = &ledger_source_dir {
                // R2.3.5 ledger fallback: live resolution empty → resolve
                // the mode from the recorded source directory.
                paths.push(dir.clone());
            } else {
                // Rung 3 for source-less entities (R2.3.4: climb from the
                // host default): an off host-default (or a Hold) means the
                // entity is not gated — nothing to heal.
                match lookup_path(ctx, "") {
                    crate::config::OntologyLookup::Mode(crate::config::OntologyMode::Off)
                    | crate::config::OntologyLookup::Hold => continue,
                    _ => {}
                }
            }
        }

        let decision = resolve_node_gate_decision(conn, &id, &paths, ctx.clone());
        if decision.verdict != crate::db::entity_gate::ModeVerdict::Gate {
            continue;
        }
        let Some(vocab) = decision.vocabulary else {
            continue;
        };
        if vocab.contains(&entity_type) {
            continue; // compliant (incl. legalized document/process)
        }

        // Ledger surfacing per the R2.4.6 table: degraded /
        // unlabeled_landing / gate_skipped rows are surfaced + queued;
        // alias_retype / queue_retype rows are reversibility records and
        // never surface (transparent below).
        if let Some((reason, _)) = &ledger {
            if matches!(
                OriginReason::parse(reason),
                Some(
                    OriginReason::Degraded
                        | OriginReason::UnlabeledLanding
                        | OriginReason::GateSkipped
                )
            ) {
                counts.queued += 1;
                eprintln!(
                    "ontology heal: {id} ({entity_type}) surfaced via origin ledger \
                     reason={reason} — queued for ruling"
                );
                continue;
            }
        }

        if any_off_source {
            counts.queued += 1;
            eprintln!(
                "ontology heal: {id} ({entity_type}) has an off-directory source — \
                 report-or-queue, never auto-retyped (R2.3.6)"
            );
            continue;
        }

        // Signed alias table (R2.6.2) with alias-target declaredness (r2-M1).
        let key = NodeVocabulary::key(&entity_type);
        let alias = ALIAS_TABLE
            .iter()
            .find(|(from, _)| NodeVocabulary::key(from) == key)
            .filter(|(_, to)| vocab.contains(to));
        match alias {
            Some((_, target)) => {
                if dry {
                    counts.retyped += 1;
                    continue;
                }
                // Per-row BEGIN IMMEDIATE (R2.6.1) with the origin row in
                // the SAME transaction (R2.4.6/R2.3.6).
                let tx = ImmediateTx::begin(conn)?;
                let (now_secs, _) = crate::db::commit::now_timestamps();
                let changed = tx.execute(
                    "UPDATE curated_entities
                     SET entity_type = ?1, updated_at = ?2
                     WHERE id = ?3 AND deleted_at IS NULL AND entity_type = ?4",
                    params![target, now_secs, id, entity_type],
                )?;
                if changed > 0 {
                    write_origin_ledger_row(
                        &tx,
                        &id,
                        Some(entity_type.as_str()),
                        OriginReason::AliasRetype,
                        None,
                    )?;
                    counts.retyped += 1;
                }
                tx.commit()?;
            }
            None => {
                counts.queued += 1;
                eprintln!(
                    "ontology heal: {id} drifted type '{entity_type}' has no applicable \
                     signed alias — queued for a Kurt ruling (R2.6.3)"
                );
            }
        }
    }
    Ok(counts)
}

fn lookup_path(ctx: &GateResolutionContext<'_>, path: &str) -> crate::config::OntologyLookup {
    ctx.ingest.ontology_lookup(
        path,
        ctx.vault_root,
        ctx.degraded,
        ctx.schema,
        ctx.schema_unparseable,
    )
}

/// R2.9.3 census: incidental `mode='off'` manifest rows, plus the §2.3.1
/// unmarked/off `tier_fact` warning. Report-only — nothing is migrated.
fn census(conn: &Connection) -> Result<CensusReport> {
    let off: i64 = conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entity_manifests WHERE mode = 'off'",
        [],
        |r| r.get(0),
    )?;
    if off > 0 {
        eprintln!(
            "ontology heal census (R2.9.3): {off} entity manifest row(s) carry \
             mode='off' — reported, not migrated; only deliberate \
             ct_entity_optouts rows skip the gate"
        );
    }
    let unmarked = matches!(
        crate::wiki_graph::wiki_get_ontology(conn, "tier_fact"),
        Ok(o) if o.mode != "strict"
    );
    if unmarked {
        eprintln!(
            "ontology heal census: the tier_fact manifest row is absent or not \
             strict — the gate SKIPs (§2.3.1); only a deliberate opt-out differs"
        );
    }
    Ok(CensusReport {
        incidental_off_manifests: off as usize,
        unmarked_tier_fact: unmarked,
    })
}

/// Read-only "would the ensure act?" computation (R2.4.4 r12-m4) for the
/// non-`--yes` arm — reports "ensure pending (read-only)" instead of
/// writing. Returns `(would_write, foreign_no_preferred_fallback)`.
fn ensure_pending_readonly(conn: &Connection) -> Result<(usize, usize)> {
    let rows: Vec<String> = {
        let mut stmt = conn.prepare("SELECT manifest_json FROM llm_wiki_entity_manifests")?;
        let mut rs = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rs.next()? {
            out.push(row.get(0)?);
        }
        out
    };
    let mut would_write = 0;
    let mut foreign_no_choice = 0;
    for manifest_json in rows {
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&manifest_json) else {
            continue; // Malformed: the ensure leaves it alone.
        };
        let declared: Vec<String> = root
            .get("node_types")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let declared_lower: std::collections::HashSet<String> =
            declared.iter().map(|s| NodeVocabulary::key(s)).collect();
        let is_ea_subset = crate::db::entity_gate::EA_SEED_TYPES
            .iter()
            .all(|seed| declared_lower.contains(&NodeVocabulary::key(seed)));
        let has_fallback = root
            .get("fallback_node_type")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .is_some_and(|s| !s.is_empty());
        let choice = if declared_lower.contains("concept") {
            true
        } else {
            declared_lower.contains("project")
        };
        if is_ea_subset {
            let missing_doc = !declared_lower.contains("document");
            let missing_proc = !declared_lower.contains("process");
            if missing_doc || missing_proc || (!has_fallback && choice) {
                would_write += 1;
            }
        } else if !has_fallback {
            if choice {
                would_write += 1;
            } else {
                foreign_no_choice += 1;
            }
        }
    }
    Ok((would_write, foreign_no_choice))
}

/// The old watermark: the authoritative row when present, else the
/// best-effort initial stamp (r13-MAJOR-3) so gate-era config changes are
/// still visible to the first `heal --yes`. Value shapes: JSON
/// `{"hash":..,"stamped_at":..}` or a bare hash string.
pub(crate) fn read_watermark(conn: &Connection) -> Result<Option<(String, i64)>> {
    if let Some(v) = conn
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [WATERMARK_KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&v) {
            if let Some(h) = val.get("hash").and_then(|x| x.as_str()) {
                let ts = val.get("stamped_at").and_then(|x| x.as_i64()).unwrap_or(0);
                return Ok(Some((h.to_string(), ts)));
            }
        }
        return Ok(Some((v, 0)));
    }
    Ok(conn
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [INITIAL_WATERMARK_KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(|h| (h, 0)))
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IngestConfig, OntologyMode};
    use crate::db::connection::open_in_memory;
    use crate::db::entity_gate::EA_SEED_TYPES;
    use serde_json::json;

    fn ctx<'a>(
        ingest: &'a IngestConfig,
        degraded: &'a crate::config::OntologyDegradedState,
    ) -> GateResolutionContext<'a> {
        GateResolutionContext {
            ingest,
            degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        }
    }

    /// Run the pass with a locally owned IngestConfig.
    fn run_with(
        conn: &mut Connection,
        ingest: &IngestConfig,
        flag: DriftFlag,
        apply: bool,
    ) -> OntologyHealReport {
        let degraded = crate::config::OntologyDegradedState::default();
        let c = ctx(ingest, &degraded);
        let mut report = OntologyHealReport::default();
        if let Err(e) = run_ctx(conn, &c, flag, apply, &mut report) {
            report.error = Some(format!("{e:#}"));
        }
        report
    }

    // `run` reads the policy from disk; tests need to inject the ingest
    // config, so factor the post-policy body.
    fn run_ctx(
        conn: &mut Connection,
        ctx: &GateResolutionContext<'_>,
        flag: DriftFlag,
        apply: bool,
        report: &mut OntologyHealReport,
    ) -> Result<()> {
        // Mirror `run` after the policy load, with the supplied ctx.
        // (Tests call this directly; production goes through `run`.)
        let ties = crate::config::ontology_ties(ctx.ingest);
        if ctx.degraded.is_degraded() || !ties.is_empty() {
            report.skipped_reason = Some("degraded_config".into());
            return Ok(());
        }
        if matches!((&report.drift, &flag), (Some(_), DriftFlag::None)) {
            report.skipped_reason = Some("unconfirmed_drift".into());
            return Ok(());
        }
        if !apply {
            let _ = ensure_pending_readonly(conn)?;
            report.census = census(conn)?;
            let counts = remap_pass(conn, ctx, true)?;
            report.retyped = counts.retyped;
            report.queued = counts.queued;
            report.report_only = counts.report_only;
            return Ok(());
        }
        let _ = crate::db::entity_gate::ensure_all_manifest_vocabularies(conn);
        report.census = census(conn)?;
        let counts = remap_pass(conn, ctx, false)?;
        report.retyped = counts.retyped;
        report.queued = counts.queued;
        report.report_only = counts.report_only;
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES (?1, ?2)",
            params![ALIAS_REMAP_MARKER_KEY, "1"],
        )?;
        Ok(())
    }

    fn seed_tier_fact_manifest(conn: &Connection, extra: &[&str], fallback: Option<&str>) {
        let mut types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        // `role` is already an EA slug; `extra` adds more (e.g. concept).
        for e in extra {
            types.push(json!({"type": e}));
        }
        let mut m = json!({"node_types": types, "edge_types": []});
        if let Some(f) = fallback {
            m["fallback_node_type"] = json!(f);
        }
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', ?1, 1)",
            params![m.to_string()],
        )
        .unwrap();
    }

    fn seed_entity(conn: &Connection, id: &str, entity_type: &str) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, '', NULL, 1, 1, NULL)",
            params![id, id, entity_type],
        )
        .unwrap();
    }

    fn seed_fact(conn: &Connection, entry_id: &str, entity_id: &str, source_ref: Option<&str>) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_ref, created_at, updated_at, deleted_at
             ) VALUES (?1, ?2, 't', 'b', '[]', 'inferred', 'user', ?3, 1, 1, NULL)",
            params![entry_id, entity_id, source_ref],
        )
        .unwrap();
    }

    fn entity_type(conn: &Connection, id: &str) -> String {
        conn.query_row(
            "SELECT entity_type FROM curated_entities WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn meta(conn: &Connection, key: &str) -> Option<String> {
        conn.query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
    }

    fn ledger_reason(conn: &Connection, id: &str) -> Option<(String, Option<String>)> {
        conn.query_row(
            "SELECT reason, original_type FROM entity_type_origin WHERE entity_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap()
    }

    /// Pre-wave-1 `agent` row (no ledger row) → alias retype to `role`,
    /// with the origin row in the same transaction; re-run is a no-op.
    #[test]
    fn alias_retype_writes_ledger_and_is_idempotent() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        let ingest = IngestConfig::default();
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 1, "{r:?}");
        assert_eq!(r.queued, 0, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "role");
        assert_eq!(
            ledger_reason(&conn, "e1"),
            Some(("alias_retype".into(), Some("agent".into())))
        );
        // Second run: compliant now, no new surfacing.
        let r2 = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r2.retyped, 0, "{r2:?}");
        assert_eq!(r2.queued, 0, "{r2:?}");
    }

    /// Drifted type with no signed alias (`character`) → queued, untouched.
    #[test]
    fn no_alias_drift_queues_for_ruling() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "character");
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 0);
        assert_eq!(r.queued, 1);
        assert_eq!(entity_type(&conn, "e1"), "character");
    }

    /// `document` is legalized by the ensure → compliant, zero queued.
    #[test]
    fn legalized_document_is_compliant_after_ensure() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], None); // pre-wave-1: no document/fallback
        seed_entity(&conn, "e1", "document");
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.queued, 0, "{r:?}");
    }

    /// R2.3.2 scope (r21/r18-m2): stale inline evidence + EMPTY map → the
    /// ladder decides normally and the remap proceeds.
    #[test]
    fn empty_map_stale_hash_still_remaps() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        // Inline evidence JSON whose content_hash resolves to nothing.
        seed_fact(
            &conn,
            "f1",
            "e1",
            Some(r#"{"evidence":[{"content_hash":"deadbeef"}]}"#),
        );
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 1, "{r:?}");
        assert_eq!(r.report_only, 0, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "role");
    }

    /// R2.3.2 scope: same stale hash + an `off` entry in the map → zero
    /// retypes (report-only).
    #[test]
    fn off_entry_stale_hash_is_report_only() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        seed_fact(
            &conn,
            "f1",
            "e1",
            Some(r#"{"evidence":[{"content_hash":"deadbeef"}]}"#),
        );
        let mut ingest = IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".into(), OntologyMode::Off);
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.report_only, 1, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// R2.3.5: a ledger row carrying `source_directory` keeps the entity
    /// report-only even after the off entry is removed from the map.
    #[test]
    fn ledger_source_directory_survives_off_entry_removal() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        seed_fact(
            &conn,
            "f1",
            "e1",
            Some(r#"{"evidence":[{"content_hash":"deadbeef"}]}"#),
        );
        conn.execute(
            "INSERT INTO entity_type_origin (entity_id, original_type, reason, source_directory, recorded_at)
             VALUES ('e1', 'agent', 'gate_skipped', 'ops', 1)",
            [],
        )
        .unwrap();
        // Off entry REMOVED: empty map, but the ledger row remains.
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.report_only, 1, "{r:?}");
    }

    /// R2.3.6: an entity with an off-directory source is queue-only, never
    /// auto-retyped.
    #[test]
    fn off_sourced_entity_queues_not_retypes() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        // Grounded source under the off folder: document + chunk with a
        // matching content_hash.
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES ('ops/a.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        let doc_id: i64 = conn
            .query_row("SELECT id FROM documents", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, content_hash)
             VALUES (?1, 't', 0, 1, 1, 'abc123')",
            params![doc_id],
        )
        .unwrap();
        seed_fact(
            &conn,
            "f1",
            "e1",
            Some(r#"{"evidence":[{"content_hash":"abc123"}]}"#),
        );
        let mut ingest = IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".into(), OntologyMode::Off);
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.queued, 1, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// R2.4.6 surfacing: a `gate_skipped` ledger row on a drifted entity
    /// under GATE → surfaced + queued, not retyped.
    #[test]
    fn gate_skipped_ledger_row_surfaces_to_queue() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        conn.execute(
            "INSERT INTO entity_type_origin (entity_id, original_type, reason, source_directory, recorded_at)
             VALUES ('e1', 'agent', 'gate_skipped', NULL, 1)",
            [],
        )
        .unwrap();
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.queued, 1, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// Dry run classifies but never mutates.
    #[test]
    fn dry_run_does_not_mutate() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, false);
        assert_eq!(r.retyped, 1, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
        assert!(meta(&conn, ALIAS_REMAP_MARKER_KEY).is_none());
        assert!(ledger_reason(&conn, "e1").is_none());
    }

    /// First-ever run: no drift section, watermark + marker stored.
    #[test]
    fn first_run_stores_watermark_and_marker() {
        let mut conn = open_in_memory().unwrap();
        // The production `run` (policy from disk; in-memory conn → default
        // policy with no config file).
        let r = ontology_heal_pass(&mut conn, DriftFlag::None, true);
        assert!(r.drift.is_none(), "{r:?}");
        assert_eq!(r.error, None, "{r:?}");
        assert!(meta(&conn, ALIAS_REMAP_MARKER_KEY).is_some());
        assert!(meta(&conn, WATERMARK_KEY).is_some());
    }

    /// Unconfirmed drift: skipped, watermark untouched.
    #[test]
    fn unconfirmed_drift_skips_remap_and_keeps_watermark() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_meta (key, value) VALUES ('ontology_config_watermark', ?1)",
            params![r#"{"hash":"deadbeef","stamped_at":7}"#],
        )
        .unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        let r = ontology_heal_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(r.skipped_reason.as_deref(), Some("unconfirmed_drift"));
        let d = r.drift.as_ref().unwrap();
        assert_eq!(d.old_hash, "deadbeef");
        assert_eq!(d.old_stamped_at, 7);
        assert!(!d.old_hash.is_empty() && d.new_hash != "deadbeef");
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(
            meta(&conn, WATERMARK_KEY).as_deref(),
            Some(r#"{"hash":"deadbeef","stamped_at":7}"#),
            "watermark must be untouched"
        );
        assert_eq!(entity_type(&conn, "e1"), "agent");
        assert!(meta(&conn, ALIAS_REMAP_MARKER_KEY).is_none());
    }

    /// Waive with the matching hash: exit-clean state, zero retypes,
    /// watermark unchanged, marker not set.
    #[test]
    fn waive_matching_hash_skips_everything() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_meta (key, value) VALUES ('ontology_config_watermark', ?1)",
            params![r#"{"hash":"deadbeef","stamped_at":7}"#],
        )
        .unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        let r = ontology_heal_pass(&mut conn, DriftFlag::Waive("deadbeef".into()), true);
        assert_eq!(r.skipped_reason.as_deref(), Some("drift_waived"));
        assert!(r.drift.as_ref().unwrap().waived);
        assert_eq!(r.retyped, 0);
        assert_eq!(entity_type(&conn, "e1"), "agent");
        assert!(meta(&conn, ALIAS_REMAP_MARKER_KEY).is_none());
        assert_eq!(
            meta(&conn, WATERMARK_KEY).as_deref(),
            Some(r#"{"hash":"deadbeef","stamped_at":7}"#)
        );
    }

    /// Mismatched hash on either flag → unconfirmed, nothing done.
    #[test]
    fn mismatched_confirm_hash_is_unconfirmed() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_meta (key, value) VALUES ('ontology_config_watermark', ?1)",
            params![r#"{"hash":"deadbeef","stamped_at":7}"#],
        )
        .unwrap();
        let r = ontology_heal_pass(&mut conn, DriftFlag::Confirm("wrong".into()), true);
        assert_eq!(r.skipped_reason.as_deref(), Some("unconfirmed_drift"));
        assert_eq!(
            meta(&conn, WATERMARK_KEY).as_deref(),
            Some(r#"{"hash":"deadbeef","stamped_at":7}"#)
        );
    }

    /// Confirmed drift proceeds and stores the NEW watermark.
    #[test]
    fn confirmed_drift_proceeds_and_stores_new_watermark() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_meta (key, value) VALUES ('ontology_config_watermark', ?1)",
            params![r#"{"hash":"deadbeef","stamped_at":7}"#],
        )
        .unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        let r = ontology_heal_pass(&mut conn, DriftFlag::Confirm("deadbeef".into()), true);
        assert!(r.drift.as_ref().unwrap().confirmed);
        assert_eq!(r.skipped_reason, None);
        assert_eq!(r.retyped, 1, "{r:?}");
        let stored = meta(&conn, WATERMARK_KEY).unwrap();
        let v: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_ne!(v["hash"].as_str().unwrap(), "deadbeef");
        assert!(meta(&conn, ALIAS_REMAP_MARKER_KEY).is_some());
    }

    /// The initial gate-time stamp serves as the old hash when no
    /// authoritative row exists (r13-MAJOR-3 rationale).
    #[test]
    fn initial_stamp_is_the_old_hash_before_the_first_heal() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_meta (key, value) VALUES ('initial_drift_watermark', 'oldgate')",
            [],
        )
        .unwrap();
        let r = ontology_heal_pass(&mut conn, DriftFlag::None, true);
        assert_eq!(
            r.skipped_reason.as_deref(),
            Some("unconfirmed_drift"),
            "gate-era config change must be visible to the first heal: {r:?}"
        );
        assert_eq!(r.drift.as_ref().unwrap().old_hash, "oldgate");
    }

    // ------------------------------------------------------------------
    // Fix round 1 — the brief's §6 enumerated heal-half scenarios
    // ------------------------------------------------------------------

    /// Grounded source helper: a document at `path` with a chunk whose
    /// content_hash the evidence cites.
    fn seed_grounded_doc(conn: &Connection, path: &str, hash: &str) {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES (?1, 'h', 'user_doc', 'indexed')",
            [path],
        )
        .unwrap();
        let doc_id: i64 = conn
            .query_row("SELECT id FROM documents WHERE path = ?1", [path], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, content_hash)
             VALUES (?1, 't', 0, 1, 1, ?2)",
            params![doc_id, hash],
        )
        .unwrap();
    }

    fn seed_evidence_fact(conn: &Connection, entity_id: &str, hash: &str) {
        seed_fact(
            conn,
            &format!("f_{entity_id}_{hash}"),
            entity_id,
            Some(&format!(r#"{{"evidence":[{{"content_hash":"{hash}"}}]}}"#)),
        );
    }

    /// §6 item 3: the remaining signed alias pairs — `component`→`service`
    /// and `software`→`document` (the target legalized by the ensure).
    #[test]
    fn remaining_alias_pairs_retype() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e_comp", "component");
        seed_entity(&conn, "e_sw", "software");
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 2, "{r:?}");
        assert_eq!(r.queued, 0, "{r:?}");
        assert_eq!(entity_type(&conn, "e_comp"), "service");
        assert_eq!(entity_type(&conn, "e_sw"), "document");
    }

    /// §6 item 3: `process` becomes legal via the ensure → compliant.
    #[test]
    fn process_is_legalized_by_the_ensure() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], None);
        seed_entity(&conn, "e_proc", "process");
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.queued, 0, "{r:?}");
        assert_eq!(entity_type(&conn, "e_proc"), "process");
    }

    /// §6 item 9 (vault-moved): an absolute source path that can no longer
    /// be placed inside the vault (root None) under an off prefix → Hold →
    /// zero retypes (unplaceable-with-root).
    #[test]
    fn vault_moved_off_prefix_zero_retypes() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        seed_grounded_doc(&conn, "/gone/vault/ops/a.md", "aaa111");
        seed_evidence_fact(&conn, "e1", "aaa111");
        let mut ingest = IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".into(), OntologyMode::Off);
        // vault_root None → the absolute path is Unplaceable → Hold.
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// §6 item 8: `folder_ontology` non-empty + effective root None +
    /// `heal --yes` → zero retypes (the unplaceable path cannot be matched,
    /// so no folder mode may decide — Hold via the map's off entry).
    #[test]
    fn nonempty_map_with_no_root_zero_retypes() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        seed_grounded_doc(&conn, "/elsewhere/x.md", "bbb222");
        seed_evidence_fact(&conn, "e1", "bbb222");
        let mut ingest = IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".into(), OntologyMode::Off);
        ingest
            .folder_ontology
            .insert("people".into(), OntologyMode::Strict);
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// §6 item 9 (SKIP + warning): mode=strict via rung 2 with NO strict
    /// tier_fact vocabulary row → the verdict is StrictNoVocab (SKIP for
    /// the retype) and the census warns `unmarked_tier_fact`.
    #[test]
    fn strict_rung2_without_vocabulary_skips_and_warns() {
        let mut conn = open_in_memory().unwrap();
        // NO tier_fact manifest row at all.
        seed_entity(&conn, "e1", "agent");
        seed_grounded_doc(&conn, "people/x.md", "ccc333");
        seed_evidence_fact(&conn, "e1", "ccc333");
        let mut ingest = IngestConfig::default();
        ingest
            .folder_ontology
            .insert("people".into(), OntologyMode::Strict);
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.queued, 0, "{r:?}");
        assert!(
            r.census.unmarked_tier_fact,
            "census must warn on the unmarked tier_fact row: {:?}",
            r.census
        );
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// §6 item 9 (same-bytes invalidation): the live hash the drift compare
    /// consumes is the canonical, insertion-order-independent hash built by
    /// `ontology_config_watermark_hash` — same map in two insertion orders
    /// hashes identically (no false drift), while a SAME-LENGTH key edit
    /// still changes it (no stale-policy miss). (Order-independence is
    /// pinned directly in config/mod.rs
    /// `watermark_hash_is_insertion_order_independent` and the raw-marker /
    /// tie-degraded rows; this pins the heal-side consumption.)
    #[test]
    fn live_hash_order_independent_and_same_length_edits_invalidate() {
        let build = |ops_first: bool| {
            let mut ingest = IngestConfig::default();
            if ops_first {
                ingest
                    .folder_ontology
                    .insert("ops".into(), OntologyMode::Off);
                ingest
                    .folder_ontology
                    .insert("people".into(), OntologyMode::Strict);
            } else {
                ingest
                    .folder_ontology
                    .insert("people".into(), OntologyMode::Strict);
                ingest
                    .folder_ontology
                    .insert("ops".into(), OntologyMode::Off);
            }
            crate::config::ontology_config_watermark_hash(&ingest, None, false)
        };
        assert_eq!(
            build(true),
            build(false),
            "insertion order must not change the live hash"
        );
        // Same-length edit (`aaa` → `aab`) must change the hash — a
        // bytes-keyed cache or length-based stamp would miss it.
        let mut edited = IngestConfig::default();
        edited
            .folder_ontology
            .insert("aab".into(), OntologyMode::Off);
        edited
            .folder_ontology
            .insert("people".into(), OntologyMode::Strict);
        assert_ne!(
            build(true),
            crate::config::ontology_config_watermark_hash(&edited, None, false),
            "a same-length key edit must invalidate the live hash"
        );
    }

    /// §6 item 10: the V20 empty-evidence sentinel
    /// (`{"proposal_id":null,"evidence":[]}`) is claimed-but-unresolved →
    /// report-only under an off-scoped map; the auto-retype count stays 0.
    #[test]
    fn v20_empty_evidence_sentinel_is_report_only() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        seed_fact(
            &conn,
            "f1",
            "e1",
            Some(r#"{"proposal_id":null,"evidence":[]}"#),
        );
        let mut ingest = IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".into(), OntologyMode::Off);
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.report_only, 1, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// §6 item 10 / R2.3.2a row: a PLAIN PATH ref (`documents/notes.md`) is
    /// `HadEvidenceUnresolved`, never a silent climb → report-only under an
    /// off-scoped map, zero retypes. (The resolver-row halves — plain path,
    /// truncated JSON, empty evidence, absent ref — are pinned directly in
    /// `db::entities` `resolve_source_core` tests.)
    #[test]
    fn plain_path_ref_is_report_only() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        seed_fact(&conn, "f1", "e1", Some("documents/notes.md"));
        let mut ingest = IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".into(), OntologyMode::Off);
        let r = run_with(&mut conn, &ingest, DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.report_only, 1, "{r:?}");
    }

    /// R2.3.2a fault row, heal side: a DB fault mid-census propagates out
    /// of the scan and lands in `error` (plan-p8-m1) — never "no source".
    #[test]
    fn census_db_fault_is_captured_into_error() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        conn.execute("DROP TABLE entity_type_origin", []).unwrap();
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert!(r.error.is_some(), "fault must surface in error: {r:?}");
        assert_eq!(r.retyped, 0, "{r:?}");
    }

    /// §6 item 10 (r10-M3): an engine-side manifest rewrite never removes a
    /// deliberate `ct_entity_optouts` row — the drifted entity keeps its
    /// opt-out across the rewrite and is never retyped.
    #[test]
    fn engine_manifest_rewrite_keeps_the_optout() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        conn.execute(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at)
             VALUES ('e1', 'deliberate', 1)",
            [],
        )
        .unwrap();
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "opt-out must skip the gate (rung 1a): {r:?}");
        // Engine-style rewrite of the manifest row (UPDATE manifest_json).
        conn.execute(
            "UPDATE llm_wiki_entity_manifests
             SET manifest_json = '{\"node_types\":[{\"type\":\"role\"}],\"edge_types\":[]}',
                 updated_at = 2
             WHERE entity_id = 'tier_fact'",
            [],
        )
        .unwrap();
        let still: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ct_entity_optouts WHERE entity_id = 'e1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still, 1, "the opt-out row survives the rewrite");
        let r2 = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r2.retyped, 0, "{r2:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// §6 item 2a: an UNREADABLE entity manifest row (malformed
    /// manifest_json on the entity's own row) → report-or-hold: queued with
    /// a loud note, never retyped.
    #[test]
    fn unreadable_entity_manifest_row_is_report_or_hold() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('e1', 'strict', '{not json', 1)",
            [],
        )
        .unwrap();
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(r.retyped, 0, "{r:?}");
        assert_eq!(r.queued, 1, "{r:?}");
        assert_eq!(entity_type(&conn, "e1"), "agent");
    }

    /// §6 item 2a: an UNMARKED entity manifest row (mode not strict) climbs
    /// rungs 2-4 — the tier_fact vocabulary gates normally.
    #[test]
    fn unmarked_entity_manifest_row_climbs() {
        let mut conn = open_in_memory().unwrap();
        seed_tier_fact_manifest(&conn, &[], Some("concept"));
        seed_entity(&conn, "e1", "agent");
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('e1', 'emergent', '{}', 1)",
            [],
        )
        .unwrap();
        let r = run_with(&mut conn, &IngestConfig::default(), DriftFlag::None, true);
        assert_eq!(
            r.retyped, 1,
            "climb reaches the strict tier_fact rung: {r:?}"
        );
        assert_eq!(entity_type(&conn, "e1"), "role");
    }
}
