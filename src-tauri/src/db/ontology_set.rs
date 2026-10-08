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
    /// The entity the write landed on — the caller's id resolved through
    /// any merge redirect to its survivor (R2.7.5 r13-MAJOR-1).
    pub entity_id: String,
    /// An opt-out row was written (`--mode off`).
    pub optout_written: bool,
    /// An opt-out row was deleted (`--mode strict`, r13-MAJOR-2).
    pub optout_deleted: bool,
    /// A strict entity manifest ROW was written (`--mode strict`).
    pub manifest_row_written: bool,
}

/// Resolve a caller-supplied entity id to the entity a write must land on:
/// through any merge redirect to its survivor (R2.7.5 r13-MAJOR-1 — every
/// id-keyed mutator resolves first, else a stale loser id writes a row no
/// gate or heal scan ever reads), and refuse an id that names no entity
/// (a typo would otherwise write an inert row and report success).
pub fn resolve_target_entity(conn: &Connection, entity_id: &str) -> Result<String> {
    let resolved = crate::db::entities::resolve_entity_id(conn, entity_id)?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_entities WHERE id = ?1)",
        params![resolved],
        |r| r.get(0),
    )?;
    if !exists {
        bail!("no entity `{entity_id}` in this brain — check the id; nothing was written");
    }
    Ok(resolved)
}

