//! Ontology node-type gate primitives (Task 2, spec r21):
//!
//! * [`ImmediateTx`] — the only constructor for the gate's transaction; carries
//!   the IMMEDIATE guarantee at the type level so a bare `&Connection` cannot
//!   reach the insert helper.
//! * [`NodeVocabulary`] — the single casing/canonicalization owner for node
//!   types, mirroring [`crate::db::commit::EdgeVocabulary`].
//! * [`ensure_manifest_vocabulary`] — the idempotent ensure step that adds
//!   `document` and `process` to a seed-subset manifest and writes
//!   `fallback_node_type` (Task 2 owns the ensure; Task 3 only calls it).
//!
//! The SHARED source-resolution core ([`crate::db::entities::resolve_source_core`]
//! / [`crate::db::entities::SourceResolution`]) lives in `db/entities.rs` per
//! the File Structure table — both the write-time gate (Task 3) and the heal
//! census (Task 5) consume it from there.
//!
//! What this module does NOT do: it does NOT call the gate at insert sites.
//! Task 3 wires that. The shared insert helper exists in this module so
//! Task 3 can wire it; the helper signature and the
//! [`AdmitOutcome`] it returns are the contract Task 3 reads.

use crate::db::schema::OriginReason;
use crate::hasher::hash_bytes;
use crate::wiki_graph::WikiManifest;
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;

/// A transaction that is guaranteed to have been opened with
/// [`TransactionBehavior::Immediate`].
///
/// Derefs to [`rusqlite::Transaction`] (NOT `Connection`): the call sites in
/// `commit.rs` and `bundle_apply.rs` use `tx.rollback()` and pass `&tx` to
/// helpers that take `&rusqlite::Transaction`; a Connection deref cannot
/// compile there. `Transaction` derefs transitively to `Connection`, so
/// `&tx → &Connection` helpers still work.
pub struct ImmediateTx<'c> {
    inner: Transaction<'c>,
}

impl<'c> ImmediateTx<'c> {
    /// Open an IMMEDIATE transaction on `conn`. The only legal way to build an
    /// `ImmediateTx`; every code path that needs a gate call funnels here so
    /// the IMMEDIATE guarantee cannot be skipped.
    pub fn begin(conn: &'c mut Connection) -> Result<Self> {
        let inner = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Ok(Self { inner })
    }

    /// Consuming commit. The lifetime `'c` ends when the transaction
    /// commits; callers cannot reuse `&mut Connection` until the next begin.
    pub fn commit(self) -> Result<()> {
        Ok(self.inner.commit()?)
    }

    /// Consuming rollback.
    pub fn rollback(self) -> Result<()> {
        Ok(self.inner.rollback()?)
    }
}

impl<'c> std::ops::Deref for ImmediateTx<'c> {
    type Target = Transaction<'c>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'c> std::ops::DerefMut for ImmediateTx<'c> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// The 17 EA / SchemaSoftwareOrg seed slugs (spec §1.1).
///
/// The ensure's subset guard compares a manifest's `node_types` against this
/// set: every EA slug ⊆ the row's declared `node_types` is the pass condition.
/// Foreign manifests fail the guard and get the fallback-only declare-or-
/// report treatment (no `document`/`process` injection).
pub const EA_SEED_TYPES: &[&str] = &[
    "action",
    "creativework",
    "design_spec",
    "event",
    "handoff",
    "organization",
    "person",
    "place",
    "procedure",
    "product",
    "project",
    "reference_doc",
    "review",
    "role",
    "service",
    "session_recap",
    "software_application",
];

/// The signed alias table (spec §2.6.2 + R2.4.4 degrade ladder rung).
///
/// One place; adding a key means adding the value (and an `into_iter` arm).
/// `(declared_type_alias, declared_target)` — the target MUST itself be
/// declared in the resolved manifest for the alias to apply (spec r2-M1).
pub const ALIAS_TABLE: &[(&str, &str)] = &[
    ("agent", "role"),
    ("component", "service"),
    ("software", "document"),
];

/// `NodeVocabulary` — the single casing/canonicalization owner for node
/// types. Mirrors `EdgeVocabulary` (`db/commit.rs`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeVocabulary {
    /// trim+lowercase → the manifest's canonical spelling.
    by_key: std::collections::HashMap<String, String>,
    /// The manifest's declared fallback type, if any.
    fallback: Option<String>,
}

impl NodeVocabulary {
    /// Membership-rule key — one place, just like `EdgeVocabulary::key`.
    pub fn key(candidate: &str) -> String {
        candidate.trim().to_lowercase()
    }

    /// Build the vocabulary from a manifest.
    ///
    /// Empty-declared-set rule (spec §2.4.5): if a strict manifest declares
    /// zero node types, every write is held — there is nothing to admit, and
    /// `is_empty()` exposes that fact. The fallback is parsed even on an
    /// empty manifest so a manifest that names a fallback but no declared
    /// set can still degrade via §2.4.5.
    pub fn from_manifest(manifest: &WikiManifest) -> Self {
        let mut by_key: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for node in &manifest.node_types {
            let k = Self::key(&node.type_name);
            if k.is_empty() {
                continue;
            }
            by_key
                .entry(k)
                .or_insert_with(|| node.type_name.trim().to_string());
        }
        Self {
            by_key,
            fallback: manifest
                .fallback_node_type
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        }
    }

    /// The manifest's declared fallback, if any.
    pub fn fallback(&self) -> Option<&str> {
        self.fallback.as_deref()
    }

    /// The manifest's spelling of `candidate`, or `None` when undeclared.
    pub fn canonicalize(&self, candidate: &str) -> Option<&str> {
        self.by_key.get(&Self::key(candidate)).map(String::as_str)
    }

    /// Whether `candidate` names a declared node type (case/whitespace
    /// insensitive).
    pub fn contains(&self, candidate: &str) -> bool {
        self.by_key.contains_key(&Self::key(candidate))
    }

    /// True when the vocabulary admits no names. A strict manifest with an
    /// empty vocabulary is a §2.4.5 configuration error: mints are held, never
    /// silently admitted.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// The outcome the shared insert helper returns so Task 3 can write the
/// ledger row in the SAME transaction.
///
/// `AdmittedDeclared` / `Aliased` / `DegradedToFallback` are the three
/// "the helper inserted" outcomes — the caller writes a `degraded` or
/// `gate_skipped` origin row in the same transaction. `Skipped` means the
/// helper did NOT insert (the gate held it; SKIP-path callers like bundle
/// import land the literal `'concept'` and write a `gate_skipped` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitOutcome {
    /// The label was declared; the helper inserted verbatim.
    AdmittedDeclared { label: String, entity_id: String },
    /// The label was a known alias and its target is declared; the helper
    /// inserted under the target.
    Aliased {
        original_label: String,
        landed_as: String,
        entity_id: String,
    },
    /// The label was undeclared and degraded to the manifest's fallback.
    /// Caller writes a `degraded` ledger row.
    DegradedToFallback {
        original_label: String,
        landed_as: String,
        entity_id: String,
    },
    /// The gate held the proposal — strict + empty vocabulary / no fallback
    /// declared. The helper did NOT insert. Caller decides what to do
    /// (refuse error vs. fall back to a literal, which is bundle-import's job).
    Held { original_label: Option<String> },
    /// Gate SKIPPED (off / no manifest). The helper did NOT insert; the caller
    /// is responsible for landing the literal `'concept'` (bundle / GUI / OKF
    /// skip path) and writing a `gate_skipped` ledger row.
    Skipped { original_label: Option<String> },
}

/// The shape of the "should we gate?" decision, separate from the helper's
/// insert. Task 3 will inspect this on every write; for now the helper does
/// both at once because there is exactly one gate call site per insert
/// helper caller (Task 3 wires those).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Gated: vocabulary applies; helper inserts with admit ladder.
    Gate(NodeVocabulary),
    /// SKIP: off folder, off host, or no manifest — helper does NOT insert.
    /// The caller lands the literal `'concept'` and writes a `gate_skipped`
    /// ledger row.
    Skip,
    /// Held: strict + empty vocabulary or no fallback declared — helper does
    /// NOT insert. Caller writes a refusal.
    Held,
}

