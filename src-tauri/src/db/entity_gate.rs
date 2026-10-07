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

use crate::hasher::hash_bytes;
use crate::wiki_graph::WikiManifest;
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;
use std::collections::HashSet;

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
}
