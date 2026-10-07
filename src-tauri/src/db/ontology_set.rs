//! `ct ontology set` DB-side writes (plan Task 8, spec r21 §2.11).
//!
//! The config-file half (`ingest.ontology_default`, `ingest.folder_ontology`)
//! lives in the CLI (`tools/src/cmds.rs`) because it edits the brain config
//! like `folder_tiers` does; this module owns everything that touches
//! brain.db:
//!
//! * bare `--mode off` on an entity → a `ct_entity_optouts` row (D8,
//!   rung 1(a)); allowed only because R2.9.2's split-clear landed in Task 0.
//! * `--entity <id> --mode strict` → reversal rule r13-MAJOR-2: the
//!   `ct_entity_optouts` row is DELETED in the same transaction as the
//!   strict manifest-ROW write (else rung 1(a) keeps returning SKIP and the
//!   entity stays ungated forever). The row's `node_types` +
//!   `fallback_node_type` are copied VERBATIM from the RESOLVED `tier_fact`
//!   manifest (r13-m4: never the raw EA seed — the live row carries
//!   `concept`, and copying the 17-type seed would make `concept` drift).
//! * `--fallback <type>` → writes `fallback_node_type` into the target
//!   manifest's `manifest_json` (tier/manifest-level; target defaults to
//!   `tier_fact`). Declares the §2.4.4 key explicitly; the ensure does not
//!   run on this write path.
//!
//! STDOUT CONTRACT: no printing here — the CLI reports via its own JSON
//! object; notes go to STDERR from the caller.

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::config::OntologyMode;
use crate::db::entity_gate::ImmediateTx;

/// The manifest row every entity-scoped strict write resolves from.
pub const TIER_FACT: &str = "tier_fact";

/// Outcome of `--entity <id> --mode <mode>` (spec §2.11 reversal rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityModeOutcome {
    /// An opt-out row was written (`--mode off`).
    pub optout_written: bool,
    /// An opt-out row was deleted (`--mode strict`, r13-MAJOR-2).
    pub optout_deleted: bool,
    /// A strict entity manifest ROW was written (`--mode strict`).
    pub manifest_row_written: bool,
}

/// `--entity <id> --mode off`: write the deliberate `ct_entity_optouts` row
/// (rung 1(a)). Idempotent (upsert); returns after the write commits.
pub fn set_entity_optout(conn: &mut Connection, entity_id: &str) -> Result<EntityModeOutcome> {
    let tx = ImmediateTx::begin(conn)?;
    tx.execute(
        "INSERT OR REPLACE INTO ct_entity_optouts (entity_id, reason, created_at)
         VALUES (?1, 'ct ontology set --mode off', strftime('%s','now'))",
        params![entity_id],
    )?;
    tx.commit()?;
    Ok(EntityModeOutcome {
        optout_written: true,
        optout_deleted: false,
        manifest_row_written: false,
    })
}

/// `--entity <id> --mode strict` (r13-MAJOR-2): ONE transaction deletes the
/// `ct_entity_optouts` row AND writes the strict manifest ROW whose
/// `node_types` + `fallback_node_type` are copied VERBATIM from the
/// resolved `tier_fact` manifest (r13-m4). `edge_types` rides along verbatim
/// too when the tier row carries them — an entity-scoped strict row with a
/// synthesized empty edge list would hand the edge gate a vocabulary that
/// purges every edge the tier manifest declares.
pub fn set_entity_strict(conn: &mut Connection, entity_id: &str) -> Result<EntityModeOutcome> {
    let tier_json: Option<String> = conn
        .query_row(
            "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = ?1",
            params![TIER_FACT],
            |r| r.get(0),
        )
        .optional()?;
    let Some(tier_json) = tier_json else {
        bail!(
            "no `{TIER_FACT}` manifest row to resolve the entity vocabulary from — \
             run `ct heal --yes` once so the ensure creates it (§2.4.4), then retry"
        );
    };
    let tier: serde_json::Value = serde_json::from_str(&tier_json)
        .map_err(|e| anyhow::anyhow!("`{TIER_FACT}` manifest_json is not valid JSON: {e}"))?;
    let node_types = tier
        .get("node_types")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("`{TIER_FACT}` manifest_json carries no node_types"))?;
    let mut entity_manifest = serde_json::json!({ "node_types": node_types });
    if let Some(edge_types) = tier.get("edge_types") {
        entity_manifest["edge_types"] = edge_types.clone();
    }
    if let Some(fallback) = tier.get("fallback_node_type") {
        entity_manifest["fallback_node_type"] = fallback.clone();
    }

    let tx = ImmediateTx::begin(conn)?;
    tx.execute(
        "DELETE FROM ct_entity_optouts WHERE entity_id = ?1",
        params![entity_id],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
         VALUES (?1, 'strict', ?2, strftime('%s','now'))",
        params![entity_id, entity_manifest.to_string()],
    )?;
    tx.commit()?;
    Ok(EntityModeOutcome {
        optout_written: false,
        optout_deleted: true,
        manifest_row_written: true,
    })
}