/// The single insert helper. Builds a connection from the `ImmediateTx`, runs
/// the gate (admit ladder), and inserts (or refuses). Returns enough
/// information for Task 3 to write the ledger row in the SAME transaction.
///
/// `caller_entity_type` is what the caller proposes (`None` for label-less
/// paths like bundle import).
///
/// `upsert`: okf_migration's existing semantics — update `name`/`summary`/
/// `updated_at` on conflict, but NEVER touch `entity_type` (spec r6-M3).
/// When false, the helper does a plain INSERT and reports the conflict as an
/// error (the migration caller wraps this for caller-supplied ids that must
/// not fail on a re-run).
#[allow(clippy::too_many_arguments)]
pub fn shared_insert_entity(
    tx: &ImmediateTx<'_>,
    caller_entity_id: Option<&str>,
    name: &str,
    caller_entity_type: Option<&str>,
    summary: &str,
    now_secs: i64,
    decision: GateDecision,
    upsert: bool,
) -> Result<AdmitOutcome> {
    let conn: &Connection = tx; // deref ImmediateTx → &Transaction → &Connection

    // Redirect check first (Task 7 APPLIES this at sites; Task 2 BUILDS it).
    // For now: if a caller-supplied id points to a redirect row, follow the
    // single hop. Cycles error out (no infinite loop).
    let resolved_id = match caller_entity_id {
        Some(id) => match single_hop_redirect(conn, id) {
            RedirectOutcome::Survivor(s) => s,
            RedirectOutcome::Cycle => {
                bail!("entity {id} is part of a redirect cycle")
            }
            RedirectOutcome::None => id.to_string(),
        },
        None => generate_mint_entity_id(),
    };

    match decision {
        GateDecision::Skip => Ok(AdmitOutcome::Skipped {
            original_label: caller_entity_type.map(str::to_string),
        }),
        GateDecision::Held => Ok(AdmitOutcome::Held {
            original_label: caller_entity_type.map(str::to_string),
        }),
        GateDecision::Gate(vocab) => {
            // Empty-vocab rule (§2.4.5): strict + empty vocabulary — held.
            if vocab.is_empty() {
                return Ok(AdmitOutcome::Held {
                    original_label: caller_entity_type.map(str::to_string),
                });
            }
            let proposed = caller_entity_type.map(str::trim).filter(|s| !s.is_empty());
            let admit = run_admit_ladder(&vocab, proposed);
            match admit {
                AdmitInternal::Declared(label) => {
                    do_insert(conn, &resolved_id, name, &label, summary, now_secs, upsert)?;
                    Ok(AdmitOutcome::AdmittedDeclared {
                        label,
                        entity_id: resolved_id,
                    })
                }
                AdmitInternal::Aliased {
                    original_label,
                    landed_as,
                } => {
                    do_insert(
                        conn,
                        &resolved_id,
                        name,
                        &landed_as,
                        summary,
                        now_secs,
                        upsert,
                    )?;
                    Ok(AdmitOutcome::Aliased {
                        original_label,
                        landed_as,
                        entity_id: resolved_id,
                    })
                }
                AdmitInternal::DegradedToFallback {
                    original_label,
                    landed_as,
                } => {
                    do_insert(
                        conn,
                        &resolved_id,
                        name,
                        &landed_as,
                        summary,
                        now_secs,
                        upsert,
                    )?;
                    Ok(AdmitOutcome::DegradedToFallback {
                        original_label,
                        landed_as,
                        entity_id: resolved_id,
                    })
                }
                AdmitInternal::Held => Ok(AdmitOutcome::Held {
                    original_label: caller_entity_type.map(str::to_string),
                }),
            }
        }
    }
}

fn do_insert(
    conn: &Connection,
    entity_id: &str,
    name: &str,
    entity_type: &str,
    summary: &str,
    now_secs: i64,
    upsert: bool,
) -> Result<()> {
    if upsert {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5, NULL)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               summary = excluded.summary,
               updated_at = excluded.updated_at",
            params![entity_id, name, entity_type, summary, now_secs],
        )?;
    } else {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5, NULL)",
            params![entity_id, name, entity_type, summary, now_secs],
        )?;
    }
    Ok(())
}

enum AdmitInternal {
    Declared(String),
    Aliased {
        original_label: String,
        landed_as: String,
    },
    DegradedToFallback {
        original_label: String,
        landed_as: String,
    },
    Held,
}

impl std::fmt::Debug for AdmitInternal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declared(s) => f.debug_tuple("Declared").field(s).finish(),
            Self::Aliased {
                original_label,
                landed_as,
            } => f
                .debug_struct("Aliased")
                .field("original_label", original_label)
                .field("landed_as", landed_as)
                .finish(),
            Self::DegradedToFallback {
                original_label,
                landed_as,
            } => f
                .debug_struct("DegradedToFallback")
                .field("original_label", original_label)
                .field("landed_as", landed_as)
                .finish(),
            Self::Held => write!(f, "Held"),
        }
    }
}

/// The degrade ladder (spec §2.4.4):
///   declared → alias (target declared) → fallback → held
///
/// Empty-declared-set rule (§2.4.5): a strict manifest with ZERO usable types
/// is a configuration error even when it has a fallback — there is nothing
/// to admit. Held.
fn run_admit_ladder(vocab: &NodeVocabulary, proposed: Option<&str>) -> AdmitInternal {
    if vocab.is_empty() {
        return AdmitInternal::Held;
    }
    let Some(label) = proposed else {
        // No label proposed — caller decides what to do. We return Held so
        // the helper does not silently invent a label.
        return AdmitInternal::Held;
    };
    if let Some(canonical) = vocab.canonicalize(label) {
        return AdmitInternal::Declared(canonical.to_string());
    }
    let label_key = NodeVocabulary::key(label);
    for (alias_from, alias_to) in ALIAS_TABLE {
        if NodeVocabulary::key(alias_from) == label_key {
            if vocab.contains(alias_to) {
                return AdmitInternal::Aliased {
                    original_label: label.to_string(),
                    landed_as: alias_to.to_string(),
                };
            }
            // alias target not declared → fall through to fallback
            break;
        }
    }
    if let Some(fallback) = vocab.fallback() {
        return AdmitInternal::DegradedToFallback {
            original_label: label.to_string(),
            landed_as: fallback.to_string(),
        };
    }
    AdmitInternal::Held
}

/// Mint a fresh LLM-shaped entity id (`ent_<24-hex>`), matching
/// `commit::generate_llm_id("ent_")`.
pub fn generate_mint_entity_id() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);
    format!("ent_{}", hex::encode(bytes))
}

/// Single-hop redirect resolver with cycle guard (spec R2.7.3 / plan-p2-M1).
///
/// Resolves one hop. A hand-crafted cycle (A→B, B→A) returns `Cycle` rather
/// than looping forever — the caller surfaces it to the user, never to
/// silent recursion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectOutcome {
    /// No redirect row exists for the input id; use it.
    None,
    /// The id resolves to a different entity id (its final survivor).
    Survivor(String),
    /// The redirect chain loops. Caller must surface; never silently pick a
    /// loser.
    Cycle,
}

