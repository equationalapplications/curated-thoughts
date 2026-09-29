//! `ct drift` — read-only drift report (issue #241).
//!
//! Reports what reconcile WOULD do on the next `ct ingest --yes`:
//! gone files, offline moves, excluded-dir deletes — plus ambiguous rows
//! as WARNINGS (ingest never clears them; failing on them would mean a
//! permanent nonzero exit after a "successful" repair).
//! Never writes: read-only connection, classify only.
//!
//! Exit contract: 0 clean (ambiguous is a warning, not a failure), 3 drift
//! pending, 4 empty walk, 5 INDETERMINATE — the walk skipped content
//! (pending links) or errored, so a 3 from this command cannot be trusted
//! as "deleted" (see `drift_cmd`).

use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;

/// Serializable EXACTLY as documented in the JSON contract:
/// `repointed` must emit `[{"from": .., "to": ..}]` objects, NOT
/// `Vec<(String, String)>` (which serializes as `[["a","b"]]`).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Repoint {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct DriftReport {
    pub empty_walk: bool,
    pub gone: Vec<String>,
    pub repointed: Vec<Repoint>,
    pub excluded_deletes: Vec<String>,
    pub ambiguous_warnings: Vec<String>,
}

/// Pure core: classify an already-built file list into a report + exit code.
/// Exit contract: 0 clean (ambiguous is a warning, not a failure), 3 drift
/// pending, 4 empty walk. Unit tests target THIS function (no brain config,
/// no HOME resolution); `drift_cmd` is a thin I/O wrapper around it.
pub fn drift_report(
    conn: &Connection,
    files: &[tauri_app_lib::walk_vault::WalkedFile],
    vault_root: &Path,
) -> anyhow::Result<(DriftReport, i32)> {
    let classified = tauri_app_lib::reconcile::classify_vault(conn, files, vault_root)?;

    if classified.empty_walk {
        let report = DriftReport {
            empty_walk: true,
            gone: vec![],
            repointed: vec![],
            excluded_deletes: vec![],
            ambiguous_warnings: vec![],
        };
        return Ok((report, 4));
    }

    let report = DriftReport {
        empty_walk: false,
        // Consume the classify partition directly — NO is_excluded
        // re-filter (there is no such helper, and `plan.deleted` holds
        // both categories).
        gone: classified.gone_deletes.clone(),
        excluded_deletes: classified.excluded_deletes.clone(),
        repointed: classified
            .plan
            .repointed
            .iter()
            .map(|(from, to)| Repoint {
                from: from.clone(),
                to: to.clone(),
            })
            .collect(),
        ambiguous_warnings: classified.plan.ambiguous.clone(),
    };
    let pending =
        !report.gone.is_empty() || !report.repointed.is_empty() || !report.excluded_deletes.is_empty();
    Ok((report, if pending { 3 } else { 0 }))
}

/// I/O wrapper: resolves brain + vault root, builds the walk list, prints.
///
/// Walk incompleteness is treated as INDETERMINATE, never as drift: when the
/// walk skipped content (pending-approval symlinks) or hit walker errors, a
/// "gone" classification could equally mean "deleted" or "skipped this
/// walk", so exiting 3 (and recommending `ct ingest --yes`, whose reconcile
/// would then DELETE the skipped rows) would be a wrong answer delivered
/// confidently. Exit 5 tells the operator the report is unreliable until the
/// skipped content is approved (ct trust) or the errors are resolved.
pub fn drift_cmd(json: bool) -> anyhow::Result<i32> {
    let brain = crate::write::resolve()?;
    let conn = crate::write::open_ro(&brain)?;
    // Resolve the configured vault root exactly as cmds.rs ingest does —
    // delegated to the SHARED helper (build_ingest_file_list), which runs
    // VaultConfig + canonicalize with the same context-wrapped errors as
    // ingest. Canonicalization is NOT optional — non-canonical roots break
    // relativize_to_vault matching and drift would report false gone/
    // excluded deletes (spec M4, review m1: no duplicate resolution here).
    let paths = tauri_app_lib::retrieval::resolve_brain_paths();
    // trust_links: false — drift reports what a plain ingest would see;
    // promoting pending links is a `ct trust` decision, not drift's.
    let (vault_root, files, surfacing) =
        crate::walk_list::build_ingest_file_list(&paths, false)?;

    // trust_links=false means nothing was auto-promoted, so a non-empty
    // `pending` list is content this walk SKIPPED awaiting `ct trust` —
    // combined with walker errors it makes any "gone"/"excluded-delete"
    // verdict indeterminate (deleted vs skipped-this-walk).
    let walk_incomplete = !surfacing.pending.is_empty() || !surfacing.errors.is_empty();

    let (report, code) = drift_report(&conn, &files, &vault_root)?;

    if walk_incomplete && code == 3 {
        for p in &surfacing.pending {
            eprintln!(
                "warning: pending link {} -> {} was skipped by this walk; its absence from the report is NOT verified",
                p.link, p.target
            );
        }
        for e in &surfacing.errors {
            eprintln!("warning: walk error made this report indeterminate: {e}");
        }
        eprintln!(
            "drift: walk was INCOMPLETE ({} skipped link(s), {} walker error(s)) — the report cannot distinguish deleted from skipped; approve links with `ct trust` / resolve errors, then re-run",
            surfacing.pending.len(),
            surfacing.errors.len()
        );
        return Ok(5);
    }

    if json {
        // ALWAYS the full DriftReport shape (review M2): scripts do
        // `jq '.gone | length'` — the literal `{"empty_walk": true}` stub
        // returned null exactly when the vault is unmounted.
        println!("{}", serde_json::to_string(&report)?);
    } else if report.empty_walk {
        // Spec :90-92 wording — drift made NO classification; ingest
        // or app startup WOULD purge .brain rows on this walk.
        eprintln!("drift: vault walk returned no files — vault missing or unmounted; no drift classified (ingest would purge .brain rows for this walk)");
    } else {
        for p in &report.gone {
            println!("drift: gone {p}");
        }
        for r in &report.repointed {
            println!("drift: moved {} -> {}", r.from, r.to);
        }
        for p in &report.excluded_deletes {
            println!("drift: excluded-delete {p}");
        }
        for p in &report.ambiguous_warnings {
            eprintln!("warning: ambiguous (left alone by repair): {p}");
        }
        if code == 3 {
            eprintln!("repair with: ct ingest --yes");
        }
    }
    Ok(code)
}
