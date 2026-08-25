//! One-off: approve all pending synthesize-mode proposals (memories/ folder).
//! Uses the real resolve_proposal commit path so outbox/events stay consistent.
use anyhow::{Context as _, Result};

use tauri_app_lib::db::commit::{resolve_proposal, ResolveOptions};
use tauri_app_lib::db::connection::AppDb;
use tauri_app_lib::db::proposals::{get_proposal_detail, ItemDecision, ItemDecisionKind};

fn main() -> Result<()> {
    let paths = tauri_app_lib::retrieval::resolve_brain_paths();
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)?;
    let conn = &mut db.0;

    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM curated_proposals WHERE status = 'pending'")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    println!("approving {} pending proposal(s)", ids.len());

    for pid in &ids {
        let detail = get_proposal_detail(conn, pid)
            .context("get detail")?
            .with_context(|| format!("proposal {pid} missing"))?;
        let decisions: Vec<ItemDecision> = detail
            .items
            .iter()
            .map(|i| ItemDecision {
                item_id: i.id.clone(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            })
            .collect();
        let result = resolve_proposal(
            conn,
            pid,
            &decisions,
            None,
            ResolveOptions { auto_approve: true },
        )?;
        println!(
            "approved {pid}: committed={} conflicts={} dropped_edges={} status={}",
            result.committed.len(),
            result.conflicts.len(),
            result.dropped_edges.len(),
            result.proposal_status,
        );
    }
    Ok(())
}