pub fn single_hop_redirect(conn: &Connection, entity_id: &str) -> RedirectOutcome {
    let merged: Option<String> = conn
        .query_row(
            "SELECT merged_into FROM entity_redirects WHERE entity_id = ?1",
            [entity_id],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    match merged {
        None => RedirectOutcome::None,
        Some(s) if s == entity_id => RedirectOutcome::Cycle,
        Some(s) => {
            // Verify we did not just hit a 2-hop after a forged single hop:
            // if the survivor itself is a redirect's loser (i.e. a chain),
            // we still return that survivor — Task 7's readers do path-
            // compression at merge time, so a 2-hop row would not exist on
            // a healthy brain. Treat it as the one-hop survivor.
            RedirectOutcome::Survivor(s)
        }
    }
}

/// Idempotent ensure step (spec §2.4.4, plan-p10-m4).
///
/// Adds `document` and `process` to a manifest that already declares every EA
/// seed slug (subset guard), and writes `fallback_node_type` (preferring
/// `concept` if declared, otherwise `project`) under the SAME guard.
///
/// Foreign manifests (not a subset of `EA_SEED_TYPES`) get the fallback-only
/// declare-or-report treatment: do nothing on the manifest itself, but record
/// the loud diagnostic. The full `document`+`process` injection is reserved
/// for the EA seed family.
///
/// Best-effort at `AppDb::open_with_config` — log on failure, never fail the
/// open. Memoization key is `(entity_id, sha256(manifest_json))` and is
/// recorded ONLY after the write commits (r11-m4). In a contended read-only
/// open the ensure's write may fail; callers can fall back to in-memory
/// vocabulary computation.
pub fn ensure_manifest_vocabulary(conn: &Connection, entity_id: &str) -> Result<EnsureOutcome> {
    // 1. Read the existing row.
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT mode, manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = ?1",
            [entity_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((_mode, manifest_json)) = row else {
        // No row to ensure — the spec's "missing row" case is a normal
        // SKIP per §2.1.
        return Ok(EnsureOutcome::NoRow);
    };

    // 2. Memoization: skip if (entity_id, sha256(manifest_json)) is recorded.
    let manifest_hash = hash_bytes(manifest_json.as_bytes());
    if manifest_ensure_already_done(conn, entity_id, &manifest_hash)? {
        return Ok(EnsureOutcome::AlreadyEnsured);
    }

    // 3. Parse, classify, edit.
    let mut root: serde_json::Value = match serde_json::from_str(&manifest_json) {
        Ok(v) => v,
        Err(e) => {
            // Malformed manifest_json: never guess — leave alone.
            return Ok(EnsureOutcome::Malformed(format!("{e}")));
        }
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

    // Subset guard: every EA slug ⊆ declared? If yes, do the full work;
    // otherwise do only the fallback declare-or-report.
    let is_ea_subset = {
        let declared_lower: HashSet<String> =
            declared.iter().map(|s| NodeVocabulary::key(s)).collect();
        EA_SEED_TYPES
            .iter()
            .all(|seed| declared_lower.contains(&NodeVocabulary::key(seed)))
    };

    // Pick the fallback value (prefer `concept` if declared; else `project`).
    let declared_lower: HashSet<String> = declared.iter().map(|s| NodeVocabulary::key(s)).collect();
    let fallback_choice: Option<&'static str> = if declared_lower.contains("concept") {
        Some("concept")
    } else if declared_lower.contains("project") {
        Some("project")
    } else {
        None
    };

    let mut did_set_fallback = false;
    let mut did_extend = false;

    if is_ea_subset {
        // Add `document` + `process` entries if not already present.
        let node_types = root
            .get_mut("node_types")
            .and_then(|v| v.as_array_mut())
            .context("EA manifest missing node_types array")?;
        for (slug, desc) in [
            (
                "document",
                "Written artifacts: notes, docs, papers, specs, designs, articles.",
            ),
            (
                "process",
                "Repeatable procedures, protocols, recipes, skills, runbooks.",
            ),
        ] {
            let already = node_types
                .iter()
                .any(|v| v.get("type").and_then(|t| t.as_str()) == Some(slug));
            if !already {
                node_types.push(json!({"type": slug, "description": desc}));
                did_extend = true;
            }
        }
        // Set fallback_node_type if not already present AND we have a choice.
        let existing_fallback = root
            .get("fallback_node_type")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if existing_fallback.is_none() {
            if let Some(choice) = fallback_choice {
                root["fallback_node_type"] = json!(choice);
                did_set_fallback = true;
            }
        }
    } else {
        // Foreign manifest: declare-or-report only.
        let existing_fallback = root
            .get("fallback_node_type")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if existing_fallback.is_none() && fallback_choice.is_none() {
            return Ok(EnsureOutcome::ForeignNoPreferredFallback {
                entity_id: entity_id.to_string(),
            });
        }
        // Foreign manifest with a declared fallback — fine. With no declared
        // fallback but `project` declared — declare it. With nothing
        // declared — the loud signal above.
        if existing_fallback.is_none() {
            if let Some(choice) = fallback_choice {
                root["fallback_node_type"] = json!(choice);
                did_set_fallback = true;
            }
        }
    }

    // 4. Write the edited manifest_json back + record the memo ONLY if the
    //    write commits (r11-m4). The memo key is the POST-WRITE hash so the
    //    next call reads the new manifest_json, computes its hash, and finds
    //    the memo — short-circuiting to AlreadyEnsured.
    if did_set_fallback || did_extend {
        let new_json = serde_json::to_string(&root)?;
        let new_hash = hash_bytes(new_json.as_bytes());
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1 WHERE entity_id = ?2",
            params![new_json, entity_id],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO manifest_ensure_memo (entity_id, manifest_hash, recorded_at)
             VALUES (?1, ?2, ?3)",
            params![entity_id, new_hash, crate::db::commit::now_timestamps().0],
        )?;
        tx.commit()?;
        Ok(EnsureOutcome::Ensured {
            extended: did_extend,
            fallback_set: did_set_fallback,
        })
    } else {
        // Nothing to change — still record it so we don't reparse on every
        // resolution (r11-m4).
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO manifest_ensure_memo (entity_id, manifest_hash, recorded_at)
             VALUES (?1, ?2, ?3)",
            params![
                entity_id,
                manifest_hash,
                crate::db::commit::now_timestamps().0
            ],
        )?;
        tx.commit()?;
        Ok(EnsureOutcome::AlreadyComplete)
    }
}

/// What `ensure_manifest_vocabulary` did — surfaced so callers can log and
/// tests can pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// No manifest row for this entity id — §2.1 SKIP, normal.
    NoRow,
    /// The (entity_id, manifest_hash) memo recorded an earlier done.
    AlreadyEnsured,
    /// Manifest was malformed; left alone.
    Malformed(String),
    /// Manifest was already complete (no extensions, fallback already set).
    AlreadyComplete,
    /// The ensure extended / set fallback. `extended` = true if
    /// `document`/`process` were added; `fallback_set` = true if
    /// `fallback_node_type` was written.
    Ensured { extended: bool, fallback_set: bool },
    /// Foreign manifest without a preferred-fallback option declared; the
    /// ensure refused to inject and surfaced a §2.4.5 loud diagnostic.
    ForeignNoPreferredFallback { entity_id: String },
}

fn manifest_ensure_already_done(
    conn: &Connection,
    entity_id: &str,
    manifest_hash: &str,
) -> Result<bool> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM manifest_ensure_memo WHERE entity_id = ?1 AND manifest_hash = ?2",
            params![entity_id, manifest_hash],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(exists > 0)
}

/// Run `ensure_manifest_vocabulary` for every manifest row in the brain.
/// Best-effort (r8-m6): one row's failure does NOT stop the loop; the caller
/// receives the first error and the count of rows visited.
pub fn ensure_all_manifest_vocabularies(conn: &Connection) -> Result<EnsureSummary> {
    let mut stmt = conn.prepare("SELECT entity_id FROM llm_wiki_entity_manifests")?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut summary = EnsureSummary::default();
    let mut first_err: Option<anyhow::Error> = None;
    for entity_id in rows {
        match ensure_manifest_vocabulary(conn, &entity_id) {
            Ok(outcome) => {
                summary.visited += 1;
                match outcome {
                    EnsureOutcome::Ensured {
                        extended,
                        fallback_set,
                    } => {
                        if extended {
                            summary.extended += 1;
                        }
                        if fallback_set {
                            summary.fallbacks_set += 1;
                        }
                    }
                    EnsureOutcome::ForeignNoPreferredFallback { .. } => {
                        summary.foreign_no_preferred_fallback += 1;
                    }
                    EnsureOutcome::Malformed(_) => summary.malformed += 1,
                    EnsureOutcome::AlreadyEnsured
                    | EnsureOutcome::AlreadyComplete
                    | EnsureOutcome::NoRow => {}
                }
            }
            Err(e) => {
                summary.visited += 1;
                summary.errors += 1;
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if let Some(e) = first_err {
        Err(e)
    } else {
        Ok(summary)
    }
}

/// Summary of a full pass over the manifest table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EnsureSummary {
    pub visited: usize,
    pub extended: usize,
    pub fallbacks_set: usize,
    pub foreign_no_preferred_fallback: usize,
    pub malformed: usize,
    pub errors: usize,
}

/// `NodeVocabulary` end-to-end test (plan §1, ensure → `wiki_get_ontology` →
/// `NodeVocabulary` sees the fallback). And the canonicalize casing rule.
impl WikiManifest {
    /// Build the vocabulary from this manifest, with the same rule as the
    /// node-gate helper uses. Thin pass-through kept here for ergonomics; the
    /// real single owner is [`NodeVocabulary::from_manifest`].
    pub fn node_vocabulary(&self) -> NodeVocabulary {
        NodeVocabulary::from_manifest(self)
    }

    /// Lookup helper used by tests + the ensure probe: "does this manifest
    /// declare `slug` (case/whitespace insensitive)?"
    pub fn declares_node_type(&self, slug: &str) -> bool {
        self.node_vocabulary().contains(slug)
    }
}

/// Silent guard: callers using `tx` after `rollback()` accidentally would
/// crash at the next borrow. This empty method documents that the
/// `rollback()` variant on `ImmediateTx` is consuming — the caller cannot
/// continue to use the transaction after rolling back.
#[allow(dead_code)]
fn _rollback_is_consuming() -> Result<()> {
    let mut conn = Connection::open_in_memory().unwrap();
    let tx = ImmediateTx::begin(&mut conn).unwrap();
    let _ = tx.rollback();
    Err(anyhow!("after rollback"))
}

/// The §2.3 ladder's MODE verdict, separate from the vocabulary that
/// may or may not back it. The shared insert helper takes a [`GateDecision`]
/// (which couples mode + vocabulary) — `ModeVerdict` is the intermediate the
/// resolver builds first, so rung 1's node/edge verbs and rung 4's
/// tier_fact fallback can share the path resolution without entangling the
/// vocabulary lookup.
///
/// Spec §2.3:
///   * `OptOut` — rung 1a: a deliberate `ct_entity_optouts` row exists.
///     SKIP per §2.1. The spec is also explicit (r12-M1) that this lives
///     in the CT-owned table, NOT on the manifest row.
///   * `Gate` — rung 1b: the entity's own strict manifest row exists.
///     GATE; the vocabulary comes from THIS manifest.
///   * `Off` — rungs 2/3/4 explicitly resolve to off (an `off`
///     `folder_ontology` entry, an off ontology_default, or an
///     unmarked-off tier_fact row). SKIP.
///   * `StrictNoVocab` — rung 4: tier_fact is strict with no usable
///     vocabulary (no `node_types` OR no fallback_node_type).
///     HELD per §2.4.5 — config error.
///   * `Climb` — the rungs above resolved nothing; the caller should
///     resolve mode via the `folder_ontology` / `ontology_default` /
///     `tier_fact` ladder. Today the gate resolver threads the
///     IngestConfig + degraded state to do that climb here — `Climb` is
///     kept for callers that want to do it themselves (e.g. test fixtures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeVerdict {
    OptOut,
    Gate,
    Off,
    StrictNoVocab,
    Climb,
}

/// What `resolve_node_gate_decision` did — Task 3 wires this at the four
/// production insert sites. Pairs a `ModeVerdict` (the §2.3 mode) with the
/// NodeVocabulary the gate should run against (only meaningful for `Gate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeGateDecision {
    pub verdict: ModeVerdict,
    pub vocabulary: Option<NodeVocabulary>,
    /// The directory path that caused an off-mode resolution, when known
    /// (used by the off-sourced ledger writer to populate
    /// `entity_type_origin.source_directory`).
    pub source_directory: Option<String>,
}