/// `--entity <id> --mode off`: write the deliberate `ct_entity_optouts` row
/// (rung 1(a)). Idempotent (upsert); returns after the write commits.
pub fn set_entity_optout(conn: &mut Connection, entity_id: &str) -> Result<EntityModeOutcome> {
    let tx = ImmediateTx::begin(conn)?;
    // Resolved INSIDE the write transaction so a concurrent merge cannot
    // redirect the id between the resolve and the write.
    let entity_id = resolve_target_entity(&tx, entity_id)?;
    tx.execute(
        "INSERT OR REPLACE INTO ct_entity_optouts (entity_id, reason, created_at)
         VALUES (?1, 'ct ontology set --mode off', strftime('%s','now'))",
        params![entity_id],
    )?;
    tx.commit()?;
    Ok(EntityModeOutcome {
        entity_id,
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
///
/// `fallback` (`--entity <id> --mode strict --fallback <type>`) overrides
/// the copied `fallback_node_type` on the row written here, validated
/// against the copied node types — in the SAME transaction, so a bad
/// fallback leaves nothing written.
pub fn set_entity_strict(
    conn: &mut Connection,
    entity_id: &str,
    fallback: Option<&str>,
) -> Result<EntityModeOutcome> {
    // The tier_fact read happens INSIDE the IMMEDIATE transaction (review
    // finding): the write lock is held from the read through the
    // INSERT OR REPLACE, so a concurrent engine rewrite of the tier row can
    // neither land between them nor be overwritten by a copy of the older
    // vocabulary. An early `bail!` drops the tx, which rolls back.
    let tx = ImmediateTx::begin(conn)?;
    let entity_id = resolve_target_entity(&tx, entity_id)?;
    let tier_json: Option<String> = tx
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
    if let Some(fallback) = fallback {
        entity_manifest["fallback_node_type"] =
            serde_json::json!(canonical_fallback(&entity_manifest, &entity_id, fallback)?);
    }

    // The opt-out lookup is cluster-closed (a merged-in member's opt-out
    // keeps applying to the survivor), so the reversal must clear it
    // across the whole redirect cluster or rung 1(a) keeps skipping.
    let cluster = crate::db::entities::cluster_ids(&tx, &entity_id)?;
    let optouts_deleted = tx.execute(
        &format!(
            "DELETE FROM ct_entity_optouts WHERE entity_id IN ({})",
            crate::db::entities::in_placeholders(&cluster)
        ),
        rusqlite::params_from_iter(cluster.iter()),
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
         VALUES (?1, 'strict', ?2, strftime('%s','now'))",
        params![entity_id, entity_manifest.to_string()],
    )?;
    tx.commit()?;
    Ok(EntityModeOutcome {
        entity_id,
        optout_written: false,
        optout_deleted: optouts_deleted > 0,
        manifest_row_written: true,
    })
}

/// `--entity <id> --mode <mode> [--fallback <type>]` dispatcher. A
/// fallback on an opted-out entity is meaningless (the gate never runs for
/// it) and is refused before anything is written.
pub fn set_entity_mode(
    conn: &mut Connection,
    entity_id: &str,
    mode: OntologyMode,
    fallback: Option<&str>,
) -> Result<EntityModeOutcome> {
    match (mode, fallback) {
        (OntologyMode::Off, Some(_)) => bail!(
            "--fallback has no effect on an opted-out entity (`--mode off` skips the \
             gate entirely) — nothing was written"
        ),
        (OntologyMode::Off, None) => set_entity_optout(conn, entity_id),
        (OntologyMode::Strict, fallback) => set_entity_strict(conn, entity_id, fallback),
    }
}

/// Validate `fallback` against `manifest`'s DECLARED node types and return
/// the manifest's canonical spelling (R2.4.4: a typo here would silently
/// become the landed type of every future mint).
fn canonical_fallback(
    manifest: &serde_json::Value,
    target: &str,
    fallback: &str,
) -> Result<String> {
    let parsed: crate::wiki_graph::WikiManifest = serde_json::from_value(manifest.clone())
        .map_err(|e| {
            anyhow::anyhow!("manifest_json for `{target}` is not a valid manifest: {e}")
        })?;
    let vocab = crate::db::entity_gate::NodeVocabulary::from_manifest(&parsed);
    match vocab.canonicalize(fallback) {
        Some(canonical) => Ok(canonical.to_string()),
        None => bail!(
            "`{fallback}` is not a node type declared by `{target}`'s manifest — \
             declare it in the manifest first, then set it as the fallback"
        ),
    }
}

/// Read `target`'s manifest row and compute the `--fallback` rewrite without
/// writing: `(bytes read, rewritten manifest_json)`. Shared by
/// [`validate_fallback`] (the pre-write check) and [`write_fallback`].
fn fallback_rewrite(conn: &Connection, target: &str, fallback: &str) -> Result<(String, String)> {
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
    let canonical = canonical_fallback(&manifest, target, fallback)?;
    manifest["fallback_node_type"] = serde_json::json!(canonical);
    Ok((json, manifest.to_string()))
}

/// Check a `--fallback` write would succeed (target row exists, type
/// declared) WITHOUT writing — the CLI runs this before any other write so
/// a bad fallback never leaves a partial, unreported change behind.
pub fn validate_fallback(conn: &Connection, target: &str, fallback: &str) -> Result<()> {
    fallback_rewrite(conn, target, fallback).map(|_| ())
}

/// `--fallback <type>`: write `fallback_node_type` into the target
/// manifest's `manifest_json` (target defaults to `tier_fact`). The target
/// row must already exist — this flag declares the §2.4.4 key explicitly and
/// never fabricates a manifest row.
pub fn write_fallback(conn: &Connection, target: &str, fallback: &str) -> Result<()> {
    let (json, manifest) = fallback_rewrite(conn, target, fallback)?;
    // Compare-and-swap on the bytes we READ (review finding, same rule as
    // `ensure_manifest_vocabulary`): a concurrent engine rewrite between the
    // read and this UPDATE must not be overwritten with an edited copy of
    // the OLDER manifest. Zero rows = the row moved under us — refuse loudly
    // so the operator re-runs against the new manifest.
    let changed = conn.execute(
        "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1, updated_at = strftime('%s','now')
         WHERE entity_id = ?2 AND manifest_json = ?3",
        params![manifest, target, json],
    )?;
    if changed == 0 {
        bail!(
            "`{target}`'s manifest changed while `--fallback` was being applied — \
             nothing was written; re-run the command"
        );
    }
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
        let conn = crate::db::connection::open_in_memory().unwrap();
        for id in ["ent_x", "ent_lose"] {
            conn.execute(
                "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                 VALUES (?1, ?1, 'concept', '', 1, 1)",
                params![id],
            )
            .unwrap();
        }
        conn
    }

    fn seed_tier_fact(conn: &Connection) -> serde_json::Value {
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
        manifest
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
        let manifest = seed_tier_fact(&conn);

        set_entity_mode(&mut conn, "ent_x", OntologyMode::Off, None).unwrap();
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::OptOut);

        let out = set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict, None).unwrap();
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

        // Review finding: `optout_deleted` reports what the DELETE did — a
        // repeat strict call with no opt-out row left must say false.
        let again = set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict, None).unwrap();
        assert!(!again.optout_deleted && again.manifest_row_written);
    }

    #[test]
    fn strict_without_tier_fact_row_refuses() {
        let mut conn = memory_conn();
        let err = set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict, None).unwrap_err();
        assert!(err.to_string().contains("tier_fact"));
        // No partial write: no manifest row, no optout change.
        assert_eq!(manifest_row_count(&conn).unwrap(), 0);
    }

    /// R2.7.5 r13-MAJOR-1: a merged-away loser id resolves to its survivor
    /// before the write — the opt-out and the strict row land where the gate
    /// reads them, and the outcome names the entity actually written.
    #[test]
    fn entity_writes_resolve_a_merged_loser_to_its_survivor() {
        let mut conn = memory_conn();
        seed_tier_fact(&conn);
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent_lose', 'ent_x', 1)",
            [],
        )
        .unwrap();
        let out = set_entity_mode(&mut conn, "ent_lose", OntologyMode::Off, None).unwrap();
        assert_eq!(out.entity_id, "ent_x");
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::OptOut);

        let out = set_entity_mode(&mut conn, "ent_lose", OntologyMode::Strict, None).unwrap();
        assert_eq!(out.entity_id, "ent_x");
        assert!(out.optout_deleted);
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::Gate);
    }

    /// The strict reversal clears an opt-out set on ANY cluster member (the
    /// opt-out lookup is cluster-closed), or rung 1(a) keeps skipping.
    #[test]
    fn strict_reversal_clears_a_merged_members_optout() {
        let mut conn = memory_conn();
        seed_tier_fact(&conn);
        conn.execute_batch(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at) VALUES ('ent_lose', 'user', 1);
             INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES ('ent_lose', 'ent_x', 1);",
        )
        .unwrap();
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::OptOut);
        let out = set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict, None).unwrap();
        assert!(out.optout_deleted);
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::Gate);
    }

    #[test]
    fn unknown_entity_id_is_refused_without_writing() {
        let mut conn = memory_conn();
        seed_tier_fact(&conn);
        let err = set_entity_mode(&mut conn, "ent_typo", OntologyMode::Off, None).unwrap_err();
        assert!(err.to_string().contains("no entity"), "{err}");
        let optouts: i64 = conn
            .query_row("SELECT COUNT(*) FROM ct_entity_optouts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(optouts, 0);
    }

    /// `--entity X --mode off --fallback F` is refused up front; a strict
    /// write with an undeclared fallback writes NOTHING (one transaction).
    #[test]
    fn entity_fallback_errors_leave_no_partial_write() {
        let mut conn = memory_conn();
        seed_tier_fact(&conn);
        let err =
            set_entity_mode(&mut conn, "ent_x", OntologyMode::Off, Some("document")).unwrap_err();
        assert!(err.to_string().contains("no effect"), "{err}");

        conn.execute(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at) VALUES ('ent_x', 'user', 1)",
            [],
        )
        .unwrap();
        let err =
            set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict, Some("projct")).unwrap_err();
        assert!(
            err.to_string().contains("not a node type declared"),
            "{err}"
        );
        assert_eq!(verdict(&conn, "ent_x"), ModeVerdict::OptOut, "opt-out kept");
        assert_eq!(manifest_row_count(&conn).unwrap(), 1, "only tier_fact");

        let out =
            set_entity_mode(&mut conn, "ent_x", OntologyMode::Strict, Some("Process")).unwrap();
        assert!(out.manifest_row_written);
        let row: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'ent_x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let row: serde_json::Value = serde_json::from_str(&row).unwrap();
        assert_eq!(row["fallback_node_type"], "process", "canonical spelling");
    }

    #[test]
    fn fallback_writes_into_existing_target_manifest_only() {
        let conn = memory_conn();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', '{\"node_types\":[{\"type\":\"document\"}],\"edge_types\":[]}', 1)",
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

        // R2.4.4: an undeclared fallback (the typo case) is refused and
        // leaves the manifest untouched.
        let before = row.clone();
        let err = write_fallback(&conn, TIER_FACT, "projct").unwrap_err();
        assert!(err.to_string().contains("not a node type declared"));
        let after: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after, "a refused fallback writes nothing");

        let err = write_fallback(&conn, "entity::absent", "document").unwrap_err();
        assert!(err.to_string().contains("no manifest row"));
    }
}