/// `--entity <id> --mode <mode>` dispatcher.
pub fn set_entity_mode(
    conn: &mut Connection,
    entity_id: &str,
    mode: OntologyMode,
) -> Result<EntityModeOutcome> {
    match mode {
        OntologyMode::Off => set_entity_optout(conn, entity_id),
        OntologyMode::Strict => set_entity_strict(conn, entity_id),
    }
}

/// `--fallback <type>`: write `fallback_node_type` into the target
/// manifest's `manifest_json` (target defaults to `tier_fact`). The target
/// row must already exist — this flag declares the §2.4.4 key explicitly and
/// never fabricates a manifest row.
pub fn write_fallback(conn: &Connection, target: &str, fallback: &str) -> Result<()> {
    let json: Option<String> = conn
        .query_row(
            "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = ?1",
            params![target],
            |r| r.get(0),
        )
        .optional()?;
    let Some(json) = json else {
        bail!(
            "no manifest row for `{target}` — `--fallback` writes into an EXISTING \
             manifest's manifest_json; run `ct heal --yes` once so the ensure \
             creates the row, then retry"
        );
    };
    let mut manifest: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| anyhow::anyhow!("manifest_json for `{target}` is not valid JSON: {e}"))?;
    manifest["fallback_node_type"] = serde_json::json!(fallback);
    conn.execute(
        "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1, updated_at = strftime('%s','now')
         WHERE entity_id = ?2",
        params![manifest.to_string(), target],
    )?;
    Ok(())
}

/// The §2.11 no-manifest warning input: rows in `llm_wiki_entity_manifests`.
pub fn manifest_row_count(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM llm_wiki_entity_manifests", [], |r| {
            r.get(0)
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IngestConfig, OntologyDegradedState};
    use crate::db::entity_gate::{resolve_node_gate_decision, GateResolutionContext, ModeVerdict};
    use serde_json::json;

    fn memory_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entity_manifests (
                 entity_id TEXT PRIMARY KEY,
                 mode TEXT NOT NULL DEFAULT 'off',
                 manifest_json TEXT NOT NULL DEFAULT '{}',
                 updated_at INTEGER NOT NULL);
             CREATE TABLE ct_entity_optouts (
                 entity_id TEXT PRIMARY KEY,
                 reason TEXT,
                 created_at INTEGER NOT NULL);",
        )
        .unwrap();
        conn
    }

    fn ctx<'a>(
        ingest: &'a IngestConfig,
        degraded: &'a OntologyDegradedState,
    ) -> GateResolutionContext<'a> {
        GateResolutionContext {
            ingest,
            degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        }
    }

    fn verdict(conn: &Connection, entity_id: &str) -> ModeVerdict {
        let ingest = IngestConfig::default();
        let degraded = OntologyDegradedState::default();
        resolve_node_gate_decision(conn, entity_id, &[], ctx(&ingest, &degraded)).verdict
    }

    /// Spec §2.11 matrix: `--entity <id> --mode off` → rung 1(a) SKIP;
    /// then `--entity <id> --mode strict` (r13-MAJOR-2 reversal) → the NEXT
    /// mint IS gated.
    #[test]
    fn off_then_strict_next_mint_is_gated() {
        let mut conn = memory_conn();
        let manifest = json!({
            "node_types": [
                {"type": "concept"}, {"type": "document"}, {"type": "process"}
            ],
            "edge_types": [],
            "fallback_node_type": "document"
        });
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', ?1, 1)",
            params![manifest.to_string()],
        )
        .unwrap();

        set_entity_mode(&mut conn, "ent_x", OntologyMode::Off).unwrap();
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::OptOut);

        let out = set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict).unwrap();
        assert!(out.optout_deleted && out.manifest_row_written);
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::Gate);

        // r13-m4: node_types + fallback copied VERBATIM from the resolved
        // tier_fact row (18-type live row incl. concept — not the EA seed).
        let row: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'ent_x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let row: serde_json::Value = serde_json::from_str(&row).unwrap();
        assert_eq!(row["node_types"], manifest["node_types"]);
        assert_eq!(row["fallback_node_type"], "document");
        // The optout row is gone from the SAME transaction.
        let optouts: i64 = conn
            .query_row("SELECT COUNT(*) FROM ct_entity_optouts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(optouts, 0);
    }

    #[test]
    fn strict_without_tier_fact_row_refuses() {
        let mut conn = memory_conn();
        let err = set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict).unwrap_err();
        assert!(err.to_string().contains("tier_fact"));
        // No partial write: no manifest row, no optout change.
        assert_eq!(manifest_row_count(&conn).unwrap(), 0);
    }

    #[test]
    fn fallback_writes_into_existing_target_manifest_only() {
        let conn = memory_conn();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', '{\"node_types\":[{\"type\":\"document\"}]}', 1)",
            [],
        )
        .unwrap();
        write_fallback(&conn, TIER_FACT, "document").unwrap();
        let row: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(row.contains("fallback_node_type"));

        let err = write_fallback(&conn, "entity::absent", "document").unwrap_err();
        assert!(err.to_string().contains("no manifest row"));
    }
}