impl NodeGateDecision {
    /// Convert to a [`GateDecision`] for the shared insert helper. An
    /// off/opt-out/climb verdict becomes `Skip`; an empty vocabulary
    /// becomes `Held`; a populated vocabulary becomes `Gate(vocab)`.
    pub fn into_gate_decision(self) -> GateDecision {
        match self.verdict {
            ModeVerdict::OptOut | ModeVerdict::Off | ModeVerdict::Climb => GateDecision::Skip,
            ModeVerdict::StrictNoVocab => GateDecision::Held,
            ModeVerdict::Gate => match self.vocabulary {
                Some(v) => GateDecision::Gate(v),
                None => GateDecision::Held,
            },
        }
    }
}

/// Resolve the §2.3 ladder for a NEW entity mint at one of the four
/// production insert sites (LLM synthesis / GUI / bundle / okf_migration).
///
/// `entity_id` is the id the helper will mint (None when caller-supplied).
/// `source_paths` are the candidate paths rung 2 walks (`folder_ontology`
/// resolution): for LLM synthesis these are the trigger document paths, for
/// bundle import the originating vault-relative path, for GUI and the
/// `ontology_default` board they are empty (start at rung 3), for
/// `okf_migration` the wiki-page path. The resolver walks ALL of them and
/// strict-wins across them per §2.3.3 (`Off` loses to a single strict match).
///
/// The §2.3 ladder's rung-2/3 inputs, packaged to keep
/// [`resolve_node_gate_decision`] under the 7-arg clippy ceiling.
///
/// `ingest` carries `folder_ontology`/`ontology_default`; `degraded` is the
/// load-time degraded state (carries dropped prefixes + global flag);
/// `schema` is the user's `OntologySelection` for rung 3; `vault_root` is
/// the effective vault root (may be None).
#[derive(Debug, Clone)]
pub struct GateResolutionContext<'a> {
    pub ingest: &'a crate::config::IngestConfig,
    pub degraded: &'a crate::config::OntologyDegradedState,
    pub schema: Option<crate::ontology_config::OntologySelection>,
    pub schema_unparseable: bool,
    pub vault_root: Option<&'a Path>,
}

/// `ingest` carries `folder_ontology`/`ontology_default`; `degraded` is the
/// load-time degraded state (carries dropped prefixes + global flag);
/// `schema` is the user's `OntologySelection` for rung 3; `vault_root` is
/// the effective vault root (may be None).
///
/// The resolver is a one-call wrapper: rung 1's `wiki_get_ontology` lookup
/// runs once, rung 2's path-config call iterates `source_paths`, rung 3
/// reads `schema` and `ontology_default` once, rung 4 falls back to a
/// tier_fact `wiki_get_ontology` lookup if no strict rung fired. Every step
/// is best-effort — a DB fault falls through to the next rung (matching
/// today's edge-cascade behavior).
#[allow(clippy::too_many_arguments)]
pub fn resolve_node_gate_decision(
    conn: &Connection,
    entity_id: &str,
    source_paths: &[String],
    ctx: GateResolutionContext<'_>,
) -> NodeGateDecision {
    let ingest = ctx.ingest;
    let degraded = ctx.degraded;
    let schema = ctx.schema;
    let schema_unparseable = ctx.schema_unparseable;
    let vault_root = ctx.vault_root;
    // Rung 1a — ct_entity_optouts row → opt-out, skip edge gating too (§2.1).
    if entity_has_optout(conn, entity_id).unwrap_or(false) {
        return NodeGateDecision {
            verdict: ModeVerdict::OptOut,
            vocabulary: None,
            source_directory: None,
        };
    }

    // Rung 1b/c/d — the entity's own manifest row. An unreadable row is
    // REPORT-OR-HOLD for nodes per r4-m4; an unmarked row climbs. A STRICT
    // row GATEs — the vocabulary comes from THIS manifest (its fallback_node_type
    // drives the degrade rung).
    match crate::wiki_graph::wiki_get_ontology(conn, entity_id) {
        Ok(o) if o.mode == "strict" => {
            // Ensure runs before the gate resolves (the gate's resolution path),
            // so a fresh install already has the `document`/`process` extensions
            // + `fallback_node_type` written. A pre-wave-1 manifest was caught
            // at the gate-resolve call below; if the strict row STILL lacks a
            // fallback the helper holds per §2.4.5.
            let manifest = o.manifest.as_ref();
            let vocab = manifest.map(NodeVocabulary::from_manifest);
            let strict_with_no_fallback = match &vocab {
                Some(v) => v.fallback().is_none(),
                None => true,
            };
            if strict_with_no_fallback {
                return NodeGateDecision {
                    verdict: ModeVerdict::StrictNoVocab,
                    vocabulary: vocab,
                    source_directory: None,
                };
            }
            return NodeGateDecision {
                verdict: ModeVerdict::Gate,
                vocabulary: vocab,
                source_directory: None,
            };
        }
        Ok(_) => {
            // Not strict (mark explicit OFF/emergent): rung 1d, climb.
        }
        Err(_) => {
            // Unreadable row — REPORT-OR-HOLD for nodes per r4-m4. Match
            // the edge cascade's silent fall-through with one extra step:
            // strict-wins via rungs 2-3 below. If the climb ALSO produces
            // no strict verdict, return Held (loud, not silent).
        }
    }

    // Rungs 2-3 — folder_ontology + ontology_default + schema (strict-wins
    // across all source paths; an `off` loses to any strict rung).
    let mut strict_source_dir: Option<String> = None;
    // First `off` resolution seen at rung 2/3 — when the ladder's final
    // verdict is Off (SKIP), the r21 ledger table records "the `off` folder
    // if rung 2 caused the SKIP" as the row's `source_directory`.
    let mut off_source_dir: Option<String> = None;
    for source in source_paths {
        match ingest.ontology_lookup(source, vault_root, degraded, schema, schema_unparseable) {
            crate::config::OntologyLookup::Mode(crate::config::OntologyMode::Strict) => {
                // Found a strict rung; vocabulary comes from tier_fact below.
                strict_source_dir = Some(source.clone());
                break;
            }
            crate::config::OntologyLookup::Mode(crate::config::OntologyMode::Off) => {
                // Off found, but continue to look for any strict rung
                // (strict-wins, R2.3.3). Remember the off folder for the
                // SKIP ledger row's source_directory (R2.4.6 r21).
                off_source_dir.get_or_insert_with(|| source.clone());
                continue;
            }
            crate::config::OntologyLookup::Hold => {
                return NodeGateDecision {
                    verdict: ModeVerdict::StrictNoVocab,
                    vocabulary: None,
                    source_directory: Some(source.clone()),
                };
            }
            crate::config::OntologyLookup::Climb => {
                // Try the next source; if all climb we fall through to rung 4.
                continue;
            }
        }
    }
    if let Some(dir) = strict_source_dir {
        // Pull the tier_fact vocabulary for the gate.
        let tier_fact_vocab = tier_fact_vocabulary(conn);
        return match tier_fact_vocab {
            Some(v) => NodeGateDecision {
                verdict: ModeVerdict::Gate,
                vocabulary: Some(v),
                source_directory: Some(dir),
            },
            None => NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab,
                vocabulary: None,
                source_directory: Some(dir),
            },
        };
    }

    // Rung 4 — tier_fact itself.
    match crate::wiki_graph::wiki_get_ontology(conn, "tier_fact") {
        Ok(o) if o.mode == "strict" => {
            let vocab = o.manifest.as_ref().map(NodeVocabulary::from_manifest);
            let strict_with_no_fallback = match &vocab {
                Some(v) => v.fallback().is_none(),
                None => true,
            };
            if strict_with_no_fallback {
                NodeGateDecision {
                    verdict: ModeVerdict::StrictNoVocab,
                    vocabulary: vocab,
                    source_directory: None,
                }
            } else {
                NodeGateDecision {
                    verdict: ModeVerdict::Gate,
                    vocabulary: vocab,
                    source_directory: None,
                }
            }
        }
        Ok(_) => {
            // Unmarked/off tier_fact row: §2.3.1 SKIP. If a rung 2/3 lookup
            // resolved off, record that folder — the r21 `gate_skipped` row
            // carries it as `source_directory`.
            NodeGateDecision {
                verdict: ModeVerdict::Off,
                vocabulary: None,
                source_directory: off_source_dir,
            }
        }
        Err(_) => {
            // Corrupt tier_fact manifest: REPORT-OR-HOLD per §2.3 rung 4.
            NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab,
                vocabulary: None,
                source_directory: None,
            }
        }
    }
}

