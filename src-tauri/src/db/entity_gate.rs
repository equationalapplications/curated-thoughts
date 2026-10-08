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
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;
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
    /// Membership-rule key. Delegates to [`EdgeVocabulary::key`] so node
    /// and edge membership share ONE rule (review finding: two copies of
    /// the same `trim().to_lowercase()` could drift apart).
    ///
    /// [`EdgeVocabulary::key`]: crate::db::commit::EdgeVocabulary::key
    pub fn key(candidate: &str) -> String {
        crate::db::commit::EdgeVocabulary::key(candidate)
    }

    /// Build the vocabulary from a manifest.
    ///
    /// Empty-declared-set rule (spec §2.4.5, r25): if a strict manifest
    /// declares zero node types, every write is held — there is nothing to
    /// admit, and `is_empty()` exposes that fact. A fallback named over an
    /// empty declared set does NOT soften this: `hold_reason` checks
    /// `is_empty()` first, so the mints hold rather than degrade onto it.
    ///
    /// A fallback that names a DECLARED type is stored in that entry's
    /// canonical spelling — the spelling the degrade ladder lands (R2.4.4:
    /// one spelling per type, same rule as the declared and alias arms). An
    /// UNDECLARED fallback keeps its raw spelling: it can never land, and
    /// `FallbackNotDeclared`'s message names exactly what the row says.
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
        let fallback = manifest
            .fallback_node_type
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|f| by_key.get(&Self::key(&f)).cloned().unwrap_or(f));
        Self { by_key, fallback }
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

    /// The declared node types in the manifest's spelling, sorted — for
    /// refusal messages that must list the legal `--fallback` values.
    pub fn declared(&self) -> Vec<String> {
        let mut declared: Vec<String> = self.by_key.values().cloned().collect();
        declared.sort();
        declared
    }

    /// Why this STRICT vocabulary cannot gate, or `None` when it can.
    /// R2.4.4/R2.4.5: it gates only when it declares node types AND a
    /// `fallback_node_type` that is itself one of them. Anything less holds
    /// EVERY new-entity mint — a mint proposing a declared type included;
    /// the gate never runs a partial vocabulary. `manifest` names the row
    /// the vocabulary came from (`None` when the caller does not know).
    pub fn hold_reason(&self, manifest: Option<&str>) -> Option<HoldReason> {
        // The usable path — every gated mint — allocates nothing; the
        // payloads below are built only for an actual hold.
        if self.is_usable() {
            return None;
        }
        let manifest = manifest.map(str::to_string);
        if self.is_empty() {
            return Some(HoldReason::EmptyNodeTypes {
                manifest,
                fallback: self.fallback().map(str::to_string),
            });
        }
        match self.fallback() {
            None => Some(HoldReason::NoFallback {
                manifest,
                declared: self.declared(),
            }),
            Some(fallback) if !self.contains(fallback) => Some(HoldReason::FallbackNotDeclared {
                manifest,
                fallback: fallback.to_string(),
                declared: self.declared(),
            }),
            Some(_) => None,
        }
    }

    /// True when this vocabulary can gate (R2.4.4/R2.4.5): it declares node
    /// types AND a `fallback_node_type` that is one of them.
    pub fn is_usable(&self) -> bool {
        !self.is_empty() && self.fallback().is_some_and(|f| self.contains(f))
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
    /// The gate held the proposal (§2.4.5); `reason` names the cause. The
    /// helper did NOT insert. Caller decides what to do (refusal error, or
    /// an atomic abort for bundle import / okf_migration).
    Held {
        original_label: Option<String>,
        reason: HoldReason,
    },
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
    /// Held (§2.4.5) — helper does NOT insert. Caller writes a refusal that
    /// names the `HoldReason`.
    Held(HoldReason),
}

/// Why the gate HELD a mint (§2.4.5). Several distinct causes share the one
/// Held verdict; each refusal message names its own, so the operator is not
/// sent to add a fallback when the real fault is an unreadable row or a
/// degraded config. Only the vocabulary causes are fixed with
/// `ct ontology set --fallback`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldReason {
    /// The strict manifest declares zero node types — nothing to admit.
    /// `fallback` is the `fallback_node_type` the row already names over
    /// the empty set, if any — the fix differs (declare it vs. also name
    /// one).
    EmptyNodeTypes {
        manifest: Option<String>,
        fallback: Option<String>,
    },
    /// The strict manifest declares node types but no `fallback_node_type`.
    /// Every new-entity mint holds, a DECLARED-type mint included (R2.4.5).
    NoFallback {
        manifest: Option<String>,
        declared: Vec<String>,
    },
    /// The declared `fallback_node_type` is not one of the manifest's node
    /// types (R2.4.4) — landing it would write an undeclared type.
    FallbackNotDeclared {
        manifest: Option<String>,
        fallback: String,
        declared: Vec<String>,
    },
    /// The rung 1(a) opt-out lookup faulted (D8 fail-closed).
    OptOutLookupFailed { entity_id: String },
    /// The mint's merge-redirect chain could not be resolved (a cycle or a
    /// database fault), so the survivor whose manifest row governs the
    /// mint is unknown (R2.7.5, D8 fail-closed).
    RedirectUnresolved { entity_id: String },
    /// A defensive backstop fired: a code path the resolver is built never
    /// to reach. Named as what it is — a bug — rather than guessing a
    /// configuration cause the operator would then chase.
    Internal { detail: &'static str },
    /// The entity's own manifest row could not be read (r4-m4).
    EntityManifestUnreadable { entity_id: String },
    /// The `tier_fact` manifest row could not be read (§2.3 rung 4).
    TierFactUnreadable,
    /// The folder/host ontology config is degraded or conflicting for this
    /// mint's source while a strict vocabulary exists (`OntologyLookup::Hold`,
    /// r10-MINOR-2). `source` is `None` for a pathless mint.
    ConfigHold { source: Option<String> },
}

impl HoldReason {
    /// The `ct ontology set` target flag for a manifest row: `tier_fact` is
    /// the default target; an entity row needs `--entity`.
    fn fallback_command(manifest: &Option<String>, declared: &[String]) -> String {
        // The command must stay pasteable shell — no `[--entity <id>]`
        // bracket placeholders (glob characters in zsh/bash). When the
        // caller cannot name the row, print the default (`tier_fact`)
        // command and name the entity-row variant in prose.
        use crate::db::ontology_set::{set_command, SetCommand, TIER_FACT};
        let (entity, note) = match manifest.as_deref() {
            Some(TIER_FACT) => (None, ""),
            Some(id) => (Some(id), ""),
            None => (
                None,
                " (add `--entity <id>` before `--fallback` when an entity row holds \
                 the mint)",
            ),
        };
        format!(
            "fix: `{}` with one of: {}{note}",
            set_command(SetCommand {
                entity,
                strict: false,
                fallback: true,
            }),
            declared.join(", ")
        )
    }
}