/// The SINGLE production entry point for the four insert sites (LLM
/// synthesis / GUI / bundle / okf_migration). Loads the ingest policy for
/// the connection's brain dir ([`crate::config::ingest_policy_for_db`] —
/// cached per config-file bytes), stamps the initial drift watermark at
/// the first gate resolution (r13-MAJOR-3), and walks the FULL §2.3
/// ladder — rungs 1a/1b/1c/1d, the rung 2/3 `folder_ontology` /
/// `ontology_default` climb via [`resolve_node_gate_decision`], and the
/// rung 4 `tier_fact` fallback.
///
/// `source_paths` per site (spec R2.3.4): LLM synthesis passes the
/// proposal's trigger document paths; okf_migration passes the wiki-page
/// vault-relative path; GUI and bundle import pass `&[]` (no proposal, no
/// document path — the ladder starts at rung 3 and falls to rung 4).
///
/// Watermark placement (fix-round-1 C1): `ImmediateTx::begin` takes only
/// `&mut Connection`, so the config hash cannot reach it without a config
/// load inside `begin` — that would violate the hold-time rule (config is
/// loaded at resolution time, not transaction-open time). The stamp is
/// therefore wired HERE, at every `resolve_*` production call, inside the
/// same IMMEDIATE transaction as the mint: it commits with the insert and
/// rolls back with a Held/abort (the next successful resolution re-stamps;
/// `INSERT OR IGNORE` keeps the FIRST successful stamp, which is the
/// watermark's definition). Best-effort per the spec — the `let _ =`
/// swallows read-only/contended failures.
pub fn resolve_production_gate(
    tx: &ImmediateTx<'_>,
    entity_id: &str,
    source_paths: &[String],
) -> (GateDecision, NodeGateDecision) {
    let conn: &Connection = tx;
    let policy = crate::config::ingest_policy_for_db(conn.path());
    let degraded = policy.ontology_degraded_state();

    // C1 (r13-MAJOR-3): the INITIAL watermark is stamped at the first gate
    // resolution so gate-time degrade decisions are always made under a
    // recorded config hash.
    let config_hash = crate::config::ontology_config_watermark_hash(
        &policy.tiers,
        policy.ontology_selection,
        policy.ontology_unparseable,
    );
    let _ = stamp_initial_watermark(tx, &config_hash);

    let ctx = GateResolutionContext {
        ingest: &policy.tiers,
        degraded: &degraded,
        schema: policy.ontology_selection,
        schema_unparseable: policy.ontology_unparseable,
        vault_root: policy.vault_root.as_deref(),
    };
    let node = resolve_node_gate_decision(conn, entity_id, source_paths, ctx);
    (node.clone().into_gate_decision(), node)
}

/// The SINGLE origin-ledger writer for gate outcomes shared by the insert
/// sites (fix-round-1 I3; replaces the former per-site copies in
/// `commit.rs` / `entities.rs`). Row shape per the R2.4.6 r21 table:
///
///   * SKIP outcome → `gate_skipped` row; `original_type` = the label the
///     outcome carried (the proposed label, or `None` when the caller
///     supplied none — never `''`); `source_directory` = the `off` folder
///     when rung 2 caused the SKIP, else NULL.
///   * `DegradedToFallback` → `degraded` row with the original label,
///     trimmed, pre-canonicalization; `source_directory` NULL (degrade
///     rows only carry a directory when off-sourced, which cannot happen
///     on a Gate verdict).
///   * Admitted-as-declared / alias-admitted / held → NO row.
///
/// Bundle import's `unlabeled_landing` row and okf_migration's
/// `Some("concept")` skip-row original_type are site-specific per the same
/// table and stay at their sites.
pub fn write_gate_origin_ledger(
    tx: &ImmediateTx<'_>,
    entity_id: &str,
    outcome: &AdmitOutcome,
    decision: GateDecision,
    skip_source_directory: Option<&str>,
) -> Result<()> {
    match (decision, outcome) {
        (GateDecision::Skip, AdmitOutcome::Skipped { original_label }) => {
            write_origin_ledger_row(
                tx,
                entity_id,
                original_label.as_deref(),
                OriginReason::GateSkipped,
                skip_source_directory,
            )?;
        }
        (_, AdmitOutcome::DegradedToFallback { original_label, .. }) => {
            let label: Option<&str> = if !original_label.is_empty() {
                Some(original_label.as_str())
            } else {
                None
            };
            write_origin_ledger_row(tx, entity_id, label, OriginReason::Degraded, None)?;
        }
        _ => {
            // Admitted-as-declared, alias-admitted, held: NO ledger row.
        }
    }
    Ok(())
}

/// Look up the tier_fact vocabulary (the rung 4 fallback when a strict rung
/// 2/3 fires). Memoized per-call: rung 2 may deliver a strict verdict, but
/// the tier_fact `wiki_get_ontology` runs at most ONCE per gate resolution.
fn tier_fact_vocabulary(conn: &Connection) -> Option<NodeVocabulary> {
    match crate::wiki_graph::wiki_get_ontology(conn, "tier_fact") {
        Ok(o) if o.mode == "strict" => o.manifest.as_ref().map(NodeVocabulary::from_manifest),
        _ => None,
    }
}

/// Rung 1a — does this entity have a deliberate opt-out row?
pub(crate) fn entity_has_optout(conn: &Connection, entity_id: &str) -> rusqlite::Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ct_entity_optouts WHERE entity_id = ?1",
            [entity_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(count > 0)
}

/// Insert an `entity_type_origin` ledger row (spec R2.4.6 r21).
///
/// `INSERT OR IGNORE` — first-origin-wins. A heal retype of an entity that
/// already has a row writes nothing (the spec's reversibility record; r21
/// locks in the first label, never overwrites it).
///
/// `original_type` MUST be `None` for "no label supplied" — never `''`
/// (r21 contract; a sentinel string is mistakable for a real label).
///
/// Returns the rows affected (0 = entity already had a row; 1 = new row).
pub fn write_origin_ledger_row(
    tx: &ImmediateTx<'_>,
    entity_id: &str,
    original_type: Option<&str>,
    reason: OriginReason,
    source_directory: Option<&str>,
) -> Result<usize> {
    let conn: &Connection = tx;
    let now = crate::db::commit::now_timestamps().0;
    let rows = conn.execute(
        "INSERT OR IGNORE INTO entity_type_origin
         (entity_id, original_type, reason, source_directory, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            entity_id,
            original_type,
            reason.as_str(),
            source_directory,
            now
        ],
    )?;
    Ok(rows)
}

/// Stamp the initial watermark row at first gate/heal resolution (r13-MAJOR-3).
///
/// Best-effort: skipped on read-only or contended connections, where the
/// write would fail. The function returns the rows affected (0 on a
/// contention failure or already-stamped row, 1 on a fresh stamp) so tests
/// can pin both branches.
pub fn stamp_initial_watermark(tx: &ImmediateTx<'_>, config_hash: &str) -> Result<usize> {
    let conn: &Connection = tx;
    let rows = conn.execute(
        "INSERT OR IGNORE INTO llm_wiki_meta (key, value)
         VALUES ('initial_drift_watermark', ?1)",
        params![config_hash],
    )?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::wiki_graph::WikiNodeType;
    use rusqlite::Connection;

    // Helper used by all ensure_manifest_vocabulary tests to insert a manifest
    // row (the DDL declares `updated_at NOT NULL`).
    fn insert_manifest(conn: &Connection, entity_id: &str, mode: &str, manifest_json: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES (?1, ?2, ?3, 1)",
            params![entity_id, mode, manifest_json],
        )
        .unwrap();
    }

    /// The 17 EA seed slugs (spec §1.1) — pinned here as a string so the
    /// tests can assert the constant has not silently dropped a slug.
    #[test]
    fn ea_seed_types_match_spec_section_1_1() {
        let expected: &[&str] = &[
            "action",
            "creativework",
            "design_spec",
            "event",
            "handoff",
            "organization",
            "person",
            "place",
            "procedure",
            "product",
            "project",
            "reference_doc",
            "review",
            "role",
            "service",
            "session_recap",
            "software_application",
        ];
        assert_eq!(EA_SEED_TYPES, expected);
        assert_eq!(EA_SEED_TYPES.len(), 17);
    }

    /// `fallback_node_type` reader: declared fallback is what the ladder uses.
    #[test]
    fn node_vocabulary_reads_fallback_node_type() {
        let manifest = WikiManifest {
            node_types: vec![WikiNodeType {
                type_name: "person".into(),
                ..Default::default()
            }],
            edge_types: vec![],
            fallback_node_type: Some("project".into()),
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        assert_eq!(v.fallback(), Some("project"));
    }

    /// Empty-declared-set rule (spec §2.4.5): `is_empty()` reports true and
    /// the admit ladder holds.
    #[test]
    fn empty_declared_set_holds() {
        let manifest = WikiManifest {
            node_types: vec![],
            edge_types: vec![],
            fallback_node_type: Some("project".into()),
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        assert!(v.is_empty());
        match run_admit_ladder(&v, Some("person")) {
            AdmitInternal::Held => {}
            other => panic!("expected Held, got {other:?}"),
        }
    }

    /// Canonicalize casing: `Person` is admitted-as-declared only when the
    /// manifest declares `person`.
    #[test]
    fn canonicalize_case_insensitive() {
        let manifest = WikiManifest {
            node_types: vec![WikiNodeType {
                type_name: "person".into(),
                ..Default::default()
            }],
            edge_types: vec![],
            fallback_node_type: None,
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        assert_eq!(v.canonicalize("Person"), Some("person"));
        assert_eq!(v.canonicalize("  person  "), Some("person"));
        assert!(v.contains("PERSON"));
        assert!(!v.contains("agent"));
    }

    /// Alias ladder rung (r2-M1): agent → role when role is declared.
    #[test]
    fn alias_ladder_declared_target() {
        let manifest = WikiManifest {
            node_types: vec![
                WikiNodeType {
                    type_name: "person".into(),
                    ..Default::default()
                },
                WikiNodeType {
                    type_name: "role".into(),
                    ..Default::default()
                },
            ],
            edge_types: vec![],
            fallback_node_type: Some("project".into()),
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        match run_admit_ladder(&v, Some("agent")) {
            AdmitInternal::Aliased {
                original_label,
                landed_as,
            } => {
                assert_eq!(original_label, "agent");
                assert_eq!(landed_as, "role");
            }
            other => panic!("expected Aliased, got {other:?}"),
        }
    }

    /// Alias ladder rung: target UNDECLARED → falls through to fallback.
    #[test]
    fn alias_ladder_undeclared_target_falls_through() {
        let manifest = WikiManifest {
            node_types: vec![WikiNodeType {
                type_name: "person".into(),
                ..Default::default()
            }],
            edge_types: vec![],
            fallback_node_type: Some("project".into()),
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        match run_admit_ladder(&v, Some("agent")) {
            AdmitInternal::DegradedToFallback {
                original_label,
                landed_as,
            } => {
                assert_eq!(original_label, "agent");
                assert_eq!(landed_as, "project");
            }
            other => panic!("expected DegradedToFallback, got {other:?}"),
        }
    }

    /// Undeclared label → fallback rung when fallback is declared.
    #[test]
    fn undeclared_label_degrades_to_fallback() {
        let manifest = WikiManifest {
            node_types: vec![WikiNodeType {
                type_name: "person".into(),
                ..Default::default()
            }],
            edge_types: vec![],
            fallback_node_type: Some("project".into()),
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        match run_admit_ladder(&v, Some("character")) {
            AdmitInternal::DegradedToFallback {
                original_label,
                landed_as,
            } => {
                assert_eq!(original_label, "character");
                assert_eq!(landed_as, "project");
            }
            other => panic!("expected DegradedToFallback, got {other:?}"),
        }
    }

    /// Undeclared label + no fallback declared → Held.
    #[test]
    fn undeclared_label_no_fallback_holds() {
        let manifest = WikiManifest {
            node_types: vec![WikiNodeType {
                type_name: "person".into(),
                ..Default::default()
            }],
            edge_types: vec![],
            fallback_node_type: None,
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        assert!(matches!(
            run_admit_ladder(&v, Some("character")),
            AdmitInternal::Held
        ));
    }

    /// EA-subset manifest → ensure extends AND sets fallback to `project`.
    #[test]
    fn ensure_extends_ea_seed_and_sets_project_fallback() {
        let conn = open_in_memory().unwrap();
        let types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "tier_fact", "a", &manifest_json);

        let outcome = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        match outcome {
            EnsureOutcome::Ensured {
                extended,
                fallback_set,
            } => {
                assert!(extended);
                assert!(fallback_set);
            }
            other => panic!("expected Ensured, got {other:?}"),
        }

        // Re-running is a no-op memo.
        let outcome2 = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        assert_eq!(outcome2, EnsureOutcome::AlreadyEnsured);

        // Read back the manifest_json and parse it.
        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        let slugs: Vec<String> = parsed["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["type"].as_str().map(String::from))
            .collect();
        assert!(slugs.iter().any(|s| s == "document"));
        assert!(slugs.iter().any(|s| s == "process"));
        assert_eq!(parsed["fallback_node_type"].as_str(), Some("project"));
    }

    /// Manifest with `concept` already declared → ensure picks `concept`
    /// as the fallback (spec r10-M1, "the live ThinkPad row").
    #[test]
    fn ensure_prefers_concept_when_already_declared() {
        let conn = open_in_memory().unwrap();
        let mut types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        types.push(json!({"type": "concept"}));
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "tier_fact", "a", &manifest_json);

        let outcome = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        assert!(matches!(outcome, EnsureOutcome::Ensured { .. }));

        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["fallback_node_type"].as_str(), Some("concept"));
    }

    /// Foreign manifest → fallback-only treatment; no `document`/`process`
    /// injection.
    #[test]
    fn ensure_foreign_manifest_gets_fallback_only() {
        let conn = open_in_memory().unwrap();
        // 9-type SchemaOrg manifest.
        let types: Vec<serde_json::Value> = [
            "person",
            "organization",
            "place",
            "event",
            "project",
            "action",
            "creativework",
            "review",
            "product",
        ]
        .into_iter()
        .map(|s| json!({"type": s}))
        .collect();
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "schema_org_vault", "a", &manifest_json);

        let outcome = ensure_manifest_vocabulary(&conn, "schema_org_vault").unwrap();
        match outcome {
            EnsureOutcome::Ensured {
                extended,
                fallback_set,
            } => {
                assert!(
                    !extended,
                    "foreign manifest must NOT be extended with document/process"
                );
                assert!(
                    fallback_set,
                    "foreign manifest's fallback must still be set"
                );
            }
            other => panic!("expected Ensured (no extension, fallback set), got {other:?}"),
        }

        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'schema_org_vault'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        let slugs: Vec<String> = parsed["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["type"].as_str().map(String::from))
            .collect();
        assert!(!slugs.iter().any(|s| s == "document"));
        assert!(!slugs.iter().any(|s| s == "process"));
        assert_eq!(parsed["fallback_node_type"].as_str(), Some("project"));
    }

    /// Foreign manifest whose declared set does NOT include `concept` or
    /// `project` → loud §2.4.5 signal (no fallback written).
    #[test]
    fn ensure_foreign_manifest_no_preferred_fallback_loud() {
        let conn = open_in_memory().unwrap();
        let types: Vec<serde_json::Value> = (0..5)
            .map(|i| json!({"type": format!("novel_{i}")}))
            .collect();
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "foreign_vault", "a", &manifest_json);

        let outcome = ensure_manifest_vocabulary(&conn, "foreign_vault").unwrap();
        assert_eq!(
            outcome,
            EnsureOutcome::ForeignNoPreferredFallback {
                entity_id: "foreign_vault".into()
            }
        );
    }

    /// Hand-stripped `fallback_node_type` → ensure re-runs and writes it.
    #[test]
    fn ensure_hand_stripped_key_re_runs() {
        let conn = open_in_memory().unwrap();
        let types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        // No fallback_node_type in the stored manifest.
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "tier_fact", "a", &manifest_json);
        // First run sets fallback.
        let outcome = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        assert!(matches!(outcome, EnsureOutcome::Ensured { .. }));

        // Hand-strip the key in the stored JSON.
        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        parsed.as_object_mut().unwrap().remove("fallback_node_type");
        let stripped = serde_json::to_string(&parsed).unwrap();
        conn.execute(
            "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1 WHERE entity_id = 'tier_fact'",
            params![stripped],
        )
        .unwrap();
        // Also clear the memo so we re-run.
        conn.execute("DELETE FROM manifest_ensure_memo", [])
            .unwrap();

        let outcome = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        assert!(matches!(
            outcome,
            EnsureOutcome::Ensured {
                fallback_set: true,
                ..
            }
        ));
    }

    /// Pre-wave-1 manifest WITHOUT `fallback_node_type` → ensure writes the
    /// per-manifest fallback (matrix case from spec §2.4.4).
    #[test]
    fn ensure_pre_wave_1_manifest_writes_fallback() {
        let conn = open_in_memory().unwrap();
        // A strict manifest that already declared the EA seed but never had the
        // key. This is the most common pre-wave-1 shape.
        let types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "tier_fact", "s", &manifest_json);

        let outcome = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        assert!(matches!(
            outcome,
            EnsureOutcome::Ensured {
                fallback_set: true,
                ..
            }
        ));

        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["fallback_node_type"].as_str(), Some("project"));
    }

    /// Memo records only after commit: a transaction that aborts leaves the
    /// memo absent (r11-m4).
    #[test]
    fn ensure_memo_records_only_after_commit() {
        let conn = open_in_memory().unwrap();
        let types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "tier_fact", "a", &manifest_json);

        ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        let memo_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM manifest_ensure_memo WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(memo_count, 1, "ensure must record its memo post-commit");
    }

    /// Single-hop redirect: A → B returns Survivor(B).
    #[test]
    fn single_hop_redirect_returns_survivor() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES (?1, ?2, 1)",
            params!["ent_a", "B"],
        )
        .unwrap();
        let outcome = single_hop_redirect(&conn, "ent_a");
        assert_eq!(outcome, RedirectOutcome::Survivor("B".into()));
    }

    /// Cycle (A → A) reports Cycle, never loops.
    #[test]
    fn single_hop_redirect_detects_self_cycle() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES (?1, ?2, 1)",
            params!["ent_a", "ent_a"],
        )
        .unwrap();
        let outcome = single_hop_redirect(&conn, "ent_a");
        assert_eq!(outcome, RedirectOutcome::Cycle);
    }

    /// No redirect → `None`.
    #[test]
    fn single_hop_redirect_returns_none_when_absent() {
        let conn = open_in_memory().unwrap();
        let outcome = single_hop_redirect(&conn, "ent_a");
        assert_eq!(outcome, RedirectOutcome::None);
    }

    /// Engine-rewrite survival of the CT table (spec R2.4.4 / §6.10 r10-M3,
    /// Task 2 matrix case): when an engine manifest rewrite updates
    /// `llm_wiki_entity_manifests.manifest_json`, a `ct_entity_optouts` row
    /// for the same entity MUST survive the rewrite — not be wiped by the
    /// ensure step.
    #[test]
    fn ensure_engine_rewrite_preserves_ct_entity_optouts() {
        let conn = open_in_memory().unwrap();
        let types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        insert_manifest(&conn, "tier_fact", "a", &manifest_json);
        // The CT table row that must survive the rewrite.
        conn.execute(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at) VALUES (?1, ?2, 1)",
            params!["tier_fact", "user opt-out"],
        )
        .unwrap();

        // Engine-rewrite simulation: a new manifest_json payload with
        // different fallback + content hash.
        let rewritten = serde_json::json!({
            "node_types": types,
            "edge_types": [],
            "fallback_node_type": "concept",
            "marker": "engine-rewritten",
        });
        let rewritten_json = serde_json::to_string(&rewritten).unwrap();
        conn.execute(
            "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1 WHERE entity_id = ?2",
            params![rewritten_json, "tier_fact"],
        )
        .unwrap();

        ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();

        let reason: Option<String> = conn
            .query_row(
                "SELECT reason FROM ct_entity_optouts WHERE entity_id = ?1",
                params!["tier_fact"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            reason.as_deref(),
            Some("user opt-out"),
            "ct_entity_optouts row must survive an engine manifest rewrite",
        );
    }

    /// §6 item 2: write-time gate at all four insert paths through the
    /// shared helper. The helper's `&ImmediateTx` parameter type-checks
    /// (a plain `&Connection` does not compile); we verify the four
    /// outcomes on a fresh brain (no manifest row → SKIP path) here.
    #[test]
    fn gate_runs_at_all_four_insert_paths_on_skip() {
        let conn = open_in_memory().unwrap();
        // Fresh brain (no `tier_fact` row): all four paths land on the
        // SKIP path with the literal `'concept'` and a `gate_skipped`
        // ledger row. No invented label enters `entity_type`.
        let mut conn = conn;

        // LLM synthesis path (mirrors `commit::create_entity_if_needed`):
        // open IMMEDIATE tx, gate call, land SKIP-path literal + ledger.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let decision = GateDecision::Skip;
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_llm"),
            "LLM-mint",
            Some("concept"),
            "summary",
            100,
            decision,
            false,
        )
        .unwrap();
        assert_eq!(
            outcome,
            AdmitOutcome::Skipped {
                original_label: Some("concept".into())
            }
        );
        // Caller-side: insert the literal `'concept'` and write the
        // ledger row (this is the production pattern; the helper does
        // NOT insert on Skip).
        tx.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, 'concept', '', 100, 100)",
            params!["ent_llm", "LLM-mint"],
        )
        .unwrap();
        write_origin_ledger_row(
            &tx,
            "ent_llm",
            Some("concept"),
            crate::db::schema::OriginReason::GateSkipped,
            None,
        )
        .unwrap();
        tx.commit().unwrap();

        // GUI path: SKIP, helper does NOT insert, caller lands literal.
        let mut conn2 = open_in_memory().unwrap();
        let tx = ImmediateTx::begin(&mut conn2).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_gui"),
            "GUI-mint",
            None,
            "summary",
            100,
            GateDecision::Skip,
            false,
        )
        .unwrap();
        assert!(matches!(
            outcome,
            AdmitOutcome::Skipped {
                original_label: None
            }
        ));
        tx.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, 'concept', '', 100, 100)",
            params!["ent_gui", "GUI-mint"],
        )
        .unwrap();
        tx.commit().unwrap();

        // Bundle path: SKIP, helper does NOT insert, caller lands
        // `'concept'` literal + `gate_skipped` ledger row (r2-M2a).
        let mut conn3 = open_in_memory().unwrap();
        let tx = ImmediateTx::begin(&mut conn3).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_bundle"),
            "Bundle-mint",
            None,
            "summary",
            100,
            GateDecision::Skip,
            false,
        )
        .unwrap();
        assert!(matches!(outcome, AdmitOutcome::Skipped { .. }));
        tx.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, 'concept', '', 100, 100)",
            params!["ent_bundle", "Bundle-mint"],
        )
        .unwrap();
        tx.commit().unwrap();

        // okf_migration path: SKIP, upsert mode, preserves `entity_type`.
        let mut conn4 = open_in_memory().unwrap();
        // Pre-existing row with the same id should be left alone for
        // `entity_type` (r6-M3).
        conn4.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, 'Pre-existing', 'document', 'pre', 50, 50)",
            params!["ent_okf"],
        )
        .unwrap();
        let tx = ImmediateTx::begin(&mut conn4).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_okf"),
            "okf-mint",
            None,
            "summary",
            100,
            GateDecision::Skip,
            true, // upsert mode for okf_migration
        )
        .unwrap();
        assert!(matches!(outcome, AdmitOutcome::Skipped { .. }));
        tx.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, 'concept', ?3, NULL, 100, 100, NULL)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               summary = excluded.summary,
               updated_at = excluded.updated_at",
            params!["ent_okf", "okf-mint", "summary"],
        )
        .unwrap();
        tx.commit().unwrap();
        // The upsert preserved the pre-existing `document` type (r6-M3).
        let entity_type: String = conn4
            .query_row(
                "SELECT entity_type FROM curated_entities WHERE id = 'ent_okf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            entity_type, "document",
            "upsert mode preserves entity_type (r6-M3)"
        );
    }

    /// §6 item 2: no invented label ever enters `entity_type`. The
    /// shared helper's GATE decision only admits declared / aliased
    /// / degraded-to-fallback; the SKIP and Held paths never
    /// fabricate a label.
    #[test]
    fn gate_no_invented_label() {
        let conn = open_in_memory().unwrap();
        let mut conn = conn;

        // Held path: helper does NOT insert, caller surfaces refusal.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_held"),
            "Held-mint",
            Some("character"),
            "summary",
            100,
            GateDecision::Held,
            false,
        )
        .unwrap();
        assert!(matches!(outcome, AdmitOutcome::Held { .. }));
        // No `curated_entities` row was inserted.
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM curated_entities WHERE id = 'ent_held'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "Held must NOT insert a row");
        tx.commit().unwrap();
    }

    /// §6 item 2: SG6 (held proposal's facts survive). The Held path
    /// refuses the entity INSERT but does NOT drop the proposal's facts.
    /// This test pins the shape: a separate `llm_wiki_entries` row
    /// written before the gate call survives the Held mint.
    #[test]
    fn gate_held_proposal_keeps_facts() {
        let conn = open_in_memory().unwrap();
        let mut conn = conn;
        // Fact inserted OUTSIDE the helper (the proposal flow already
        // committed the fact insert in its own transaction).
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at)
             VALUES ('fact_1', 'ent_held', 'Title', 'Body', '[]', 'inferred', 'librarian_inferred', 100, 100)",
            [],
        )
        .unwrap();

        // Now the gate Held-out the entity mint.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_held"),
            "Held-mint",
            Some("character"),
            "summary",
            200,
            GateDecision::Held,
            false,
        )
        .unwrap();
        assert!(matches!(outcome, AdmitOutcome::Held { .. }));
        // Held does NOT touch `llm_wiki_entries` (the helper never
        // writes to it). The fact row is intact.
        tx.commit().unwrap();
        let fact_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'fact_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fact_count, 1, "Held must NOT drop the proposal's facts");
    }

    /// §6 item 2: the shared helper's degrade ladder for GATE decisions.
    /// Tests the full ladder end-to-end through `shared_insert_entity`:
    /// declared → admitted; alias → aliased to target; undeclared with
    /// fallback → degraded; undeclared no-fallback → held.
    #[test]
    fn gate_degrade_ladder_through_helper() {
        let conn = open_in_memory().unwrap();
        let mut conn = conn;

        // Build a vocabulary with declared = person, role + fallback = project.
        let vocab = {
            let mut by_key = std::collections::HashMap::new();
            by_key.insert("person".to_string(), "person".to_string());
            by_key.insert("role".to_string(), "role".to_string());
            NodeVocabulary {
                by_key,
                fallback: Some("project".to_string()),
            }
        };

        // 1) Declared → AdmittedDeclared.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_person"),
            "Person-mint",
            Some("person"),
            "",
            100,
            GateDecision::Gate(vocab.clone()),
            false,
        )
        .unwrap();
        match outcome {
            AdmitOutcome::AdmittedDeclared { label, .. } => assert_eq!(label, "person"),
            other => panic!("expected AdmittedDeclared, got {other:?}"),
        }
        tx.commit().unwrap();

        // 2) Alias (agent → role when role is declared).
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_alias"),
            "Alias-mint",
            Some("agent"),
            "",
            200,
            GateDecision::Gate(vocab.clone()),
            false,
        )
        .unwrap();
        match outcome {
            AdmitOutcome::Aliased {
                original_label,
                landed_as,
                ..
            } => {
                assert_eq!(original_label, "agent");
                assert_eq!(landed_as, "role");
            }
            other => panic!("expected Aliased, got {other:?}"),
        }
        tx.commit().unwrap();

        // 3) Undeclared with fallback → DegradedToFallback.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_character"),
            "Character-mint",
            Some("character"),
            "",
            300,
            GateDecision::Gate(vocab.clone()),
            false,
        )
        .unwrap();
        match outcome {
            AdmitOutcome::DegradedToFallback {
                original_label,
                landed_as,
                ..
            } => {
                assert_eq!(original_label, "character");
                assert_eq!(landed_as, "project");
            }
            other => panic!("expected DegradedToFallback, got {other:?}"),
        }
        tx.commit().unwrap();

        // 4) Empty vocabulary → Held (the empty-vocab rule is enforced
        // BEFORE the ladder runs).
        let empty_vocab = NodeVocabulary::default();
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_empty"),
            "Empty-mint",
            Some("anything"),
            "",
            400,
            GateDecision::Gate(empty_vocab),
            false,
        )
        .unwrap();
        assert!(matches!(outcome, AdmitOutcome::Held { .. }));
        tx.commit().unwrap();
    }

    /// Fix-round-1 C1 (r13-MAJOR-3): the production gate resolver stamps
    /// the INITIAL watermark at the FIRST gate resolution, and the stamp is
    /// idempotent across re-resolutions (`INSERT OR IGNORE` keeps the first
    /// value — the watermark's definition).
    #[test]
    fn production_gate_stamps_initial_watermark() {
        let mut conn = open_in_memory().unwrap();
        // Fresh brain: no manifest rows → ladder falls to rung 4 → Off/SKIP.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let (decision, _node) = resolve_production_gate(&tx, "ent_new", &[]);
        assert!(matches!(decision, GateDecision::Skip));
        tx.commit().unwrap();

        let stamped: Option<String> = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'initial_drift_watermark'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert!(
            stamped.is_some(),
            "first gate resolution must stamp initial_drift_watermark"
        );

        // Re-resolution does not duplicate or overwrite the row.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let _ = resolve_production_gate(&tx, "ent_other", &[]);
        tx.commit().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_meta WHERE key = 'initial_drift_watermark'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "watermark stamp must be idempotent");
    }

    /// Fix-round-1 I2 (Ruling R6): the production resolver walks rungs 2/3
    /// of the §2.3 ladder via the connection's ingest policy — a strict
    /// `folder_ontology` prefix GATEs (vocabulary pulled from tier_fact),
    /// an `off` prefix SKIPs with the off folder recorded as the ledger
    /// `source_directory` (R2.4.6 r21).
    #[test]
    fn production_gate_walks_rung_2_folder_ontology() {
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
            ],
            || {
                // Strict case: `ops` folder is strict; tier_fact carries a
                // strict manifest with fallback → Gate with tier_fact vocab.
                {
                    let brain = tempfile::TempDir::new().unwrap();
                    std::fs::write(
                        brain.path().join("config.json"),
                        r#"{"vault_path":"/v","ingest":{"folder_ontology":{"ops":"strict"}}}"#,
                    )
                    .unwrap();
                    let mut conn =
                        crate::db::connection::open_app_db(&brain.path().join("brain.db"), None)
                            .unwrap();
                    let manifest = serde_json::json!({
                        "node_types": [{"type": "person"}],
                        "edge_types": [],
                        "fallback_node_type": "person",
                    });
                    insert_manifest(
                        &conn,
                        "tier_fact",
                        "strict",
                        &serde_json::to_string(&manifest).unwrap(),
                    );

                    let tx = ImmediateTx::begin(&mut conn).unwrap();
                    let (decision, node) =
                        resolve_production_gate(&tx, "ent_new", &["ops/a.md".to_string()]);
                    tx.commit().unwrap();
                    assert!(
                        matches!(decision, GateDecision::Gate(_)),
                        "strict folder_ontology rung must Gate, got {decision:?}"
                    );
                    assert_eq!(node.source_directory.as_deref(), Some("ops/a.md"));
                    assert_eq!(node.verdict, ModeVerdict::Gate);
                }

                // Off case: `ops` folder is off; fresh brain (no manifest
                // rows) → Off/SKIP with the off folder recorded.
                {
                    let brain = tempfile::TempDir::new().unwrap();
                    std::fs::write(
                        brain.path().join("config.json"),
                        r#"{"vault_path":"/v","ingest":{"folder_ontology":{"ops":"off"}}}"#,
                    )
                    .unwrap();
                    let mut conn =
                        crate::db::connection::open_app_db(&brain.path().join("brain.db"), None)
                            .unwrap();

                    let tx = ImmediateTx::begin(&mut conn).unwrap();
                    let (decision, node) =
                        resolve_production_gate(&tx, "ent_new", &["ops/a.md".to_string()]);
                    tx.commit().unwrap();
                    assert!(matches!(decision, GateDecision::Skip));
                    assert_eq!(node.verdict, ModeVerdict::Off);
                    assert_eq!(
                        node.source_directory.as_deref(),
                        Some("ops/a.md"),
                        "rung-2-caused SKIP must record the off folder (R2.4.6 r21)"
                    );
                }
            },
        );
    }
}