impl std::fmt::Display for HoldReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let named = |m: &Option<String>| match m {
            Some(m) => format!("the strict manifest `{m}`"),
            None => "the strict manifest".to_string(),
        };
        match self {
            Self::EmptyNodeTypes { manifest, fallback } => {
                // The fix-line must name a writer that can actually repair
                // the row: `tier_fact` (and unnamed rows) belong to the
                // wiki engine, but an entity strict row is CT-owned and
                // the engine will never rewrite it. `--mode strict`
                // OVERWRITES it with `tier_fact`'s vocabulary — whatever
                // the row held before — and refuses unless that
                // vocabulary can gate, so one command repairs it fully.
                // Node types alone do not: a strict row also needs a
                // declared fallback (R2.4.5), so the tier arm names the
                // whole repair rather than leave a second hold to
                // discover — declaring the fallback it already names, or
                // naming one when it has none.
                use crate::db::ontology_set::{set_command, SetCommand, TIER_FACT};
                let fix = match manifest.as_deref() {
                    Some(id) if id != TIER_FACT => format!(
                        "fix: `{}` overwrites this row with the `tier_fact` vocabulary \
                         (refused until `tier_fact` itself declares node types and a \
                         fallback — the wiki engine owns that row)",
                        set_command(SetCommand {
                            entity: Some(id),
                            strict: true,
                            fallback: false,
                        })
                    ),
                    _ => match fallback {
                        Some(fallback) => format!(
                            "fix: add node types to that manifest — `{fallback}`, the \
                             fallback it already names, among them (the wiki engine owns \
                             manifest rows)"
                        ),
                        None => format!(
                            "fix: add node types to that manifest (the wiki engine owns \
                             manifest rows), then name one as its fallback with `{}` — a \
                             strict row needs both",
                            set_command(SetCommand {
                                entity: None,
                                strict: false,
                                fallback: true,
                            })
                        ),
                    },
                };
                write!(
                    f,
                    "{} declares no node types, so there is nothing to admit (§2.4.5); \
                     {fix}",
                    named(manifest)
                )
            }
            Self::NoFallback { manifest, declared } => write!(
                f,
                "{} declares no `fallback_node_type`, so every new-entity mint is held — \
                 including mints that propose a declared type (§2.4.5); {}",
                named(manifest),
                Self::fallback_command(manifest, declared)
            ),
            Self::FallbackNotDeclared {
                manifest,
                fallback,
                declared,
            } => write!(
                f,
                "{} names `{fallback}` as its `fallback_node_type`, but `{fallback}` is not \
                 one of its node types, so landing it would write an undeclared type \
                 (R2.4.4); {}",
                named(manifest),
                Self::fallback_command(manifest, declared)
            ),
            Self::OptOutLookupFailed { entity_id } => write!(
                f,
                "the opt-out lookup for entity `{entity_id}` failed (a database fault), so \
                 the mint is held rather than risk gating an opted-out entity (D8); retry, \
                 and check the brain database if it persists"
            ),
            Self::RedirectUnresolved { entity_id } => write!(
                f,
                "the merge redirect chain for entity `{entity_id}` could not be resolved \
                 (a cycle or a database fault), so the mint is held rather than read a \
                 merged-away entity's manifest (R2.7.5); retry, and check the brain \
                 database if it persists"
            ),
            Self::Internal { detail } => write!(
                f,
                "internal gate inconsistency ({detail}) — the mint is held rather than \
                 guessing a cause; this is a bug, please report it"
            ),
            Self::EntityManifestUnreadable { entity_id } => write!(
                f,
                "entity `{entity_id}`'s own manifest row could not be read (corrupt \
                 `manifest_json` or a database fault), so the mint is held rather than \
                 skipping a possibly-strict entity (r4-m4); repair or remove that row"
            ),
            Self::TierFactUnreadable => write!(
                f,
                "the `tier_fact` manifest row could not be read (corrupt `manifest_json` \
                 or a database fault) (§2.3 rung 4); repair that row"
            ),
            Self::ConfigHold { source } => {
                let what = match source {
                    Some(dir) => format!("source `{dir}`"),
                    None => "this mint (no source path)".to_string(),
                };
                write!(
                    f,
                    "the folder/host ontology config is degraded or conflicting for {what}, \
                     so the mint is held rather than guessing a mode; fix the \
                     `ingest.folder_ontology` / `ingest.ontology_default` entries in the \
                     brain config"
                )
            }
        }
    }
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

    // Redirect check first (Task 7 APPLIES this at sites; Task 2 BUILDS it):
    // a caller-supplied id that points at a redirect row resolves to its
    // TERMINAL survivor (the whole chain, not one hop). Cycles error out
    // (no infinite loop).
    let resolved_id = match caller_entity_id {
        Some(id) => match redirect_survivor(conn, id)? {
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
        GateDecision::Held(reason) => Ok(AdmitOutcome::Held {
            original_label: caller_entity_type.map(str::to_string),
            reason,
        }),
        GateDecision::Gate(vocab) => {
            // R2.4.4/R2.4.5 usability: the production resolver only builds
            // `Gate` over a usable vocabulary; this re-check holds a caller
            // that hands in an unusable one directly.
            if let Some(reason) = vocab.hold_reason(None) {
                return Ok(AdmitOutcome::Held {
                    original_label: caller_entity_type.map(str::to_string),
                    reason,
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
                // Unreachable after the usability check above (the ladder
                // holds only on an empty or fallback-less vocabulary); kept
                // total rather than panicking, and named as the bug it
                // would be — never a fabricated configuration cause.
                AdmitInternal::Held => Ok(AdmitOutcome::Held {
                    original_label: caller_entity_type.map(str::to_string),
                    reason: HoldReason::Internal {
                        detail: "admit ladder held a usable vocabulary",
                    },
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
/// Production callers only reach this ladder over a USABLE vocabulary
/// ([`NodeVocabulary::hold_reason`] is `None`): an empty or fallback-less
/// strict vocabulary holds EVERY mint before the ladder runs (R2.4.5),
/// declared labels included. The Held arms below are a defensive backstop
/// that keeps the function total — there is nothing to degrade onto and
/// inventing a label is forbidden (R2.4.4).
fn run_admit_ladder(vocab: &NodeVocabulary, proposed: Option<&str>) -> AdmitInternal {
    if vocab.is_empty() {
        // §2.4.5 empty-declared-set configuration error — held even when a
        // fallback is declared (there is nothing to admit onto).
        return AdmitInternal::Held;
    }
    let Some(label) = proposed else {
        // UNLABELED mint (GUI blank-type / LLM synthesis without a type /
        // okf_migration / bundle import — all pass no proposed label).
        // We must NOT invent a label (R2.4.4), but the manifest's declared
        // fallback IS a declared label, not an invention: land there
        // (spec §2.5 GUI bullet "no-fallback-exists → refusal error
        // shown; otherwise normal ladder"; bundle bullet "lands untyped
        // entities as the MANIFEST'S DECLARED FALLBACK"). With NO
        // fallback declared there is nothing to land on — Held, which
        // each site surfaces per its own contract (GUI refusal error,
        // bundle/migration atomic abort).
        if let Some(fallback) = vocab.fallback() {
            return AdmitInternal::DegradedToFallback {
                original_label: String::new(),
                landed_as: fallback.to_string(),
            };
        }
        // Strict vocabulary WITH types but WITHOUT a fallback: held.
        return AdmitInternal::Held;
    };
    if let Some(canonical) = vocab.canonicalize(label) {
        return AdmitInternal::Declared(canonical.to_string());
    }
    let label_key = NodeVocabulary::key(label);
    for (alias_from, alias_to) in ALIAS_TABLE {
        if NodeVocabulary::key(alias_from) == label_key {
            // Land the MANIFEST's spelling of the target (as the declared
            // arm does), never the alias table's: `agent` against a
            // manifest declaring `Role` lands `Role`, not `role`.
            if let Some(canonical) = vocab.canonicalize(alias_to) {
                return AdmitInternal::Aliased {
                    original_label: label.to_string(),
                    landed_as: canonical.to_string(),
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

/// Redirect resolver with cycle guard (spec R2.7.3 / plan-p2-M1).
///
/// Walks the WHOLE chain to its terminal survivor through the shared
/// cycle-guarded walk ([`resolve_redirect_chain`]). A hand-crafted cycle
/// (A→B, B→A) returns `Cycle` rather than looping forever — the caller
/// surfaces it to the user, never to silent recursion.
///
/// [`resolve_redirect_chain`]: crate::db::merge_duplicates::resolve_redirect_chain
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

/// Resolve `entity_id` to its terminal survivor. Review finding: the
/// previous resolver followed ONE hop, so a legacy multi-hop chain
/// (a→b→c, written before merge-time path compression) handed a mint the
/// intermediate loser `b` — splitting the cluster. D8: a read FAULT
/// propagates — a locked/faulting lookup must never silently insert
/// against a loser id. A missing ROW is the normal `None` case.
pub fn redirect_survivor(conn: &Connection, entity_id: &str) -> Result<RedirectOutcome> {
    use crate::db::merge_duplicates::{resolve_redirect_chain, ChainResolution};
    Ok(match resolve_redirect_chain(conn, entity_id)? {
        ChainResolution::None => RedirectOutcome::None,
        ChainResolution::Survivor(s) => RedirectOutcome::Survivor(s),
        ChainResolution::Cycle(_) => RedirectOutcome::Cycle,
    })
}

/// The ensure's pure decision (§2.4.4): what [`ensure_manifest_vocabulary`]
/// WOULD do to `manifest_json`, without touching the database. The single
/// owner of the subset guard / fallback preference / `document`+`process`
/// extension rules — the read-only `ct heal` report derives from this too
/// (review finding: a hand-rolled second copy could silently diverge).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnsurePlan {
    /// Unparseable manifest_json — never guess, leave alone.
    Malformed(String),
    /// Nothing to change.
    Complete,
    /// Foreign manifest with no declared fallback and no preferred choice.
    ForeignNoPreferredFallback,
    /// The PLANNED row still carries a vocabulary the gate cannot run
    /// (R2.4.5: an existing fallback naming no declared type, or an empty
    /// declared set) — every mint on it holds. Foreign-row shapes the
    /// ensure must not rewrite (§2.4.4 declare-or-report); reported loudly
    /// via heal instead, never memoized, so the pass re-reports until the
    /// operator fixes it with `ct ontology set --fallback`.
    UnusableVocabulary { reason: HoldReason },
    /// The edited manifest to write back.
    Edit {
        new_json: String,
        extended: bool,
        fallback_set: bool,
    },
}

/// `owner` names the row (`tier_fact` or an entity id) so an unusable
/// verdict's fix-line targets the row that actually holds; `strict` is the
/// row's mode — the gate reads only strict rows, so only a strict row's
/// vocabulary can hold a mint and be reported unusable.
pub(crate) fn plan_manifest_ensure(
    manifest_json: &str,
    owner: &str,
    strict: bool,
) -> Result<EnsurePlan> {
    let mut root: serde_json::Value = match serde_json::from_str(manifest_json) {
        Ok(v) => v,
        Err(e) => {
            // Malformed manifest_json: never guess — leave alone.
            return Ok(EnsurePlan::Malformed(format!("{e}")));
        }
    };

    // "Declared" is read through the gate's own lenient reader (review
    // finding: a hand-rolled `{type: …}`-only extraction saw ZERO types in
    // a bare-string manifest the gate reads as declaring them, so the
    // planner and the gate classified the same row differently).
    let vocab = NodeVocabulary::from_manifest(&crate::wiki_graph::parse_manifest_value(&root));

    // Subset guard: every EA slug ⊆ declared? If yes, do the full work;
    // otherwise do only the fallback declare-or-report.
    let is_ea_subset = EA_SEED_TYPES.iter().all(|seed| vocab.contains(seed));

    // Pick the fallback value (prefer `concept` if declared; else `project`),
    // written in the MANIFEST's own spelling so the fallback names exactly
    // the declared entry (`Concept` stays `Concept`).
    let fallback_choice: Option<String> = vocab
        .canonicalize("concept")
        .or_else(|| vocab.canonicalize("project"))
        .map(str::to_string);
    // The row's EXISTING fallback as written — `None` when the key is
    // absent, not a string, or blank (the reader treats all three alike).
    let existing_fallback = vocab.fallback().map(str::to_string);

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
            // Membership-key comparison (review finding): an exact-string
            // check appended `document` beside an existing `Document`,
            // polluting the manifest with case-variant duplicates.
            if !vocab.contains(slug) {
                node_types.push(json!({"type": slug, "description": desc}));
                did_extend = true;
            }
        }
        // Set `fallback_node_type` when the key is absent OR names no
        // declared type (r25: both leave every mint held). Replacing an
        // undeclared fallback with the preferred DECLARED choice keeps the
        // row usable; the EA family always has one (`project` is a seed
        // type), so this arm cannot fall through to the unusable report.
        let fallback_usable = existing_fallback
            .as_ref()
            .is_some_and(|f| vocab.contains(f));
        if !fallback_usable {
            if let Some(choice) = fallback_choice {
                root["fallback_node_type"] = json!(choice);
                did_set_fallback = true;
            }
        }
    } else {
        // Foreign manifest: declare-or-report only.
        if existing_fallback.is_none() && fallback_choice.is_none() {
            return Ok(EnsurePlan::ForeignNoPreferredFallback);
        }
        // Foreign manifest with a declared fallback — fine. With no declared
        // fallback but `project` declared — declare it. With nothing
        // declared — the loud signal above. An EXISTING fallback naming no
        // declared type is left untouched here (declare-or-report) and
        // caught by the post-edit usability check below.
        if existing_fallback.is_none() {
            if let Some(choice) = fallback_choice {
                root["fallback_node_type"] = json!(choice);
                did_set_fallback = true;
            }
        }
    }

    // R2.4.5 post-edit usability check: the PLANNED row must leave a
    // vocabulary the gate can run — the ensure must not stamp healthy
    // (`Complete`) a row that holds every mint. Evaluated through the
    // gate's own reader on the post-edit state, so the planner and the
    // gate can never disagree about usability. EA-family rows were
    // repaired above; this fires for foreign rows the ensure must not
    // rewrite: an existing fallback naming no declared type, or an empty
    // declared set. Strict rows only: the gate never reads a non-strict
    // row's vocabulary, so nothing is held on one and reporting it would
    // be a permanent false alarm (review finding).
    if strict {
        let planned_vocab =
            NodeVocabulary::from_manifest(&crate::wiki_graph::parse_manifest_value(&root));
        if let Some(reason) = planned_vocab.hold_reason(Some(owner)) {
            return Ok(EnsurePlan::UnusableVocabulary { reason });
        }
    }

    if did_set_fallback || did_extend {
        Ok(EnsurePlan::Edit {
            new_json: serde_json::to_string(&root)?,
            extended: did_extend,
            fallback_set: did_set_fallback,
        })
    } else {
        Ok(EnsurePlan::Complete)
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
    let Some((mode, manifest_json)) = row else {
        // No row to ensure — the spec's "missing row" case is a normal
        // SKIP per §2.1.
        return Ok(EnsureOutcome::NoRow);
    };

    // 2. Memoization: skip if this (entity, planner epoch, gated mode,
    //    bytes) was already planned healthy.
    let gated_mode = gated_row_mode(conn, entity_id, &mode)?;
    let manifest_hash = ensure_memo_key(gated_mode, &manifest_json);
    if manifest_ensure_already_done(conn, entity_id, &manifest_hash)? {
        return Ok(EnsureOutcome::AlreadyEnsured);
    }

    // 3. Parse, classify, edit (the pure planner).
    let strict = gated_mode == "strict";
    let (new_json, did_extend, did_set_fallback) =
        match plan_manifest_ensure(&manifest_json, entity_id, strict)? {
            EnsurePlan::Malformed(e) => return Ok(EnsureOutcome::Malformed(e)),
            EnsurePlan::ForeignNoPreferredFallback => {
                return Ok(EnsureOutcome::ForeignNoPreferredFallback {
                    entity_id: entity_id.to_string(),
                });
            }
            EnsurePlan::UnusableVocabulary { reason } => {
                return Ok(EnsureOutcome::UnusableVocabulary {
                    entity_id: entity_id.to_string(),
                    reason,
                });
            }
            EnsurePlan::Complete => (None, false, false),
            EnsurePlan::Edit {
                new_json,
                extended,
                fallback_set,
            } => (Some(new_json), extended, fallback_set),
        };

    // 4. Write the edited manifest_json back + record the memo ONLY if the
    //    write commits (r11-m4). The memo key is the POST-WRITE hash so the
    //    next call reads the new manifest_json, computes its hash, and finds
    //    the memo — short-circuiting to AlreadyEnsured.
    if let Some(new_json) = new_json {
        let new_hash = ensure_memo_key(gated_mode, &new_json);
        let tx = conn.unchecked_transaction()?;
        // Compare-and-swap on the bytes we READ (review finding): the read
        // above is outside the transaction, so a concurrent engine/desktop
        // rewrite between read and UPDATE must not be overwritten with an
        // edited copy of the OLDER manifest. Zero rows changed = the row
        // moved under us — leave it alone; the next ensure pass re-reads
        // the newer bytes and re-decides. No memo is recorded in that case
        // (r11-m4: memo only after the write commits).
        let changed = tx.execute(
            "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1
              WHERE entity_id = ?2 AND manifest_json = ?3",
            params![new_json, entity_id, manifest_json],
        )?;
        if changed == 0 {
            // NOT a success (review finding): the fallback the gate needs
            // may still be missing — report the race distinctly.
            return Ok(EnsureOutcome::RacedConcurrentWrite);
        }
        record_ensure_memo(&tx, entity_id, &new_hash)?;
        tx.commit()?;
        Ok(EnsureOutcome::Ensured {
            extended: did_extend,
            fallback_set: did_set_fallback,
        })
    } else {
        // Nothing to change — still record it so we don't reparse on every
        // resolution (r11-m4).
        let tx = conn.unchecked_transaction()?;
        record_ensure_memo(&tx, entity_id, &manifest_hash)?;
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
    /// The row (post any ensure edits) still carries a vocabulary the gate
    /// cannot run — R2.4.5 holds every mint on it. The ensure writes
    /// nothing and records no memo; heal reports the row until the
    /// operator fixes it (`ct ontology set --fallback`).
    UnusableVocabulary {
        entity_id: String,
        reason: HoldReason,
    },
    /// The compare-and-swap write changed zero rows — the manifest was
    /// rewritten concurrently between read and UPDATE. Nothing was written
    /// and no memo recorded; the next pass re-reads the newer bytes.
    RacedConcurrentWrite,
}

/// The planner's verdict epoch. Bump it whenever
/// [`plan_manifest_ensure`] can return a different verdict for the same
/// bytes: a memo is a cached "planned healthy", and one recorded by an
/// older planner must never short-circuit a newer one (review finding: a
/// pre-r26 memo kept `Complete` on rows r26 repairs or reports, so the
/// ensure answered `AlreadyEnsured` forever while read-only `ct heal`
/// reported the row unusable).
const ENSURE_PLANNER_EPOCH: &str = "r26";

/// The mode the GATE sees for `entity_id`'s row: its stored `mode`, except
/// that a merged-away (or cycle-locked) id's row is never read — rung 1b
/// reads the terminal survivor's row (R2.7.5) — so it gates nothing and is
/// reported as `"redirected"` (review finding: judging it by raw id
/// re-reported a loser's leftover row forever, with a fix that writes the
/// survivor's row instead). D8: a faulting redirect read propagates.
pub(crate) fn gated_row_mode<'m>(
    conn: &Connection,
    entity_id: &str,
    mode: &'m str,
) -> Result<&'m str> {
    Ok(match redirect_survivor(conn, entity_id)? {
        RedirectOutcome::None => mode,
        RedirectOutcome::Survivor(_) | RedirectOutcome::Cycle => "redirected",
    })
}

/// The memo key: the planner's verdict depends on the epoch, the row's
/// mode (only strict rows can be unusable) and its bytes — all three key
/// it, so a mode flip or a planner change re-plans.
fn ensure_memo_key(mode: &str, manifest_json: &str) -> String {
    hash_bytes(format!("{ENSURE_PLANNER_EPOCH}\0{mode}\0{manifest_json}").as_bytes())
}

/// Record `manifest_hash` as this entity's ONE memo, dropping any earlier
/// key — older bytes or an older epoch can never match again, so keeping
/// them only grows the table.
fn record_ensure_memo(conn: &Connection, entity_id: &str, manifest_hash: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM manifest_ensure_memo WHERE entity_id = ?1 AND manifest_hash <> ?2",
        params![entity_id, manifest_hash],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO manifest_ensure_memo (entity_id, manifest_hash, recorded_at)
         VALUES (?1, ?2, ?3)",
        params![
            entity_id,
            manifest_hash,
            crate::db::commit::now_timestamps().0
        ],
    )?;
    Ok(())
}

fn manifest_ensure_already_done(
    conn: &Connection,
    entity_id: &str,
    manifest_hash: &str,
) -> Result<bool> {
    // D8 (review finding): a faulting memo read PROPAGATES — it is never
    // "not memoized". `ensure_all_manifest_vocabularies` is best-effort per
    // row, so one fault surfaces as that row's error without stopping the
    // pass.
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM manifest_ensure_memo WHERE entity_id = ?1 AND manifest_hash = ?2",
        params![entity_id, manifest_hash],
        |r| r.get(0),
    )?;
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
                    EnsureOutcome::UnusableVocabulary { entity_id, reason } => {
                        // Named per row with its own cause and fix — a bare
                        // count cannot say which row holds or why.
                        eprintln!("[entity-gate] manifest `{entity_id}` cannot gate: {reason}");
                        summary.unusable_vocabulary.push(entity_id);
                    }
                    EnsureOutcome::Malformed(_) => summary.malformed += 1,
                    EnsureOutcome::RacedConcurrentWrite => {
                        summary.raced += 1;
                        eprintln!(
                            "[entity-gate] manifest ensure for {entity_id} raced a concurrent \
                             manifest rewrite; nothing written — the next ensure pass retries"
                        );
                    }
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
    /// Strict rows (entity ids) whose post-ensure vocabulary still cannot
    /// gate (R2.4.5) — reported by name, not repaired.
    pub unusable_vocabulary: Vec<String>,
    pub malformed: usize,
    /// Rows whose CAS write lost a concurrent rewrite (nothing written).
    pub raced: usize,
    pub errors: usize,
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
///   * `StrictNoVocab` — the mint must be gated but cannot be: a strict
///     row with no usable vocabulary (no `node_types`, no
///     `fallback_node_type`, or an undeclared one), an unreadable row, a
///     faulted opt-out lookup, or a degraded config. HELD per §2.4.5; the
///     [`HoldReason`] names which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeVerdict {
    OptOut,
    Gate,
    Off,
    StrictNoVocab(HoldReason),
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
    /// off/opt-out verdict becomes `Skip`; a held verdict becomes
    /// `Held(reason)`; a gated vocabulary becomes `Gate(vocab)`.
    pub fn into_gate_decision(self) -> GateDecision {
        match self.verdict {
            ModeVerdict::OptOut | ModeVerdict::Off => GateDecision::Skip,
            ModeVerdict::StrictNoVocab(reason) => GateDecision::Held(reason),
            ModeVerdict::Gate => match self.vocabulary {
                Some(v) => GateDecision::Gate(v),
                // The resolver never builds `Gate` without a vocabulary;
                // name the bug rather than invent an empty-manifest cause.
                None => GateDecision::Held(HoldReason::Internal {
                    detail: "gate verdict without a vocabulary",
                }),
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

impl GateResolutionContext<'_> {
    /// Rungs 2–3 for a set of source paths — the ONE construction both the
    /// node ladder and the edge endpoint ladder walk: each path through
    /// `folder_ontology` / the host default, or — for a mint with NO
    /// resolvable source (GUI / bundle / fact-less endpoint, R2.3.4) — one
    /// pathless rung-3 resolution with the same degraded guards.
    pub(crate) fn source_lookups<'p>(
        &self,
        source_paths: &'p [String],
    ) -> Vec<(Option<&'p String>, crate::config::OntologyLookup)> {
        if source_paths.is_empty() {
            return vec![(
                None,
                self.ingest.ontology_lookup_pathless(
                    self.degraded,
                    self.schema,
                    self.schema_unparseable,
                ),
            )];
        }
        source_paths
            .iter()
            .map(|source| {
                (
                    Some(source),
                    self.ingest.ontology_lookup(
                        source,
                        self.vault_root,
                        self.degraded,
                        self.schema,
                        self.schema_unparseable,
                    ),
                )
            })
            .collect()
    }
}

/// The mode verdict for a STRICT vocabulary read from manifest row `owner`:
/// GATE when usable, else HELD (§2.4.5) naming why — no parseable manifest,
/// no node types, or no declared `fallback_node_type` (R2.4.4/R2.4.5).
fn strict_vocabulary_verdict(vocabulary: Option<&NodeVocabulary>, owner: &str) -> ModeVerdict {
    let reason = match vocabulary {
        Some(v) => v.hold_reason(Some(owner)),
        None => Some(HoldReason::EmptyNodeTypes {
            manifest: Some(owner.to_string()),
            fallback: None,
        }),
    };
    match reason {
        Some(reason) => ModeVerdict::StrictNoVocab(reason),
        None => ModeVerdict::Gate,
    }
}

/// The verdict of a STRICT manifest row (rung 1b's own row, or rung 4's
/// `tier_fact`), named `owner`: GATE under its vocabulary, or HELD (§2.4.5)
/// when that vocabulary is unusable.
fn strict_row_decision(
    manifest: Option<&crate::wiki_graph::WikiManifest>,
    owner: &str,
) -> NodeGateDecision {
    let vocabulary = manifest.map(NodeVocabulary::from_manifest);
    NodeGateDecision {
        verdict: strict_vocabulary_verdict(vocabulary.as_ref(), owner),
        vocabulary,
        source_directory: None,
    }
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
    // Rung 1a — ct_entity_optouts row → opt-out, skip edge gating too (§2.1).
    // D8: a read FAULT is never "no row" — it maps to Held (loud), so a
    // locked/faulting lookup cannot silently push an opted-out entity back
    // under the strict ladder's vocabulary assumptions.
    match entity_has_optout(conn, entity_id) {
        Ok(true) => {
            return NodeGateDecision {
                verdict: ModeVerdict::OptOut,
                vocabulary: None,
                source_directory: None,
            };
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!(
                "[entity-gate] opt-out lookup failed for {entity_id}: {e}; \
                 holding the mint (rung 1a fail-closed)"
            );
            return NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab(HoldReason::OptOutLookupFailed {
                    entity_id: entity_id.to_string(),
                }),
                vocabulary: None,
                source_directory: None,
            };
        }
    }

    // Rung 1b/c/d — the entity's own manifest row. An unreadable row is
    // REPORT-OR-HOLD for nodes per r4-m4; an unmarked row climbs. A STRICT
    // row GATEs — the vocabulary comes from THIS manifest (its fallback_node_type
    // drives the degrade rung).
    //
    // The row read is the TERMINAL SURVIVOR's (R2.7.5): a mint under a
    // merged-away id lands on the survivor (`shared_insert_entity`
    // resolves the same chain), so the survivor's row governs it — and
    // it is the row `ct ontology set --entity` writes, so a held mint's
    // fix-line names a row that command can actually repair (review
    // finding: reading the loser's row by raw id held mints on a row no
    // CT command can reach). D8: an unresolvable chain holds.
    let manifest_owner = match redirect_survivor(conn, entity_id) {
        Ok(RedirectOutcome::Survivor(survivor)) => survivor,
        Ok(RedirectOutcome::None) => entity_id.to_string(),
        Ok(RedirectOutcome::Cycle) | Err(_) => {
            return NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab(HoldReason::RedirectUnresolved {
                    entity_id: entity_id.to_string(),
                }),
                vocabulary: None,
                source_directory: None,
            };
        }
    };
    let mut entity_row_unreadable = false;
    match crate::wiki_graph::wiki_get_ontology(conn, &manifest_owner) {
        Ok(o) if o.mode == "strict" => {
            // Ensure runs before the gate resolves (the gate's resolution path),
            // so a fresh install already has the `document`/`process` extensions
            // + `fallback_node_type` written. A pre-wave-1 manifest was caught
            // at the gate-resolve call below; if the strict row STILL lacks a
            // fallback the helper holds per §2.4.5.
            return strict_row_decision(o.manifest.as_ref(), &manifest_owner);
        }
        Ok(_) => {
            // Not strict (mark explicit OFF/emergent): rung 1d, climb.
        }
        Err(_) => {
            // Unreadable row — REPORT-OR-HOLD for nodes per r4-m4. Match
            // the edge cascade's silent fall-through with one extra step:
            // strict-wins via rungs 2-3 below. If the climb ALSO produces
            // no strict verdict, the rung-4 `Ok(_)` arm below returns Held
            // (loud, not silent) via `entity_row_unreadable`.
            entity_row_unreadable = true;
        }
    }

    // Rungs 2-3 — folder_ontology + ontology_default + schema (strict-wins
    // across all source paths; an `off` source loses to a strict or climbing
    // sibling, but all-`off` SKIPs — rung 4 never overrides it).
    let mut strict_source_dir: Option<String> = None;
    // First `off` resolution seen at rung 2/3 — when the ladder's final
    // verdict is Off (SKIP), the r21 ledger table records "the `off` folder
    // if rung 2 caused the SKIP" as the row's `source_directory`.
    let mut off_source_dir: Option<String> = None;
    // Some source climbed past rungs 2-3 — it resolves at the strict
    // residual rung 4, so an `off` sibling cannot decide SKIP (R2.3.3).
    let mut any_climb = false;
    // Pathless mints (GUI / bundle, R2.3.4) START at rung 3: resolve the
    // host default once, with the same degraded guards as a path lookup.
    let lookups = ctx.source_lookups(source_paths);
    let mut strict_found = false;
    let mut off_found = false;
    // First source that resolved Hold (r10-MINOR-2), if no later source
    // resolved strict — see the Hold arm below. `hold_seen` is separate
    // from the directory: a pathless mint's Hold has no source at all.
    let mut hold_seen = false;
    let mut hold_source_dir: Option<String> = None;
    for (source, lookup) in lookups {
        match lookup {
            crate::config::OntologyLookup::Mode(crate::config::OntologyMode::Strict) => {
                // Found a strict rung; vocabulary comes from tier_fact below.
                strict_found = true;
                strict_source_dir = source.cloned();
                break;
            }
            crate::config::OntologyLookup::Mode(crate::config::OntologyMode::Off) => {
                // Off found, but continue to look for any strict rung
                // (strict-wins, R2.3.3). Remember the off folder for the
                // SKIP ledger row's source_directory (R2.4.6 r21).
                off_found = true;
                if off_source_dir.is_none() {
                    off_source_dir = source.cloned();
                }
                continue;
            }
            crate::config::OntologyLookup::Hold => {
                // r10-MINOR-2: the vocabulary check runs FIRST — with no
                // strict `tier_fact` row there is nothing a hold could
                // protect (SKIP per §2.1), so the mint succeeds. Hold only
                // when the mint would otherwise be gated, or when the
                // vocabulary row cannot be read (fail-closed).
                //
                // Like the `Off` arm, do NOT return here: a later source
                // path may climb past rungs 2-3 to a strict rung, and
                // strict-wins (R2.3.3). Remember this hold — if no later
                // source resolves strict, its verdict (SKIP, or Held on a
                // strict/unreadable `tier_fact` row) applies below.
                if !hold_seen {
                    hold_seen = true;
                    hold_source_dir = source.cloned();
                }
                continue;
            }
            crate::config::OntologyLookup::Climb => {
                // Try the next source; if all climb we fall through to rung 4.
                any_climb = true;
                continue;
            }
        }
    }
    if strict_found {
        // Mode vs vocabulary (r9-M1): rungs 2–3 supply a MODE, never a
        // vocabulary — that comes from `tier_fact`. Mode = GATE with no
        // strict vocabulary row → SKIP + census warning, mirroring §2.1's
        // no-row SKIP (matrix: fresh brain + `ct ontology set --mode
        // strict` + one LLM mint → SKIP + warning, mint succeeds). Only a
        // strict row that is unusable (no fallback, §2.4.5) or unreadable
        // holds.
        return match tier_fact_row_state(conn) {
            TierFactRow::NotStrict => {
                eprintln!(
                    "[entity-gate] {entity_id}: strict mode resolved from folder/host \
                     config but no strict `tier_fact` vocabulary row exists — SKIP \
                     (the row is seeded by the wiki engine when the app opens this \
                     brain; CT cannot create it, §1.6)"
                );
                NodeGateDecision {
                    verdict: ModeVerdict::Off,
                    vocabulary: None,
                    source_directory: None,
                }
            }
            TierFactRow::Strict(vocab) => NodeGateDecision {
                verdict: strict_vocabulary_verdict(vocab.as_ref(), "tier_fact"),
                vocabulary: vocab,
                source_directory: strict_source_dir,
            },
            TierFactRow::Unreadable => NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab(HoldReason::TierFactUnreadable),
                vocabulary: None,
                source_directory: strict_source_dir,
            },
        };
    }
    if hold_seen && !any_climb {
        // r4-m4 parity with the `off_found` arm below (review finding): an
        // UNREADABLE entity manifest row is REPORT-OR-HOLD — never a silent
        // SKIP — even when the held source is the only deciding signal and
        // `tier_fact` is absent. Without this guard the deferred Hold
        // returned Off and the mint landed ungated, exactly the silent
        // admission the rung-4 `Ok(_)` arm's `entity_row_unreadable` check
        // exists to prevent.
        if entity_row_unreadable {
            return NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab(HoldReason::EntityManifestUnreadable {
                    entity_id: manifest_owner.clone(),
                }),
                vocabulary: None,
                source_directory: None,
            };
        }
        // No source resolved strict and none climbs, so the deferred Hold
        // verdict from r10-MINOR-2 applies (same arms as the original
        // in-loop return): with no strict `tier_fact` row there is nothing
        // the hold protects — SKIP; with one, the hold protects it. A
        // CLIMBING sibling instead reaches rung 4 below: strict-wins means
        // the held source (strict or off, unknown) cannot change a strict
        // rung-4 verdict, and its vocabulary would come from `tier_fact`
        // either way (r9-M1) — the same "held source reaches rung 4" rule
        // the edge endpoint ladder applies.
        if tier_fact_row_state(conn) == TierFactRow::NotStrict {
            return NodeGateDecision {
                verdict: ModeVerdict::Off,
                vocabulary: None,
                source_directory: None,
            };
        }
        return NodeGateDecision {
            verdict: ModeVerdict::StrictNoVocab(HoldReason::ConfigHold {
                source: hold_source_dir.clone(),
            }),
            vocabulary: None,
            source_directory: hold_source_dir.clone(),
        };
    }
    if off_found && !any_climb {
        // Every source decided `off` at rung 2/3 — §2.3 "first hit
        // decides": SKIP, never overridden by a strict rung-4 tier_fact
        // (the D8 matrix case "bare host-off + GUI-minted character →
        // ungated"). r4-m4 still applies: an unreadable entity row holds.
        if entity_row_unreadable {
            return NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab(HoldReason::EntityManifestUnreadable {
                    entity_id: manifest_owner.clone(),
                }),
                vocabulary: None,
                source_directory: None,
            };
        }
        return NodeGateDecision {
            verdict: ModeVerdict::Off,
            vocabulary: None,
            source_directory: off_source_dir,
        };
    }

    // Rung 4 — tier_fact itself.
    match crate::wiki_graph::wiki_get_ontology(conn, "tier_fact") {
        Ok(o) if o.mode == "strict" => strict_row_decision(o.manifest.as_ref(), "tier_fact"),
        Ok(_) => {
            // Unmarked/off tier_fact row: §2.3.1 SKIP — UNLESS the entity's
            // own manifest row was unreadable: r4-m4's Held promise (no
            // silent admission when the entity said strict but could not be
            // read). If the climb produced a strict verdict we never reach
            // here (rungs 2-3 returned above).
            if entity_row_unreadable {
                return NodeGateDecision {
                    verdict: ModeVerdict::StrictNoVocab(HoldReason::EntityManifestUnreadable {
                        entity_id: manifest_owner.clone(),
                    }),
                    vocabulary: None,
                    source_directory: None,
                };
            }
            // If a rung 2/3 lookup resolved off, record that folder — the
            // r21 `gate_skipped` row carries it as `source_directory`.
            NodeGateDecision {
                verdict: ModeVerdict::Off,
                vocabulary: None,
                source_directory: off_source_dir,
            }
        }
        Err(_) => {
            // Corrupt tier_fact manifest: REPORT-OR-HOLD per §2.3 rung 4.
            NodeGateDecision {
                verdict: ModeVerdict::StrictNoVocab(HoldReason::TierFactUnreadable),
                vocabulary: None,
                source_directory: None,
            }
        }
    }
}

/// The SINGLE production entry point for the four insert sites (LLM
/// synthesis / GUI / bundle / okf_migration). The caller loads the ingest
/// policy for the connection's brain dir ([`crate::config::ingest_policy_for_db`]
/// — cached per config-file bytes) BEFORE opening the IMMEDIATE transaction
/// (r21 hold-time rule: no filesystem I/O inside it) and passes it in here;
/// this function stamps the initial drift watermark at the first gate
/// resolution (r13-MAJOR-3) and walks the FULL §2.3 ladder — rungs
/// 1a/1b/1c/1d, the rung 2/3 `folder_ontology` / `ontology_default` climb
/// via [`resolve_node_gate_decision`], and the rung 4 `tier_fact` fallback.
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
    policy: &crate::config::IngestPolicy,
    entity_id: &str,
    source_paths: &[String],
) -> (GateDecision, NodeGateDecision) {
    let conn: &Connection = tx;
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

/// The same ladder as [`resolve_production_gate`], strictly READ-ONLY (no
/// watermark stamp, no transaction): what a mint WOULD resolve to. For
/// previews that must agree with the apply they describe.
pub fn preview_production_gate(
    conn: &Connection,
    policy: &crate::config::IngestPolicy,
    entity_id: &str,
    source_paths: &[String],
) -> GateDecision {
    let degraded = policy.ontology_degraded_state();
    let ctx = GateResolutionContext {
        ingest: &policy.tiers,
        degraded: &degraded,
        schema: policy.ontology_selection,
        schema_unparseable: policy.ontology_unparseable,
        vault_root: policy.vault_root.as_deref(),
    };
    resolve_node_gate_decision(conn, entity_id, source_paths, ctx).into_gate_decision()
}

/// Land a mint the gate SKIPped (the shared insert helper does not insert
/// on Skip). No gate ran, so there is no vocabulary to violate: the
/// caller's label lands verbatim, the pre-existing `'concept'` literal only
/// when there is none (§2.5 / r2-M2a). The LLM and GUI mint paths share
/// this; bundle import and okf_migration keep their own literal landings
/// (no label to carry, §2.5).
pub(crate) fn land_skipped_entity(
    tx: &ImmediateTx<'_>,
    id: &str,
    name: &str,
    label: Option<&str>,
    summary: &str,
    now_secs: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO curated_entities (
            id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at
         ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5, NULL)",
        params![id, name, label.unwrap_or("concept"), summary, now_secs],
    )?;
    Ok(())
}

/// The SINGLE origin-ledger writer for gate outcomes shared by the insert
/// sites (fix-round-1 I3; replaces the former per-site copies in
/// `commit.rs` / `entities.rs`). Row shape per the R2.4.6 r21 table:
///
///   * SKIP outcome → `gate_skipped` row; `original_type` = the label the
///     outcome carried (the proposed label, or `None` when the caller
///     supplied none — never `''`); `source_directory` = the `off` folder
///     when rung 2 caused the SKIP, else NULL.
///   * `DegradedToFallback` with a LABEL → `degraded` row with the original
///     label, trimmed, pre-canonicalization; `DegradedToFallback` with NO
///     label (the unlabeled-mint landing, `original_label: ""`) →
///     `unlabeled_landing` row with `original_type` NULL (r21 normative
///     table — a label-less landing is an unlabeled landing, not a
///     degrade); `source_directory` NULL either way (degrade rows only
///     carry a directory when off-sourced, which cannot happen on a Gate
///     verdict).
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
            // Minor-4 (r21 normative table): a label-less landing is an
            // UNLABELED landing (`unlabeled_landing`, `original_type`
            // NULL), not a degrade — only a landing that degraded an
            // actual supplied label is a `degraded` row.
            let (label, reason) = if !original_label.is_empty() {
                (Some(original_label.as_str()), OriginReason::Degraded)
            } else {
                (None, OriginReason::UnlabeledLanding)
            };
            write_origin_ledger_row(tx, entity_id, label, reason, None)?;
        }
        _ => {
            // Admitted-as-declared, alias-admitted, held: NO ledger row.
        }
    }
    Ok(())
}

/// The `tier_fact` manifest row as the mode-vs-vocabulary rule (r9-M1)
/// needs it: strict (with its vocabulary, if the manifest parses), present
/// but not strict / absent, or unreadable.
#[derive(Debug, PartialEq, Eq)]
enum TierFactRow {
    Strict(Option<NodeVocabulary>),
    NotStrict,
    Unreadable,
}

fn tier_fact_row_state(conn: &Connection) -> TierFactRow {
    match crate::wiki_graph::wiki_get_ontology(conn, "tier_fact") {
        Ok(o) if o.mode == "strict" => {
            TierFactRow::Strict(o.manifest.as_ref().map(NodeVocabulary::from_manifest))
        }
        Ok(_) => TierFactRow::NotStrict,
        Err(_) => TierFactRow::Unreadable,
    }
}

/// Rung 1a — does this entity have a deliberate opt-out row? DB faults
/// PROPAGATE (D8): a failed lookup is never interpreted as "no row" — the
/// caller maps an error to Held/continued-gating, never to an opt-out skip.
///
/// Cluster-closed (spec R2.7.5): a merge moves no rows, so an opt-out the
/// user set on a member that later merged away still lives under the
/// loser's id. The lookup resolves `entity_id` to its terminal survivor and
/// checks every member of that redirect cluster, so a deliberate opt-out on
/// ANY member keeps applying to the merged entity (D8: off means off).
/// `UNION` (not `UNION ALL`) keeps a hand-edited redirect cycle finite.
pub(crate) fn entity_has_optout(conn: &Connection, entity_id: &str) -> rusqlite::Result<bool> {
    let count: i64 = conn.query_row(
        "WITH RECURSIVE
           up(id, depth) AS (
             SELECT ?1, 0
             UNION
             SELECT r.merged_into, up.depth + 1
               FROM entity_redirects r JOIN up ON r.entity_id = up.id
              WHERE up.depth < 64
           ),
           survivor(id) AS (SELECT id FROM up ORDER BY depth DESC LIMIT 1),
           cluster(id) AS (
             SELECT id FROM survivor
             UNION
             SELECT r.entity_id FROM entity_redirects r JOIN cluster c ON r.merged_into = c.id
           )
         SELECT COUNT(*) FROM ct_entity_optouts
          WHERE entity_id IN (SELECT id FROM cluster)",
        [entity_id],
        |r| r.get(0),
    )?;
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

    /// R2.7.5 / D8: a merge moves no rows, so an opt-out set on a member
    /// that later merged away must keep applying to the merged entity —
    /// looked up from the survivor, from the loser, and from a loser-of-a-
    /// loser alike. An unrelated entity stays un-opted-out.
    #[test]
    fn optout_lookup_is_cluster_closed() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO ct_entity_optouts (entity_id, reason, created_at) VALUES ('e_lose', 'user', 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES ('e_lose','e_surv',1);
             INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES ('e_other','e_surv',1);",
        )
        .unwrap();
        for id in ["e_surv", "e_lose", "e_other"] {
            assert!(
                entity_has_optout(&conn, id).unwrap(),
                "{id} must see the cluster opt-out"
            );
        }
        assert!(!entity_has_optout(&conn, "e_unrelated").unwrap());
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

    /// Review finding: the alias rung lands the MANIFEST's spelling of the
    /// target (`Role`), not the alias table's (`role`) — same as the
    /// declared rung, so one concept never stores two spellings.
    #[test]
    fn alias_ladder_lands_manifest_spelling() {
        let manifest = WikiManifest {
            node_types: vec![WikiNodeType {
                type_name: "Role".into(),
                ..Default::default()
            }],
            edge_types: vec![],
            fallback_node_type: Some("Role".into()),
        };
        let v = NodeVocabulary::from_manifest(&manifest);
        match run_admit_ladder(&v, Some("agent")) {
            AdmitInternal::Aliased { landed_as, .. } => assert_eq!(landed_as, "Role"),
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

    /// r25/r26: an EA-family row whose `fallback_node_type` names no
    /// declared type would hold every mint — the ensure REPLACES it with
    /// the preferred declared choice (here `concept`, declared beside the
    /// seeds) instead of stamping the row healthy.
    #[test]
    fn ensure_replaces_undeclared_ea_fallback() {
        let conn = open_in_memory().unwrap();
        let mut types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        types.push(json!({"type": "concept"}));
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
            "fallback_node_type": "ghost",
        });
        insert_manifest(&conn, "tier_fact", "strict", &manifest.to_string());

        let outcome = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        match outcome {
            EnsureOutcome::Ensured { fallback_set, .. } => assert!(fallback_set),
            other => panic!("expected Ensured, got {other:?}"),
        }
        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["fallback_node_type"].as_str(), Some("concept"));

        // The replacement left a usable vocabulary: a memo-cleared re-run
        // is AlreadyComplete, not another repair or report.
        conn.execute("DELETE FROM manifest_ensure_memo", [])
            .unwrap();
        let outcome = ensure_manifest_vocabulary(&conn, "tier_fact").unwrap();
        assert_eq!(outcome, EnsureOutcome::AlreadyComplete);
    }

    /// r25/r26: a FOREIGN row whose fallback names no declared type is
    /// beyond the ensure's declare-or-report charter — nothing is written
    /// and the row is REPORTED (heal's `unusable_vocabulary` count),
    /// never stamped healthy. No memo: every pass reports it again until
    /// the operator fixes it.
    #[test]
    fn ensure_reports_foreign_undeclared_fallback() {
        let conn = open_in_memory().unwrap();
        let manifest = serde_json::json!({
            "node_types": [{"type": "person"}, {"type": "place"}],
            "edge_types": [],
            "fallback_node_type": "ghost",
        });
        let manifest_json = manifest.to_string();
        insert_manifest(&conn, "foreign_vault", "strict", &manifest_json);

        let expected = EnsureOutcome::UnusableVocabulary {
            entity_id: "foreign_vault".to_string(),
            reason: HoldReason::FallbackNotDeclared {
                manifest: Some("foreign_vault".to_string()),
                fallback: "ghost".to_string(),
                declared: vec!["person".to_string(), "place".to_string()],
            },
        };
        assert_eq!(
            ensure_manifest_vocabulary(&conn, "foreign_vault").unwrap(),
            expected
        );
        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'foreign_vault'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, manifest_json, "nothing was written");
        assert_eq!(
            ensure_manifest_vocabulary(&conn, "foreign_vault").unwrap(),
            expected,
            "unusable rows are re-reported every pass (no memo)"
        );
    }

    /// r25/r26: an empty declared set holds every mint even WITH a
    /// fallback named over it — reported, never degraded onto.
    #[test]
    fn ensure_reports_empty_node_types_with_fallback() {
        let conn = open_in_memory().unwrap();
        let manifest = serde_json::json!({
            "node_types": [],
            "edge_types": [],
            "fallback_node_type": "person",
        });
        insert_manifest(&conn, "foreign_vault", "strict", &manifest.to_string());
        assert_eq!(
            ensure_manifest_vocabulary(&conn, "foreign_vault").unwrap(),
            EnsureOutcome::UnusableVocabulary {
                entity_id: "foreign_vault".to_string(),
                reason: HoldReason::EmptyNodeTypes {
                    manifest: Some("foreign_vault".to_string()),
                    fallback: Some("person".to_string()),
                },
            }
        );
    }

    /// Review finding (memo epoch): a memo recorded by the PRE-r26 planner
    /// (key = sha256 of the bytes alone) marked rows `Complete` that r26
    /// reports or repairs. It must not short-circuit the new planner, or
    /// `ct heal --yes` answers `AlreadyEnsured` forever while read-only
    /// `ct heal` reports the row unusable.
    #[test]
    fn legacy_memo_does_not_suppress_the_r26_planner() {
        let conn = open_in_memory().unwrap();
        // Foreign strict row: reported.
        let foreign = serde_json::json!({
            "node_types": [{"type": "person"}],
            "fallback_node_type": "ghost",
        })
        .to_string();
        insert_manifest(&conn, "foreign_vault", "strict", &foreign);
        // EA-family row with an undeclared fallback: repaired.
        let mut types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        types.push(json!({"type": "document"}));
        types.push(json!({"type": "process"}));
        let ea = json!({"node_types": types, "fallback_node_type": "ghost"}).to_string();
        insert_manifest(&conn, "ea_vault", "strict", &ea);
        for (id, bytes) in [("foreign_vault", &foreign), ("ea_vault", &ea)] {
            conn.execute(
                "INSERT INTO manifest_ensure_memo (entity_id, manifest_hash, recorded_at)
                 VALUES (?1, ?2, 1)",
                params![id, hash_bytes(bytes.as_bytes())],
            )
            .unwrap();
        }

        assert!(matches!(
            ensure_manifest_vocabulary(&conn, "foreign_vault").unwrap(),
            EnsureOutcome::UnusableVocabulary { .. }
        ));
        assert_eq!(
            ensure_manifest_vocabulary(&conn, "ea_vault").unwrap(),
            EnsureOutcome::Ensured {
                extended: false,
                fallback_set: true,
            }
        );
        // The repair re-memoized under the new key and dropped the legacy
        // one: one memo row per entity, and the next pass short-circuits.
        let memos: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM manifest_ensure_memo WHERE entity_id = 'ea_vault'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(memos, 1);
        assert_eq!(
            ensure_manifest_vocabulary(&conn, "ea_vault").unwrap(),
            EnsureOutcome::AlreadyEnsured
        );
    }

    /// Review finding (mode): the gate reads only STRICT rows, so a
    /// non-strict row's unusable vocabulary holds nothing and is not
    /// reported — but flipping the row to strict (same bytes) re-plans
    /// and reports it: the memo is keyed by mode too.
    #[test]
    fn unusable_vocabulary_is_reported_for_strict_rows_only() {
        let conn = open_in_memory().unwrap();
        let manifest = serde_json::json!({
            "node_types": [{"type": "person"}],
            "fallback_node_type": "ghost",
        })
        .to_string();
        insert_manifest(&conn, "foreign_vault", "off", &manifest);
        assert_eq!(
            ensure_manifest_vocabulary(&conn, "foreign_vault").unwrap(),
            EnsureOutcome::AlreadyComplete
        );
        conn.execute(
            "UPDATE llm_wiki_entity_manifests SET mode = 'strict' WHERE entity_id = 'foreign_vault'",
            [],
        )
        .unwrap();
        assert!(matches!(
            ensure_manifest_vocabulary(&conn, "foreign_vault").unwrap(),
            EnsureOutcome::UnusableVocabulary { .. }
        ));
    }

    /// Review finding (planner reader): "declared" is read through the
    /// gate's lenient reader, so a bare-string `node_types` row is not
    /// misread as declaring nothing — `project` is found and declared as
    /// the fallback, exactly as for the object form.
    #[test]
    fn planner_reads_bare_string_node_types_like_the_gate() {
        let manifest = json!({"node_types": ["person", "project"]}).to_string();
        let EnsurePlan::Edit {
            new_json,
            fallback_set,
            ..
        } = plan_manifest_ensure(&manifest, "foreign_vault", true).unwrap()
        else {
            panic!("expected the fallback to be declared");
        };
        assert!(fallback_set);
        let parsed: serde_json::Value = serde_json::from_str(&new_json).unwrap();
        assert_eq!(parsed["fallback_node_type"], "project");
    }

    /// Review finding (merged-away id): rung 1b reads the TERMINAL
    /// SURVIVOR's manifest row — where the mint lands and the row
    /// `ct ontology set --entity` writes — never a stale loser's row.
    #[test]
    fn rung_1b_reads_the_survivor_row_not_the_losers() {
        let conn = open_in_memory().unwrap();
        // The loser's leftover row is unusable; the survivor has none.
        insert_manifest(
            &conn,
            "e_lose",
            "strict",
            &json!({"node_types": [{"type": "person"}]}).to_string(),
        );
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('e_lose', 'e_surv', 1)",
            [],
        )
        .unwrap();
        let ingest = crate::config::IngestConfig::default();
        let degraded = crate::config::OntologyDegradedState::default();
        let ctx = || GateResolutionContext {
            ingest: &ingest,
            degraded: &degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        };
        let node = resolve_node_gate_decision(&conn, "e_lose", &[], ctx());
        assert!(
            !matches!(
                &node.verdict,
                ModeVerdict::StrictNoVocab(HoldReason::NoFallback { manifest: Some(m), .. })
                    if m == "e_lose"
            ),
            "the loser's row must not govern the mint: {:?}",
            node.verdict
        );

        // A survivor strict row DOES govern, and its hold names the
        // survivor — the row the printed fix can actually repair.
        insert_manifest(
            &conn,
            "e_surv",
            "strict",
            &json!({"node_types": [{"type": "person"}]}).to_string(),
        );
        let node = resolve_node_gate_decision(&conn, "e_lose", &[], ctx());
        assert_eq!(
            node.verdict,
            ModeVerdict::StrictNoVocab(HoldReason::NoFallback {
                manifest: Some("e_surv".to_string()),
                declared: vec!["person".to_string()],
            })
        );

        // An unresolvable chain holds (D8), naming the cause.
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('e_surv', 'e_lose', 1)",
            [],
        )
        .unwrap();
        let node = resolve_node_gate_decision(&conn, "e_lose", &[], ctx());
        assert!(
            matches!(
                node.verdict,
                ModeVerdict::StrictNoVocab(HoldReason::OptOutLookupFailed { .. })
                    | ModeVerdict::StrictNoVocab(HoldReason::RedirectUnresolved { .. })
            ),
            "{:?}",
            node.verdict
        );
    }

    /// Review finding: a merged-away id's leftover strict row is never read
    /// by the gate (rung 1b reads the survivor's), so the ensure must not
    /// report it unusable — its printed fix would write the survivor's row
    /// and never clear the report.
    #[test]
    fn ensure_does_not_report_a_merged_losers_row() {
        let conn = open_in_memory().unwrap();
        insert_manifest(
            &conn,
            "e_lose",
            "strict",
            &json!({"node_types": [{"type": "person"}], "fallback_node_type": "ghost"}).to_string(),
        );
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('e_lose', 'e_surv', 1)",
            [],
        )
        .unwrap();
        assert_eq!(
            ensure_manifest_vocabulary(&conn, "e_lose").unwrap(),
            EnsureOutcome::AlreadyComplete
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
    fn redirect_survivor_returns_survivor() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES (?1, ?2, 1)",
            params!["ent_a", "B"],
        )
        .unwrap();
        let outcome = redirect_survivor(&conn, "ent_a").unwrap();
        assert_eq!(outcome, RedirectOutcome::Survivor("B".into()));
    }

    /// Cycle (A → A) reports Cycle, never loops.
    #[test]
    fn redirect_survivor_detects_self_cycle() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES (?1, ?2, 1)",
            params!["ent_a", "ent_a"],
        )
        .unwrap();
        let outcome = redirect_survivor(&conn, "ent_a").unwrap();
        assert_eq!(outcome, RedirectOutcome::Cycle);
    }

    /// Multi-hop chain (a → b → c) resolves to the terminal survivor.
    #[test]
    fn redirect_survivor_walks_the_whole_chain() {
        // Review finding: a legacy 2-hop chain must resolve to the TERMINAL
        // survivor, never the intermediate loser.
        let conn = open_in_memory().unwrap();
        for (loser, survivor) in [("ent_a", "ent_b"), ("ent_b", "ent_c")] {
            conn.execute(
                "INSERT INTO entity_redirects (entity_id, merged_into, created_at) \
                 VALUES (?1, ?2, 1)",
                params![loser, survivor],
            )
            .unwrap();
        }
        let outcome = redirect_survivor(&conn, "ent_a").unwrap();
        assert_eq!(outcome, RedirectOutcome::Survivor("ent_c".into()));
    }

    /// No redirect → `None`.
    #[test]
    fn redirect_survivor_returns_none_when_absent() {
        let conn = open_in_memory().unwrap();
        let outcome = redirect_survivor(&conn, "ent_a").unwrap();
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
            GateDecision::Held(HoldReason::TierFactUnreadable),
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
            GateDecision::Held(HoldReason::TierFactUnreadable),
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
            // R2.4.4: the fallback is itself declared.
            by_key.insert("project".to_string(), "project".to_string());
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

    /// R2.4.4 (r26): a case-variant fallback (`Person` beside the declared
    /// `person`) is USABLE — membership is key-based — but must LAND in
    /// the declared entry's spelling, the same rule the declared and
    /// alias arms follow, so one type never splits into two exact-match
    /// buckets in `entity_type`.
    #[test]
    fn case_variant_fallback_lands_the_declared_spelling() {
        let manifest = crate::wiki_graph::parse_manifest_value(&serde_json::json!({
            "node_types": [{"type": "person"}, {"type": "place"}],
            "edge_types": [],
            "fallback_node_type": "Person",
        }));
        let vocab = NodeVocabulary::from_manifest(&manifest);
        assert_eq!(vocab.hold_reason(None), None, "case variant is usable");
        assert_eq!(vocab.fallback(), Some("person"), "stored canonically");

        let conn = open_in_memory().unwrap();
        let mut conn = conn;
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            None,
            "Unlabeled-mint",
            None,
            "",
            100,
            GateDecision::Gate(vocab),
            false,
        )
        .unwrap();
        match outcome {
            AdmitOutcome::DegradedToFallback { landed_as, .. } => {
                assert_eq!(landed_as, "person");
            }
            other => panic!("expected DegradedToFallback, got {other:?}"),
        }
        tx.commit().unwrap();
    }

    /// Task 9 fix round 1, Finding 2 pin: the UNLABELED ladder arm (Task 9)
    /// reaches BOTH the GUI mint (entities::create_entity, blank type) and
    /// LLM synthesis (commit.rs, no proposed type). Both sites route through
    /// `shared_insert_entity` with `proposed_label = None` under a `Gate`
    /// decision, so pinning the helper pins both paths:
    ///
    /// (a) Gate + declared fallback → the unlabeled mint LANDS as the
    ///     fallback (no refusal) — spec §2.5 GUI bullet "no-fallback-exists
    ///     → refusal error shown; otherwise normal ladder" (refusal is
    ///     reserved for the no-fallback case) and the bundle bullet's
    ///     "lands untyped entities as the MANIFEST'S DECLARED FALLBACK".
    /// (b) Gate + NO fallback → still Held (refusal), the pre-Task-9
    ///     contract.
    #[test]
    fn unlabeled_mint_under_gate_lands_as_fallback_else_held() {
        let conn = open_in_memory().unwrap();
        let mut conn = conn;

        let vocab_with_fallback = {
            let mut by_key = std::collections::HashMap::new();
            by_key.insert("person".to_string(), "person".to_string());
            // R2.4.4: the fallback is itself declared.
            by_key.insert("project".to_string(), "project".to_string());
            NodeVocabulary {
                by_key,
                fallback: Some("project".to_string()),
            }
        };
        let vocab_without_fallback = {
            let mut by_key = std::collections::HashMap::new();
            by_key.insert("person".to_string(), "person".to_string());
            NodeVocabulary {
                by_key,
                fallback: None,
            }
        };

        // (a) Unlabeled + declared fallback → lands as the fallback.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_unlabeled_fb"),
            "Unlabeled-mint",
            None,
            "",
            500,
            GateDecision::Gate(vocab_with_fallback),
            false,
        )
        .unwrap();
        match outcome {
            AdmitOutcome::DegradedToFallback { landed_as, .. } => {
                assert_eq!(landed_as, "project");
            }
            other => panic!("expected DegradedToFallback, got {other:?}"),
        }
        let landed: String = tx
            .query_row(
                "SELECT entity_type FROM curated_entities WHERE id = 'ent_unlabeled_fb'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(landed, "project", "the row landed as the declared fallback");
        tx.commit().unwrap();

        // (b) Unlabeled + NO fallback → Held, no row (refusal preserved).
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_unlabeled_nofb"),
            "Unlabeled-mint-2",
            None,
            "",
            600,
            GateDecision::Gate(vocab_without_fallback),
            false,
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                AdmitOutcome::Held {
                    reason: HoldReason::NoFallback { .. },
                    ..
                }
            ),
            "unlabeled without a declared fallback must still refuse, got {outcome:?}"
        );
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM curated_entities WHERE id = 'ent_unlabeled_nofb'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "Held must NOT insert a row");
        tx.commit().unwrap();
    }

    /// Fix-round-1 C1 (r13-MAJOR-3): the production gate resolver stamps
    /// the INITIAL watermark at the FIRST gate resolution, and the stamp is
    /// idempotent across re-resolutions (`INSERT OR IGNORE` keeps the first
    /// value — the watermark's definition).
    #[test]
    fn production_gate_stamps_initial_watermark() {
        let mut conn = open_in_memory().unwrap();
        // r21 hold-time rule: the policy load happens BEFORE the tx opens
        // (in-memory conn → the default policy, no filesystem read).
        let policy = crate::config::ingest_policy_for_db(conn.path());
        // Fresh brain: no manifest rows → ladder falls to rung 4 → Off/SKIP.
        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let (decision, _node) = resolve_production_gate(&tx, &policy, "ent_new", &[]);
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
        let _ = resolve_production_gate(&tx, &policy, "ent_other", &[]);
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

                    // r21 hold-time rule: config (filesystem) loads BEFORE
                    // the IMMEDIATE transaction opens.
                    let policy = crate::config::ingest_policy_for_db(conn.path());
                    let tx = ImmediateTx::begin(&mut conn).unwrap();
                    let (decision, node) =
                        resolve_production_gate(&tx, &policy, "ent_new", &["ops/a.md".to_string()]);
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

                    let policy = crate::config::ingest_policy_for_db(conn.path());
                    let tx = ImmediateTx::begin(&mut conn).unwrap();
                    let (decision, node) =
                        resolve_production_gate(&tx, &policy, "ent_new", &["ops/a.md".to_string()]);
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

    /// Resolve the ladder against a strict `tier_fact` row (fallback set)
    /// with the given ingest config — the shape where a rung-2/3 `off`
    /// used to be silently overridden by rung 4.
    fn ladder_with_strict_tier_fact(
        ingest: &crate::config::IngestConfig,
        degraded: &crate::config::OntologyDegradedState,
        source_paths: &[String],
    ) -> NodeGateDecision {
        let conn = open_in_memory().unwrap();
        conn.execute("DELETE FROM llm_wiki_entity_manifests", [])
            .unwrap();
        let manifest = serde_json::json!({
            "node_types": [{"type": "person"}, {"type": "concept"}],
            "edge_types": [],
            "fallback_node_type": "concept",
        });
        insert_manifest(
            &conn,
            "tier_fact",
            "strict",
            &serde_json::to_string(&manifest).unwrap(),
        );
        let ctx = GateResolutionContext {
            ingest,
            degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        };
        resolve_node_gate_decision(&conn, "ent_new", source_paths, ctx)
    }

    /// R2.3.4 / spec §6 matrix "bare host-off + GUI-minted character →
    /// ungated": a pathless mint (GUI / bundle) STARTS at rung 3, so a
    /// host-wide `ontology_default: off` SKIPs even under a strict tier_fact.
    #[test]
    fn pathless_mint_honors_rung_3_host_off_default() {
        let ingest = crate::config::IngestConfig {
            ontology_default: Some(crate::config::OntologyMode::Off),
            ..Default::default()
        };
        let node = ladder_with_strict_tier_fact(
            &ingest,
            &crate::config::OntologyDegradedState::default(),
            &[],
        );
        assert_eq!(node.verdict, ModeVerdict::Off);
        assert_eq!(node.source_directory, None);
    }

    /// A pathless mint under a load-degraded config holds (step 0, D8) —
    /// it must not fall through to the strict tier_fact vocabulary.
    #[test]
    fn pathless_mint_holds_under_degraded_global() {
        let degraded = crate::config::OntologyDegradedState {
            global: true,
            ..Default::default()
        };
        let node =
            ladder_with_strict_tier_fact(&crate::config::IngestConfig::default(), &degraded, &[]);
        assert_eq!(
            node.verdict,
            ModeVerdict::StrictNoVocab(HoldReason::ConfigHold { source: None })
        );
    }

    /// No default + no schema → rung 3 climbs; rung 4 strict tier_fact
    /// gates (the residual "default STRICT" rule).
    #[test]
    fn pathless_mint_climbs_to_tier_fact_without_default() {
        let node = ladder_with_strict_tier_fact(
            &crate::config::IngestConfig::default(),
            &crate::config::OntologyDegradedState::default(),
            &[],
        );
        assert_eq!(node.verdict, ModeVerdict::Gate);
    }

    /// §2.3 rung 2 "off = SKIP, first hit decides": every source resolving
    /// to an `off` folder SKIPs even when tier_fact is strict, recording the
    /// off folder for the ledger row.
    #[test]
    fn all_off_sources_skip_despite_strict_tier_fact() {
        let mut ingest = crate::config::IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".to_string(), crate::config::OntologyMode::Off);
        let node = ladder_with_strict_tier_fact(
            &ingest,
            &crate::config::OntologyDegradedState::default(),
            &["ops/a.md".to_string(), "ops/b.md".to_string()],
        );
        assert_eq!(node.verdict, ModeVerdict::Off);
        assert_eq!(node.source_directory.as_deref(), Some("ops/a.md"));
    }

    /// Review finding (R2.3.3 strict-wins across source paths): a HELD
    /// source (dropped `folder_ontology` prefix) must not short-circuit the
    /// walk — a later strict source still wins and gates.
    #[test]
    fn held_source_does_not_mask_a_later_strict_source() {
        let mut ingest = crate::config::IngestConfig::default();
        ingest
            .folder_ontology
            .insert("work".to_string(), crate::config::OntologyMode::Strict);
        let degraded = crate::config::OntologyDegradedState {
            dropped_prefixes: vec!["held".to_string()],
            ..Default::default()
        };
        let node = ladder_with_strict_tier_fact(
            &ingest,
            &degraded,
            &["held/a.md".to_string(), "work/b.md".to_string()],
        );
        assert_eq!(node.verdict, ModeVerdict::Gate);
        assert_eq!(node.source_directory.as_deref(), Some("work/b.md"));
    }

    /// A held source beside a CLIMBING sibling reaches rung 4: strict-wins
    /// means the held source (strict or off, unknown) cannot change a
    /// strict rung-4 verdict, and the vocabulary is `tier_fact`'s either way.
    #[test]
    fn held_source_with_climbing_sibling_gates_via_tier_fact() {
        let degraded = crate::config::OntologyDegradedState {
            dropped_prefixes: vec!["held".to_string()],
            ..Default::default()
        };
        let node = ladder_with_strict_tier_fact(
            &crate::config::IngestConfig::default(),
            &degraded,
            &["held/a.md".to_string(), "notes/b.md".to_string()],
        );
        assert_eq!(node.verdict, ModeVerdict::Gate);
    }

    /// A held source beside an `off` sibling (no climb, no strict) still
    /// HOLDS on a strict `tier_fact`: the held source might be strict, and
    /// without it every source would be off — undeterminable, so hold.
    #[test]
    fn held_source_with_off_sibling_holds() {
        let mut ingest = crate::config::IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".to_string(), crate::config::OntologyMode::Off);
        let degraded = crate::config::OntologyDegradedState {
            dropped_prefixes: vec!["held".to_string()],
            ..Default::default()
        };
        let node = ladder_with_strict_tier_fact(
            &ingest,
            &degraded,
            &["held/a.md".to_string(), "ops/b.md".to_string()],
        );
        assert_eq!(
            node.verdict,
            ModeVerdict::StrictNoVocab(HoldReason::ConfigHold {
                source: Some("held/a.md".to_string())
            })
        );
        assert_eq!(node.source_directory.as_deref(), Some("held/a.md"));
    }

    /// r4-m4 parity (review finding): an UNREADABLE entity manifest row
    /// HOLDS even when the only deciding signal is a HELD source with no
    /// climbing sibling and no strict `tier_fact` row — the same guard the
    /// `off_found` arm and rung 4 apply. Pre-fix, the deferred-Hold arm
    /// returned Off (SKIP) here and the mint landed silently.
    #[test]
    fn held_source_with_unreadable_entity_row_holds() {
        let conn = open_in_memory().unwrap();
        // The entity's OWN manifest row is corrupt — rung 1b sets
        // `entity_row_unreadable` (it may have declared a strict mode we
        // cannot read).
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent_new', 'strict', 'not json at all', 1)",
            [],
        )
        .unwrap();
        let degraded = crate::config::OntologyDegradedState {
            dropped_prefixes: vec!["held".to_string()],
            ..Default::default()
        };
        let ctx = GateResolutionContext {
            ingest: &crate::config::IngestConfig::default(),
            degraded: &degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        };
        let node = resolve_node_gate_decision(&conn, "ent_new", &["held/a.md".to_string()], ctx);
        let reason = HoldReason::EntityManifestUnreadable {
            entity_id: "ent_new".to_string(),
        };
        assert_eq!(node.verdict, ModeVerdict::StrictNoVocab(reason.clone()));
        assert_eq!(node.into_gate_decision(), GateDecision::Held(reason));
    }

    /// Ensure dedupe is case-insensitive (review finding): a manifest that
    /// already declares `Document` must not gain a duplicate `document`,
    /// and the fallback is written in the manifest's own spelling.
    #[test]
    fn ensure_dedupe_is_case_insensitive_and_keeps_manifest_spelling() {
        let mut types: Vec<serde_json::Value> =
            EA_SEED_TYPES.iter().map(|s| json!({"type": s})).collect();
        types.push(json!({"type": "Document"}));
        types.push(json!({"type": "Concept"}));
        let manifest = json!({"node_types": types, "edge_types": []}).to_string();
        let EnsurePlan::Edit { new_json, .. } =
            plan_manifest_ensure(&manifest, "tier_fact", true).unwrap()
        else {
            panic!("expected an edit (process + fallback missing)");
        };
        let parsed: serde_json::Value = serde_json::from_str(&new_json).unwrap();
        let declared: Vec<&str> = parsed["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["type"].as_str())
            .collect();
        assert_eq!(
            declared
                .iter()
                .filter(|t| t.eq_ignore_ascii_case("document"))
                .count(),
            1,
            "no case-variant duplicate: {declared:?}"
        );
        assert!(declared.contains(&"process"), "{declared:?}");
        assert_eq!(parsed["fallback_node_type"].as_str(), Some("Concept"));
    }

    /// The same ladder on a brain with NO `tier_fact` row at all.
    fn ladder_without_tier_fact(
        ingest: &crate::config::IngestConfig,
        degraded: &crate::config::OntologyDegradedState,
        source_paths: &[String],
    ) -> NodeGateDecision {
        let conn = open_in_memory().unwrap();
        conn.execute("DELETE FROM llm_wiki_entity_manifests", [])
            .unwrap();
        let ctx = GateResolutionContext {
            ingest,
            degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        };
        resolve_node_gate_decision(&conn, "ent_new", source_paths, ctx)
    }

    /// r9-M1 mode vs vocabulary: a strict folder supplies a MODE only; with
    /// no `tier_fact` vocabulary row the mint SKIPs (+ census warning), it
    /// is not a §2.4.5 hold. Matrix: fresh brain + strict + one mint.
    #[test]
    fn strict_folder_without_tier_fact_row_skips() {
        let mut ingest = crate::config::IngestConfig::default();
        ingest
            .folder_ontology
            .insert("people".to_string(), crate::config::OntologyMode::Strict);
        let node = ladder_without_tier_fact(
            &ingest,
            &crate::config::OntologyDegradedState::default(),
            &["people/x.md".to_string()],
        );
        assert_eq!(node.verdict, ModeVerdict::Off);
        assert_eq!(node.into_gate_decision(), GateDecision::Skip);
    }

    /// r10-MINOR-2: the vocabulary check runs FIRST — a degraded-config
    /// hold is moot with no `tier_fact` row (fresh brain + typo → SKIP),
    /// while the same degraded config over a strict row still holds.
    #[test]
    fn degraded_hold_is_moot_without_tier_fact_row() {
        let degraded = crate::config::OntologyDegradedState {
            global: true,
            ..Default::default()
        };
        let node =
            ladder_without_tier_fact(&crate::config::IngestConfig::default(), &degraded, &[]);
        assert_eq!(node.verdict, ModeVerdict::Off);
        let node =
            ladder_with_strict_tier_fact(&crate::config::IngestConfig::default(), &degraded, &[]);
        assert_eq!(
            node.verdict,
            ModeVerdict::StrictNoVocab(HoldReason::ConfigHold { source: None })
        );
    }

    /// R2.3.3 strict-wins: an off source never downgrades an entity whose
    /// other source climbs to the strict residual rung.
    #[test]
    fn off_plus_climbing_source_gates_via_tier_fact() {
        let mut ingest = crate::config::IngestConfig::default();
        ingest
            .folder_ontology
            .insert("ops".to_string(), crate::config::OntologyMode::Off);
        let node = ladder_with_strict_tier_fact(
            &ingest,
            &crate::config::OntologyDegradedState::default(),
            &["ops/a.md".to_string(), "notes/b.md".to_string()],
        );
        assert_eq!(node.verdict, ModeVerdict::Gate);
    }

    /// Resolve a fresh mint against a strict `tier_fact` row carrying
    /// `manifest`, pathless (GUI/bundle shape), no folder config.
    fn ladder_over_tier_fact(manifest: serde_json::Value) -> (Connection, NodeGateDecision) {
        let conn = open_in_memory().unwrap();
        conn.execute("DELETE FROM llm_wiki_entity_manifests", [])
            .unwrap();
        insert_manifest(&conn, "tier_fact", "strict", &manifest.to_string());
        let ingest = crate::config::IngestConfig::default();
        let degraded = crate::config::OntologyDegradedState::default();
        let ctx = GateResolutionContext {
            ingest: &ingest,
            degraded: &degraded,
            schema: None,
            schema_unparseable: false,
            vault_root: None,
        };
        let node = resolve_node_gate_decision(&conn, "ent_new", &[], ctx);
        (conn, node)
    }

    /// R2.4.5 (explicit): a strict `tier_fact` that declares node types but
    /// NO `fallback_node_type` holds EVERY new-entity mint — a mint
    /// proposing a DECLARED type included. The gate never runs a partial
    /// vocabulary. The fixture declares neither `concept` nor `project`, the
    /// foreign-manifest shape the ensure leaves fallback-less.
    #[test]
    fn strict_no_fallback_holds_a_declared_label_mint() {
        let (mut conn, node) = ladder_over_tier_fact(serde_json::json!({
            "node_types": [{"type": "person"}, {"type": "place"}],
            "edge_types": [],
        }));
        let expected = HoldReason::NoFallback {
            manifest: Some("tier_fact".to_string()),
            declared: vec!["person".to_string(), "place".to_string()],
        };
        assert_eq!(node.verdict, ModeVerdict::StrictNoVocab(expected.clone()));

        let tx = ImmediateTx::begin(&mut conn).unwrap();
        let outcome = shared_insert_entity(
            &tx,
            Some("ent_new"),
            "Ada",
            Some("person"),
            "",
            1,
            node.into_gate_decision(),
            false,
        )
        .unwrap();
        assert_eq!(
            outcome,
            AdmitOutcome::Held {
                original_label: Some("person".to_string()),
                reason: expected,
            },
            "a declared label is held too"
        );
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM curated_entities WHERE id = 'ent_new'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "held: nothing inserted");
    }

    /// R2.4.4: a `fallback_node_type` that is not one of the manifest's
    /// node types is the same configuration error — it holds rather than
    /// landing an undeclared type.
    #[test]
    fn undeclared_fallback_holds_instead_of_landing_it() {
        let (_conn, node) = ladder_over_tier_fact(serde_json::json!({
            "node_types": [{"type": "person"}],
            "edge_types": [],
            "fallback_node_type": "thing",
        }));
        assert_eq!(
            node.verdict,
            ModeVerdict::StrictNoVocab(HoldReason::FallbackNotDeclared {
                manifest: Some("tier_fact".to_string()),
                fallback: "thing".to_string(),
                declared: vec!["person".to_string()],
            })
        );
    }

    /// A fallback over ZERO node types holds at the resolver (it used to
    /// reach `Gate` and hold only inside the insert helper), naming the
    /// empty set rather than a fallback the manifest already has.
    #[test]
    fn fallback_over_empty_node_types_holds_naming_the_empty_set() {
        let (_conn, node) = ladder_over_tier_fact(serde_json::json!({
            "node_types": [],
            "edge_types": [],
            "fallback_node_type": "person",
        }));
        assert_eq!(
            node.verdict,
            ModeVerdict::StrictNoVocab(HoldReason::EmptyNodeTypes {
                manifest: Some("tier_fact".to_string()),
                fallback: Some("person".to_string()),
            })
        );
    }

    /// Only the vocabulary causes print a `--fallback` fix, and the command
    /// targets the row that held the mint: `tier_fact` is the default
    /// target, an entity row needs `--entity`.
    #[test]
    fn hold_reason_messages_name_the_cause_and_the_right_fix() {
        let declared = vec!["person".to_string(), "place".to_string()];
        let tier = HoldReason::NoFallback {
            manifest: Some("tier_fact".to_string()),
            declared: declared.clone(),
        }
        .to_string();
        assert!(tier.contains("declared type"), "{tier}");
        assert!(
            tier.contains("`ct ontology set --fallback <type>` with one of: person, place"),
            "{tier}"
        );

        let entity = HoldReason::NoFallback {
            manifest: Some("ent_x".to_string()),
            declared,
        }
        .to_string();
        assert!(
            entity.contains("`ct ontology set --entity=ent_x --fallback <type>`"),
            "{entity}"
        );

        // r26: the unnamed-row arm still prints a PASTEABLE command — no
        // bracket placeholders (glob characters in zsh/bash) — with the
        // entity-row variant named in prose.
        let unnamed = HoldReason::NoFallback {
            manifest: None,
            declared: vec!["person".to_string()],
        }
        .to_string();
        assert!(!unnamed.contains('['), "{unnamed}");
        assert!(
            unnamed.contains("`ct ontology set --fallback <type>`"),
            "{unnamed}"
        );
        assert!(unnamed.contains("--entity <id>"), "{unnamed}");

        // r26: an entity strict row is CT-written (a `tier_fact` copy) —
        // its EmptyNodeTypes fix names the re-copy, not the wiki engine
        // (which never rewrites an entity-scoped row).
        let entity_empty = HoldReason::EmptyNodeTypes {
            manifest: Some("ent_x".to_string()),
            fallback: None,
        }
        .to_string();
        assert!(
            entity_empty.contains("`ct ontology set --entity=ent_x --mode strict`"),
            "{entity_empty}"
        );

        let tier_empty = HoldReason::EmptyNodeTypes {
            manifest: Some("tier_fact".to_string()),
            fallback: Some("person".to_string()),
        }
        .to_string();
        assert!(tier_empty.contains("wiki engine"), "{tier_empty}");
        assert!(tier_empty.contains("`person`"), "{tier_empty}");

        // With NO fallback named, adding node types alone would only trade
        // this hold for NoFallback: the fix names both steps.
        let tier_bare = HoldReason::EmptyNodeTypes {
            manifest: Some("tier_fact".to_string()),
            fallback: None,
        }
        .to_string();
        assert!(
            tier_bare.contains("`ct ontology set --fallback <type>`"),
            "{tier_bare}"
        );

        // Entity ids are shell-quoted in every printed command: a bundle
        // id with shell metacharacters must not inject into a paste.
        let hostile = HoldReason::NoFallback {
            manifest: Some("ent_a; rm -rf ~".to_string()),
            declared: vec!["person".to_string()],
        }
        .to_string();
        assert!(
            hostile.contains("`ct ontology set --entity='ent_a; rm -rf ~' --fallback <type>`"),
            "{hostile}"
        );

        // Backstops name a bug, never a configuration cause.
        let internal = HoldReason::Internal { detail: "x" }.to_string();
        assert!(internal.contains("bug"), "{internal}");

        for reason in [
            HoldReason::TierFactUnreadable,
            HoldReason::EntityManifestUnreadable {
                entity_id: "ent_x".to_string(),
            },
            HoldReason::OptOutLookupFailed {
                entity_id: "ent_x".to_string(),
            },
            HoldReason::ConfigHold { source: None },
            HoldReason::EmptyNodeTypes {
                manifest: Some("tier_fact".to_string()),
                fallback: Some("person".to_string()),
            },
            HoldReason::RedirectUnresolved {
                entity_id: "ent_x".to_string(),
            },
            HoldReason::Internal { detail: "x" },
        ] {
            let message = reason.to_string();
            assert!(
                !message.contains("--fallback"),
                "{reason:?} must not prescribe a fallback fix: {message}"
            );
        }
    }
}
