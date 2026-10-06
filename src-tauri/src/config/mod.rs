use crate::embedder::EmbedProfile;
pub use crate::inference::config::{EmbeddingConfig, GenerationConfig};
use crate::ontology_config::OntologyConfigBlock;
use crate::privacy::PrivacyConfig;
use crate::retrieval::BrainPaths;
use crate::trusted_links::TrustedLink;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use thiserror::Error;
use uuid::Uuid;

/// Fatal errors from `BrainConfig::load_lenient`.
///
/// Malformed top-level JSON, non-object roots, and present-but-non-string
/// `vault_path` values are classified as hard errors because masking them once
/// silently reset users' vault paths and forced re-onboarding (final-review M1,
/// and the same failure class as today's `inference::write_config` silently
/// replacing a malformed config with `{}`). Callers MUST propagate these as
/// typed errors rather than matching on a `diagnostics: Vec<String>` string.
/// The only IO condition returned as `Ok` is "file missing" — that is the
/// normal post-onboarding state, not corruption.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config.json is malformed JSON: {0}")]
    MalformedJson(#[from] serde_json::Error),
    #[error("config.json root must be a JSON object (got {actual})")]
    NonObjectRoot { actual: &'static str },
    #[error("config.json vault_path is present but not a string")]
    VaultPathNotString,
    #[error("config.json could not be read: {0}")]
    Io(#[from] std::io::Error),
}

/// Classifies a parsed-but-non-object JSON value for the error variant.
fn root_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Tier stamped on deposit-ingested entries when config does not say otherwise.
///
/// Shipped default per spec §3.2: deposits are agent-written notes under active
/// revision, and `"fact"` invokes the librarian's "ANCHOR TRUTH — do not propose
/// modifications" framing, which would freeze exactly the content agents are
/// expected to keep correcting.
pub const DEFAULT_DEPOSIT_TIER: &str = "wisdom";

/// Wiki-layer settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WikiConfig {
    /// Tier stamped on deposit-ingested entries. Shipped default `"wisdom"`.
    #[serde(default)]
    pub deposit_default_tier: Option<String>,
}

/// Per-folder ingestion tier (F4, spec 2026-09-27-vault-ingest-policy).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IngestTier {
    /// Chunks + embeddings + librarian fact extraction (current behavior).
    #[serde(rename = "full")]
    #[default]
    Full,
    /// Chunk + embed; the librarian skips fact extraction.
    #[serde(rename = "chunks-only")]
    ChunksOnly,
    /// Do not index at all (watcher-exclusion equivalent).
    #[serde(rename = "none")]
    None,
}

impl IngestTier {
    /// True when this tier suppresses librarian fact extraction.
    pub fn skips_fact_extraction(self) -> bool {
        matches!(self, IngestTier::ChunksOnly | IngestTier::None)
    }

    /// Explicit conservatism ranking used ONLY by tie resolution:
    /// `none` < `chunks-only` < `full`. When two configured keys normalize
    /// to the same prefix with different tiers, the most conservative wins.
    /// (Deliberately not called "conservatism" on `Full` — `Full` is the
    /// LEAST conservative tier under this ranking.)
    pub fn tie_rank(self) -> u8 {
        match self {
            IngestTier::None => 0,
            IngestTier::ChunksOnly => 1,
            IngestTier::Full => 2,
        }
    }
}

/// Directory-level gate mode for the node/edge ontology gate (spec
/// 2026-10-03-ontology-node-type-gate-and-heal §2.2, R2.2.1/R2.2.3).
/// Values are LOWERCASE and case-sensitive: a hand-edited `"Off"` is a bad
/// value — dropped by salvage with `ingest_ontology_degraded` set, never
/// silently parsed into a legal value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OntologyMode {
    /// No gating: mints in this subtree skip the node/edge type gate.
    Off,
    /// Strict gating: mints in this subtree must match the manifest.
    Strict,
}

impl OntologyMode {
    /// Conservatism ranking: `off` < `strict`. A same-normalized-prefix
    /// conflict resolves to the more conservative `strict` if a caller
    /// must break the tie WITHOUT the hold (resolver step (2) currently
    /// Holds instead — `strict`-wins is the documented fallback rule,
    /// D8-fail-closed); kept adjacent to [`IngestTier::tie_rank`] for the
    /// symmetry the tier tie diagnostic names.
    pub fn tie_rank(self) -> u8 {
        match self {
            OntologyMode::Off => 0,
            OntologyMode::Strict => 1,
        }
    }
}

/// Normalize a configured folder-map key so key comparisons (tie detection,
/// watermark hash) and prefix matching agree: `\` → `/`, then trim leading
/// and trailing `/`. Deliberately does NOT trim a leading `./` — configured
/// keys get no `./`-normalization (asymmetry with the queried path, see
/// [`IngestConfig::tier_for`]), so `{"./ops": …}` stays inert and is
/// flagged by the load-time unmatchable-key diagnostic.
pub fn normalize_key(key: &str) -> String {
    key.replace('\\', "/").trim_matches('/').to_string()
}

/// True when a (raw) configured key can ever match a vault-relative path.
/// Unmatchable keys — empty after normalization, a leading `./`, or any
/// segment that is empty, `.`, or `..` — are INERT (the resolver skips
/// them, same drop-one parity as the historical empty-key skip) and get a
/// load-time diagnostic: silently inert `folder_ontology` keys would leave
/// a subtree gated strict when the user meant `off` (the D8 harm).
/// Segment-based per the `..` rule: `v1..v2` is a real folder name and
/// stays matchable.
pub fn key_is_matchable(key: &str) -> bool {
    let k = normalize_key(key);
    if k.is_empty() {
        return false;
    }
    k.split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// The prefix a DROPPED `folder_ontology` key can honestly hold under
/// (M-2): [`normalize_key`] plus stripping per-segment `.` noise. This is
/// DELIBERATELY stricter than [`match_prefixes`]' queried-path trimming,
/// which strips only a LEADING `./` — `usable_prefix` removes EVERY `.`
/// segment wherever it appears, and rejects any key carrying a `..`
/// segment outright. A key like `"./ops"` is unmatchable (a load-time
/// diagnostic fires) but still has a usable path form — the resolver's
/// dropped-prefix hold must work on it, or the "mints under it hold until
/// fixed" diagnostic over-promises. Keys with NO usable form (empty, `/`,
/// or any `..` segment) return "" and hold nothing.
fn usable_prefix(key: &str) -> String {
    let k = normalize_key(key);
    let segs: Vec<&str> = k
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if segs.iter().any(|s| *s == "..") {
        return String::new();
    }
    segs.join("/")
}

/// Outcome of the shared longest-prefix resolver over a folder map
/// (`folder_tiers` / `folder_ontology`). Four states because two state
/// pairs must not be conflated: "no prefix matched" (`NoMatch`) is
/// different from "an absolute path couldn't be placed inside the vault"
/// (`Unplaceable`), and a resolving value (`Match`) is different from a
/// same-normalized-key conflict (`Tie`, which the ontology caller maps to
/// a hold and the tier caller resolves conservative-wins).
#[derive(Debug, Clone, PartialEq)]
pub enum PrefixOutcome<T> {
    /// Deepest matching prefix won; all same-depth keys agree on the value.
    Match(T),
    /// No configured prefix matches the path.
    NoMatch,
    /// Absolute path with no effective vault root, or outside the vault
    /// (`relativize_to_vault` → None). Relative paths are NEVER
    /// `Unplaceable` — the `is_absolute()` check runs first.
    Unplaceable,
    /// ≥2 configured keys normalize to the longest matched prefix with
    /// conflicting values. Carries the values so the tier caller can pick
    /// the most conservative (`IngestTier::tie_rank`) and the ontology
    /// caller can hold. Same-VALUE keys resolve to `Match` (harmless).
    Tie(Vec<T>),
}

/// One resolver call, with the detail the ontology wrapper needs for its
/// dropped-prefix scoped-hold checks.
pub struct PrefixMatch<T> {
    /// Normalized vault-relative path (`\` → `/`, leading `./` trimmed).
    /// `None` only for `Unplaceable` (no relative path could be computed).
    pub rel_path: Option<String>,
    pub outcome: PrefixOutcome<T>,
    /// `(normalized key, component depth)` of the deepest matched prefix,
    /// when one matched (set for `Match` and `Tie`).
    pub matched: Option<(String, usize)>,
}

/// The shared prefix resolver. SILENT by contract: tie and unmatchable-key
/// diagnostics fire once at load time (`load_lenient` / `load()`'s strict
/// arm), never here — `tier_for_path` runs per document on the walk hot
/// path and an in-resolver diagnostic would spam stderr per document.
pub fn resolve_prefix<T: PartialEq + Copy>(
    map: &std::collections::HashMap<String, T>,
    path: &str,
    vault_root: Option<&std::path::Path>,
) -> PrefixOutcome<T> {
    resolve_prefix_detailed(map, path, vault_root).outcome
}

pub fn resolve_prefix_detailed<T: PartialEq + Copy>(
    map: &std::collections::HashMap<String, T>,
    path: &str,
    vault_root: Option<&std::path::Path>,
) -> PrefixMatch<T> {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        // Absolute paths MUST be relativized against the effective vault
        // root before any key can match; an absolute path that cannot be
        // placed inside the vault is `Unplaceable`, never a silent `Full`
        // or a random-winner match.
        let rel = vault_root.and_then(|root| crate::walk_vault::relativize_to_vault(p, root));
        let Some(rel) = rel else {
            return PrefixMatch {
                rel_path: None,
                outcome: PrefixOutcome::Unplaceable,
                matched: None,
            };
        };
        match_prefixes(map, &rel.to_string_lossy())
    } else {
        match_prefixes(map, path)
    }
}

fn match_prefixes<T: PartialEq + Copy>(
    map: &std::collections::HashMap<String, T>,
    path: &str,
) -> PrefixMatch<T> {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_start_matches("./");
    let mut best: Option<(String, usize, Vec<T>)> = None;
    for (key, val) in map {
        let prefix = normalize_key(key);
        // Inert keys (empty after normalization, or unmatchable segments)
        // are skipped — same parity as the historical empty-key skip, now
        // extended to `.`/`..`/leading-`./` keys, which get a load-time
        // diagnostic instead of a silent skip.
        if !key_is_matchable(key) {
            continue;
        }
        if normalized.starts_with(&format!("{prefix}/")) {
            let depth = prefix.split('/').count();
            best = match best {
                None => Some((prefix, depth, vec![*val])),
                Some((_, d, _)) if depth > d => Some((prefix, depth, vec![*val])),
                Some((k, d, mut vals)) if depth == d => {
                    vals.push(*val);
                    Some((k, d, vals))
                }
                Some(kept) => Some(kept),
            };
        }
    }
    let rel_path = Some(normalized.to_string());
    match best {
        None => PrefixMatch {
            rel_path,
            outcome: PrefixOutcome::NoMatch,
            matched: None,
        },
        Some((key, depth, vals)) => {
            let first = vals[0];
            if vals.iter().all(|v| *v == first) {
                PrefixMatch {
                    rel_path,
                    outcome: PrefixOutcome::Match(first),
                    matched: Some((key, depth)),
                }
            } else {
                PrefixMatch {
                    rel_path,
                    outcome: PrefixOutcome::Tie(vals),
                    matched: Some((key, depth)),
                }
            }
        }
    }
}

/// Group a folder map by [`normalize_key`] and report conflicting
/// same-normalized-key groups as `(normalized key, value)` pairs, one pair
/// per conflicting value. Only MATCHABLE keys participate: unmatchable
/// keys are inert (the resolver skips them), so a conflict between two
/// inert keys must not degrade the config. Used by the load-time tie
/// scans, the `ontology_ties` helper, and the watermark hash's degraded
/// encoding.
fn normalized_key_ties<T: PartialEq>(map: &std::collections::HashMap<String, T>) -> Vec<(String, T)>
where
    T: Copy,
{
    let mut groups: std::collections::BTreeMap<String, Vec<T>> = std::collections::BTreeMap::new();
    for (k, v) in map {
        if !key_is_matchable(k) {
            continue;
        }
        groups.entry(normalize_key(k)).or_default().push(*v);
    }
    let mut out = Vec::new();
    for (k, vals) in groups {
        let first = vals[0];
        if vals.iter().any(|v| *v != first) {
            for v in vals {
                out.push((k.clone(), v));
            }
        }
    }
    out
}

/// Map-wide `folder_ontology` tie scan: same-normalized-key conflicting
/// modes anywhere in the map. A strict parse never passes through salvage,
/// so ties must be detected by this standalone scan at load time; the
/// watermark hash's degraded encoding also consumes it (conflicting keys
/// excluded from the hashed map).
pub fn ontology_ties(cfg: &IngestConfig) -> Vec<(String, OntologyMode)> {
    normalized_key_ties(&cfg.folder_ontology)
}

/// Tier tie scan — same shape as [`ontology_ties`] over `folder_tiers`.
/// Resolution is NOT degraded for tiers: the most conservative tier wins
/// (`IngestTier::tie_rank`); the load path just emits the loud diagnostic.
fn tier_ties(cfg: &IngestConfig) -> Vec<(String, IngestTier)> {
    normalized_key_ties(&cfg.folder_tiers)
}

/// Load-time diagnostics shared by BOTH load paths (`load()`'s strict
/// success arm never calls `load_lenient`, so each arm runs the scans
/// itself). Returns the ontology-tie degraded flag plus the diagnostic
/// lines; callers route them to `LoadReport.diagnostics` (lenient) or
/// stderr (strict).
fn scan_ingest_ties(ingest: &IngestConfig) -> (bool, Vec<String>) {
    let mut degraded = false;
    let mut msgs = Vec::new();
    let ties = ontology_ties(ingest);
    if !ties.is_empty() {
        degraded = true;
        let keys: std::collections::BTreeSet<&str> = ties.iter().map(|(k, _)| k.as_str()).collect();
        msgs.push(format!(
            "ingest.folder_ontology tie: keys {keys:?} normalize to the same prefix with conflicting modes; affected mints hold until the config is fixed"
        ));
    }
    for (k, v) in tier_ties(ingest) {
        msgs.push(format!(
            "ingest.folder_tiers tie on prefix {k:?}: most conservative wins ({v:?}; ranking none < chunks-only < full)"
        ));
    }
    (degraded, msgs)
}

/// Load-time unmatchable-key diagnostics for `folder_ontology` (see
/// [`key_is_matchable`]). `folder_tiers` keeps its historical silence —
/// an inert tier key degrades to `full`, never to a stricter gate.
/// M-2 accuracy: a key with a usable path form (`./ops`) now holds under
/// that form (see [`usable_prefix`]); only a key with NO usable form
/// truly "stays inert" — the message names which.
fn unmatchable_ontology_key_msgs(ingest: &IngestConfig) -> Vec<String> {
    ingest
        .folder_ontology
        .keys()
        .filter(|k| !key_is_matchable(k))
        .map(|k| {
            if usable_prefix(k).is_empty() {
                format!(
                    "ingest.folder_ontology key {k:?} cannot match any path (empty, or contains '.', '..', or a leading './' segment); it stays inert"
                )
            } else {
                format!(
                    "ingest.folder_ontology key {k:?} cannot match any path (contains a leading './' or '.' segment); mints under {:?} hold until it is fixed",
                    usable_prefix(k)
                )
            }
        })
        .collect()
}

/// Ingestion-policy block. Absent from config.json = every path `full`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestConfig {
    /// Map of vault-relative folder prefix → tier. Resolution walks the
    /// path's ancestor folders; the DEEPEST matching entry wins
    /// (path-component aware: `ops` never matches `ops-archive`).
    #[serde(default)]
    pub folder_tiers: std::collections::HashMap<String, IngestTier>,
    /// Map of vault-relative folder prefix → node/edge gate mode
    /// (spec R2.2.1). Same longest-prefix resolution core as
    /// `folder_tiers` (`resolve_prefix`); absent = the §2.3 ladder
    /// decides (no default from this map).
    #[serde(default)]
    pub folder_ontology: std::collections::HashMap<String, OntologyMode>,
    /// Host-wide default gate mode (spec R2.2.3). `None` = ABSENT (never
    /// chosen) — rung 3 then resolves LIVE from `ontology.schema == Off`;
    /// a set value wins over the schema-derived default. A hand-written
    /// `null` parses as `None` = absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ontology_default: Option<OntologyMode>,
}

impl IngestConfig {
    /// Resolve the tier for a VAULT-RELATIVE path.
    ///
    /// Matching is anchored at the vault root and on FOLDER components, not
    /// raw strings, so a prefix `ops` cannot capture a sibling `ops-archive`
    /// nor an `ops/` folder nested elsewhere; among matching prefixes the
    /// deepest wins; anything unmatched is [`IngestTier::Full`]. Separator
    /// spellings (`\` vs `/`) are normalized on BOTH the path and the
    /// configured keys (config.json is hand-edited). Absolute paths never
    /// match — relativize first via [`IngestConfig::tier_for_path`].
    pub fn tier_for(&self, rel_path: &str) -> IngestTier {
        match resolve_prefix(&self.folder_tiers, rel_path, None) {
            PrefixOutcome::Tie(vals) => vals
                .into_iter()
                .min_by_key(|t| t.tie_rank())
                .unwrap_or(IngestTier::Full),
            PrefixOutcome::Match(t) => t,
            // Unplaceable cannot arise: a relative path never enters the
            // absolute branch, so the vault root is never consulted.
            PrefixOutcome::NoMatch | PrefixOutcome::Unplaceable => IngestTier::Full,
        }
    }

    /// Resolve the tier for a vault-relative OR absolute path. An absolute
    /// path is relativized against `vault_root` first, so folder names in
    /// the vault's own ancestors (`/home/operations/vault/…`) can never
    /// match a tier key. An absolute path that cannot be placed inside the
    /// vault (or with no root known) resolves to [`IngestTier::Full`] —
    /// the shipped behavior (wording deliberate: `Full` is the LEAST
    /// conservative tier in the `none` < `chunks-only` < `full` ranking —
    /// do NOT "fix" this to `None`).
    ///
    /// The empty-map short-circuit stays BEFORE the core call: on the
    /// per-document walk hot path an empty map must not pay up to two
    /// `fs::canonicalize` calls inside `relativize_to_vault` for a `Full`
    /// answer either way.
    pub fn tier_for_path(&self, path: &str, vault_root: Option<&std::path::Path>) -> IngestTier {
        if self.folder_tiers.is_empty() {
            return IngestTier::Full;
        }
        match resolve_prefix(&self.folder_tiers, path, vault_root) {
            PrefixOutcome::Tie(vals) => vals
                .into_iter()
                .min_by_key(|t| t.tie_rank())
                .unwrap_or(IngestTier::Full),
            PrefixOutcome::Match(t) => t,
            // D8-adjacent parity: an unplaceable absolute path keeps the
            // shipped `Full`, exactly as before this resolver existed.
            PrefixOutcome::NoMatch | PrefixOutcome::Unplaceable => IngestTier::Full,
        }
    }

    /// Resolve the gate mode for a vault-relative OR absolute path —
    /// the CONFIG-LEVEL core behind the gate's ladder (spec R2.2.1/R2.2.6).
    ///
    /// Degraded inputs — the load-failed/global flag, a dropped
    /// `folder_ontology` prefix, or a dropped `ontology_default` value —
    /// resolve to [`OntologyLookup::Hold`] for any mint whose resolution
    /// would reach them (D8: a degraded or off state never climbs to a
    /// stricter rung). The resolver never sees dropped entries (salvage
    /// removed them), so this wrapper checks the dropped lists ITSELF:
    /// a valid entry DEEPER than a dropped parent wins (the user's
    /// narrower choice is intact), anything at-or-under a dropped prefix
    /// with no deeper valid entry holds.
    pub fn ontology_lookup(
        &self,
        path: &str,
        vault_root: Option<&std::path::Path>,
        degraded: &OntologyDegradedState,
        schema: Option<crate::ontology_config::OntologySelection>,
        schema_unparseable: bool,
    ) -> OntologyLookup {
        // (0) Global degraded (load-failed / non-object parts) → Hold.
        if degraded.global {
            return OntologyLookup::Hold;
        }

        let detailed = resolve_prefix_detailed(&self.folder_ontology, path, vault_root);

        // (1) At-or-under a dropped prefix → Hold, unless a VALID entry
        // deeper than the dropped one also matches (then the child wins).
        // Dropped keys are matched on a USABLE normalized prefix (M-2):
        // the drop diagnostic promises "mints under it hold until fixed",
        // which for an unmatchable raw key like "./ops" is only honest if
        // the wrapper strips the leading-`./` noise and holds under the
        // residual prefix. A key with NO usable path form (empty, "/",
        // or any `..` segment — the latter would root the prefix above
        // the vault) legitimately holds nothing.
        if let Some(rel) = &detailed.rel_path {
            let mut deepest_dropped: Option<usize> = None;
            for dropped in &degraded.dropped_prefixes {
                let usable = usable_prefix(dropped);
                if usable.is_empty() {
                    continue;
                }
                if rel == &usable || rel.starts_with(&format!("{usable}/")) {
                    let depth = usable.split('/').count();
                    if deepest_dropped.is_none_or(|prev| depth > prev) {
                        deepest_dropped = Some(depth);
                    }
                }
            }
            if let Some(dropped_depth) = deepest_dropped {
                let deeper_valid = match &detailed.outcome {
                    PrefixOutcome::Match(_) => detailed
                        .matched
                        .map(|(_, depth)| depth > dropped_depth)
                        .unwrap_or(false),
                    _ => false,
                };
                if !deeper_valid {
                    return OntologyLookup::Hold;
                }
            }
        }

        match detailed.outcome {
            // (2) Same-normalized-prefix conflict → Hold (r18-m1). With I-3
            // scoping ties out of the global flag, this per-path Hold is
            // what keeps tied prefixes protected (unrelated paths resolve
            // normally); refuse sites consult `ontology_ties` separately.
            PrefixOutcome::Tie(_) => OntologyLookup::Hold,
            PrefixOutcome::Match(mode) => OntologyLookup::Mode(mode),
            PrefixOutcome::Unplaceable => {
                // (3) Absolute path couldn't be placed (vault moved, no
                // effective root). Hold when the map carries any off
                // entry, any tie, or any degraded/dropped state — a silent
                // climb would let heal --yes retype folders the user
                // marked off. NEVER a climb (D8).
                //
                // (3b) Same 2b gate as NoMatch below: absent default +
                // unreadable schema intent → Hold. An unplaceable path is
                // strictly LESS known than a NoMatch path (we cannot even
                // compute a vault-relative form to match against), so it
                // must hold whenever NoMatch would.
                if self.ontology_default.is_none() && schema.is_none() && schema_unparseable {
                    return OntologyLookup::Hold;
                }
                let has_off = self
                    .folder_ontology
                    .values()
                    .any(|m| *m == OntologyMode::Off);
                if has_off
                    || !crate::config::ontology_ties(self).is_empty()
                    || !degraded.dropped_prefixes.is_empty()
                    || degraded.default_dropped
                {
                    OntologyLookup::Hold
                } else {
                    OntologyLookup::Climb
                }
            }
            PrefixOutcome::NoMatch => {
                // (2a) The default scalar was dropped by salvage → Hold
                // (else `{"ontology_default":"Off"}` dropped → climbs to
                // schema strict → heal retypes an opted-out brain).
                if degraded.default_dropped {
                    return OntologyLookup::Hold;
                }
                // (2b) Absent default + unreadable schema intent → Hold.
                if self.ontology_default.is_none() && schema.is_none() && schema_unparseable {
                    return OntologyLookup::Hold;
                }
                OntologyLookup::Climb
            }
        }
    }
}

/// The degraded/failed state carried OUT of config loading, consumed by
/// [`IngestConfig::ontology_lookup`]. Empty = healthy load.
///
/// I-3: a load-time ontology TIE is deliberately NOT part of this state —
/// the plan's complete global-trigger list (plan-p11-m2) excludes ties.
/// Ties surface through [`ontology_ties`] instead (the plan's queryable
/// source of truth): resolver step (2) Holds tied prefixes per-path, and
/// refuse sites (e.g. `ct ontology set`, heal) refuse while
/// `ontology_degraded.any()` OR `!ontology_ties(..).is_empty()`.
#[derive(Debug, Clone, Default)]
pub struct OntologyDegradedState {
    /// Load-failed route (malformed JSON, non-object root, UTF-8 failure,
    /// `VaultPathNotString`, non-NotFound read error, non-object `ingest`,
    /// or a non-object `folder_ontology` value inside an otherwise valid
    /// `ingest` block). Holds EVERYTHING.
    pub global: bool,
    /// `folder_ontology` prefixes whose VALUE failed to salvage (scoped
    /// hold: only mints under them hold).
    pub dropped_prefixes: Vec<String>,
    /// The `ontology_default` scalar failed to salvage (holds every mint
    /// that would climb to rung 3).
    pub default_dropped: bool,
}

impl OntologyDegradedState {
    pub fn is_degraded(&self) -> bool {
        self.global || !self.dropped_prefixes.is_empty() || self.default_dropped
    }
}

/// The plan-p5-m1 refuse predicate (r8-m3/r17-m1): refuse while the config
/// is degraded OR carries a tie (ties hold scoped, but no ingest-mutating
/// writer may run while the map is ambiguous). Ties are NOT part of
/// [`OntologyDegradedState`] (I-3) — they are queried live from the
/// config so the flag never conflates them with load failures.
pub fn ontology_degraded_or_tied(degraded: &OntologyDegradedState, ingest: &IngestConfig) -> bool {
    degraded.is_degraded() || !ontology_ties(ingest).is_empty()
}

/// Config-level outcome of the ontology lookup (plan Task 1). Tasks 3/5
/// map `Hold` → their `HadEvidenceUnresolved` report-or-hold class;
/// `Climb` means "no directory mode decided — the §2.3 ladder continues".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OntologyLookup {
    Mode(OntologyMode),
    Hold,
    Climb,
}

/// The ingest policy in force for one brain: the tier map plus the
/// configured vault root it is relative to.
#[derive(Debug, Clone, Default)]
pub struct IngestPolicy {
    pub tiers: IngestConfig,
    pub vault_root: Option<std::path::PathBuf>,
    /// Rung-3 carrier (r6-M4): `ontology.schema` so the gate can resolve
    /// rung 3 LIVE when `ingest.ontology_default` is absent.
    pub ontology_selection: Option<crate::ontology_config::OntologySelection>,
    /// The `ontology` block was present but failed to parse — schema intent
    /// is UNKNOWN, distinct from `Some(..)`/`None`.
    pub ontology_unparseable: bool,
    /// Degraded/failed ontology config (see [`OntologyDegradedState`]).
    pub ingest_ontology_degraded: bool,
    /// `folder_ontology` prefixes dropped by salvage (scoped-hold detail).
    pub dropped_ontology_prefixes: Vec<String>,
    /// The `ontology_default` scalar was dropped by salvage.
    pub ontology_default_dropped: bool,
}

impl IngestPolicy {
    /// Tier for `path`; `vault_root` overrides the configured one when the
    /// caller knows it (the pipeline worker does).
    pub fn tier_for(&self, path: &str, vault_root: Option<&std::path::Path>) -> IngestTier {
        self.tiers
            .tier_for_path(path, vault_root.or(self.vault_root.as_deref()))
    }

    /// The degraded state consumed by [`IngestConfig::ontology_lookup`].
    pub fn ontology_degraded_state(&self) -> OntologyDegradedState {
        OntologyDegradedState {
            global: self.ingest_ontology_degraded,
            dropped_prefixes: self.dropped_ontology_prefixes.clone(),
            default_dropped: self.ontology_default_dropped,
        }
    }

    /// Resolve the gate mode for a path, threading this policy's carrier
    /// fields (vault root override, schema, degraded state).
    pub fn ontology_lookup(
        &self,
        path: &str,
        vault_root: Option<&std::path::Path>,
    ) -> OntologyLookup {
        self.tiers.ontology_lookup(
            path,
            vault_root.or(self.vault_root.as_deref()),
            &self.ontology_degraded_state(),
            self.ontology_selection,
            self.ontology_unparseable,
        )
    }
}

/// Paths whose non-NotFound config read error has already been reported.
/// I-5: `ingest_policy_for_db` runs per document on hot paths; without this
/// memo an unreadable config (EACCES, EISDIR, …) eprints once PER DOCUMENT.
/// The degraded policy return itself stays per-call — only the stderr line
/// is memoized.
static REPORTED_READ_ERRORS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
> = std::sync::OnceLock::new();

fn report_config_read_error_once(path: &std::path::Path, err: &std::io::Error) {
    let set = REPORTED_READ_ERRORS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    if let Ok(mut guard) = set.lock() {
        if !guard.insert(path.to_path_buf()) {
            return; // already reported for this path
        }
    }
    eprintln!(
        "config: could not read ingest policy ({}); degraded (ontology holds)",
        err
    );
}

type PolicyCache = std::collections::HashMap<std::path::PathBuf, (Vec<u8>, IngestPolicy)>;

/// Canonical hash input for the ontology-drift watermark (spec R2.2.8,
/// r18-MAJOR-1). `folder_ontology` is a `HashMap`, so plain serialization
/// yields different bytes per process; this normalizes keys exactly like
/// `tier_for` (`\` → `/`, trimmed `/`), sorts them (BTreeMap), and emits a
/// fixed field order with `null`/absent identical.
///
/// When `ontology_default` is ABSENT, the effective rung-3 inputs join the
/// hash (`ontology.schema` + `ontology_unparseable`) — a schema
/// Off↔strict switch must fire a drift report even with no explicit
/// ontology config. When the config is TIE-DEGRADED (r18-MAJOR-1/r20-m2),
/// ONLY the conflicting keys are excluded from the hashed map (their
/// surviving value would depend on HashMap insertion order) and
/// `"degraded": true` flips — every other key still participates, so a
/// legitimate edit elsewhere is not masked; entering or leaving
/// tie-degraded therefore always changes the hash. UNMATCHABLE keys
/// (`"./ops"`, `"/"`, empty-after-normalization) COUNT (plan-p14-m5):
/// retagging or adding/removing an inert key is a semantic config change.
/// They are hashed under the synthetic key `/raw:` + the raw key string;
/// matchable keys keep their normalized form, which can never collide
/// with that marker: `normalize_key` output never STARTS with a slash
/// (`\` → `/` replacement cannot create a leading one, and leading/trailing
/// slashes are trimmed), so a matchable key can never take the
/// `/raw:`-prefixed synthetic form reserved here for unmatchable ones.
/// (The older `raw:` marker was not collision-safe: the raw key `""` hashed
/// to exactly `raw:`, the same slot the marker form itself produced — a
/// HashMap-iteration-order winner decided which key's value survived.)
///
/// Uses the SAME `normalize_key` as the resolver, `tier_for`, and
/// `ontology_ties` — one normalization rule, no drift between them.
pub fn ontology_config_watermark_hash(
    ingest: &IngestConfig,
    schema: Option<crate::ontology_config::OntologySelection>,
    ontology_unparseable: bool,
) -> String {
    let ties = ontology_ties(ingest);
    let degraded = !ties.is_empty();
    let tied: std::collections::BTreeSet<&str> = ties.iter().map(|(k, _)| k.as_str()).collect();
    let mut map = std::collections::BTreeMap::new();
    for (k, v) in &ingest.folder_ontology {
        let value = serde_json::to_value(v).expect("OntologyMode serializes");
        if key_is_matchable(k) {
            let nk = normalize_key(k);
            // Tie-degraded encoding: exclude ONLY the conflicting keys.
            if tied.contains(&nk.as_str()) {
                continue;
            }
            // `nk` cannot start with the synthetic `/raw:` marker (see the
            // doc comment): normalized matchable keys never start with a
            // slash.
            map.insert(nk, value);
        } else {
            // Unmatchable keys COUNT (plan-p14-m5) under the synthetic
            // `/raw:` + raw-key form — collision-safe per the doc comment.
            map.insert(format!("/raw:{k}"), value);
        }
    }
    let payload = serde_json::json!({
        "degraded": degraded,
        "folder_ontology": map,
        "ontology_default": ingest.ontology_default,
        "schema": if ingest.ontology_default.is_none() {
            serde_json::to_value(schema).expect("OntologySelection serializes")
        } else {
            serde_json::Value::Null
        },
        "schema_unparseable": if ingest.ontology_default.is_none() {
            ontology_unparseable
        } else {
            false
        },
    });
    crate::hasher::hash_bytes(payload.to_string().as_bytes())
}

/// The ingest policy for the brain that owns the database at `db_path`
/// (`Connection::path()`), resolved exactly as the pipeline worker resolves
/// its config (`brain_paths_for(parent(db))`).
///
/// Consulted per document on the ingest and synthesis hot paths, so the
/// parsed policy is cached per config file, keyed on the file's exact
/// bytes: each call is one small read, and the JSON is re-parsed only when
/// the contents change. Bytes, not mtime/length — `full` ↔ `none` is a
/// same-length edit, and `BrainConfig::write`'s temp-file rename can keep a
/// coarse (1–2 s) mtime unchanged, so a metadata stamp could serve a stale
/// policy. Hand-edits take effect on the next document.
///
/// The policy is parsed from the SAME bytes that key the cache (the
/// `:read` here, not a second `load_lenient` read — a concurrent write
/// could otherwise store a policy under bytes it was never parsed from).
/// An in-memory or path-less database resolves the default policy with NO
/// degraded flags (no path = no config file to read).
///
/// Degradation rules (spec R2.2.4): a missing config (`NotFound` ONLY)
/// resolves normally — no map is no explicit ontology config. Any OTHER
/// read error, malformed JSON, a non-object root, or a non-UTF-8 file
/// yields a DEGRADED policy (`ingest_ontology_degraded = true`) — never
/// `IngestPolicy::default()`, whose all-clear fields would let the gate
/// climb to strict rungs (D8).
pub fn ingest_policy_for_db(db_path: Option<&str>) -> IngestPolicy {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<PolicyCache>> = OnceLock::new();

    let Some(brain_dir) = db_path
        .filter(|p| !p.is_empty())
        .and_then(|p| std::path::Path::new(p).parent())
    else {
        return IngestPolicy::default();
    };
    let paths = crate::retrieval::brain_paths_for(brain_dir);
    let contents = match fs::read(&paths.config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return IngestPolicy::default();
        }
        // EACCES, EISDIR, …: NOT "missing". A degraded policy keeps the
        // gate from climbing (unreadable config + heal --yes → zero
        // retypes); silently defaulting would retype opted-out folders.
        // The stderr line is memoized per path (I-5): this fn runs per
        // document, and one line per document would flood the log.
        Err(e) => {
            report_config_read_error_once(&paths.config_path, &e);
            let mut policy = IngestPolicy::default();
            policy.ingest_ontology_degraded = true;
            return policy;
        }
    };

    let cache = CACHE.get_or_init(|| Mutex::new(PolicyCache::new()));
    if let Ok(guard) = cache.lock() {
        if let Some((cached, policy)) = guard.get(&paths.config_path) {
            if *cached == contents {
                return policy.clone();
            }
        }
    }

    let policy = parse_ingest_policy_from_bytes(&contents);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(paths.config_path, (contents, policy.clone()));
    }
    policy
}

/// Parse the ingest policy from the EXACT bytes read for the cache key —
/// the same-bytes parse rule (r11-m5). Lenient parse errors degrade to a
/// flagged policy instead of the silent default.
fn parse_ingest_policy_from_bytes(contents: &[u8]) -> IngestPolicy {
    let text = match std::str::from_utf8(contents) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("config: ingest policy file is not valid UTF-8; degraded (ontology holds)");
            let mut policy = IngestPolicy::default();
            policy.ingest_ontology_degraded = true;
            return policy;
        }
    };
    let mut policy = match BrainConfig::load_lenient_from_str(text) {
        Ok(report) => {
            let degraded = report.config.ontology_degraded.clone();
            IngestPolicy {
                tiers: report.config.ingest,
                vault_root: report
                    .config
                    .vault_path
                    .clone()
                    .map(std::path::PathBuf::from),
                ontology_selection: report.config.ontology.schema,
                ontology_unparseable: report.ontology_unparseable,
                ingest_ontology_degraded: degraded.global,
                dropped_ontology_prefixes: degraded.dropped_prefixes,
                ontology_default_dropped: degraded.default_dropped,
            }
        }
        Err(e) => {
            eprintln!("config: could not parse ingest policy ({e}); degraded (ontology holds)");
            let mut policy = IngestPolicy::default();
            policy.ingest_ontology_degraded = true;
            return policy;
        }
    };
    // Ties deliberately do NOT set `ingest_ontology_degraded` (I-3: the
    // plan's complete global-trigger list, plan-p11-m2, EXCLUDES them).
    // Per-path tie holds come from the resolver's Tie→Hold arm, which this
    // global flag previously shadowed; refuse sites query
    // `ontology_ties(&policy.tiers)` directly (plan-p5-m1). The diagnostics
    // already fired inside `load_lenient_from_str`.
    policy
}

/// Unified configuration for a brain directory.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrainConfig {
    /// User's vault root path (e.g., ~/Curated-Thoughts).
    pub vault_path: Option<String>,
    /// Embedding model profile (local Ollama or external).
    pub embed_profile: Option<EmbedProfile>,
    /// Whether the vault has migrated to v2 (immutable-source-files folder structure).
    ///
    /// Deliberately NOT `#[serde(default)]`. `load()`'s strict deserialize is
    /// the gate that routes a config with a missing or malformed block to
    /// `load_lenient`, which records a diagnostic and reports the block as
    /// missing. Defaulting these fields here makes the strict parse succeed on
    /// an incomplete file, so `load()` returns a config that silently forgot
    /// the user's settings — and the next `write()` persists that loss.
    pub migrated_to_v2: bool,
    /// LLM generation config (model, provider, base_url).
    pub generation: GenerationConfig,
    /// Embedding config (model, provider, base_url).
    pub embedding: EmbeddingConfig,
    /// Privacy mode and settings.
    pub privacy: PrivacyConfig,
    /// User's ontology selection (which schema the wiki engine is seeded with).
    #[serde(default)]
    pub ontology: OntologyConfigBlock,
    /// Wiki-layer settings (deposit tier default).
    #[serde(default)]
    pub wiki: WikiConfig,
    /// Ingestion policy (F4, spec 2026-09-27-vault-ingest-policy).
    /// `#[serde(default)]` on purpose: an absent block is the shipped
    /// behavior (every folder `full`), NOT a leniency diagnostic — this
    /// block must never route a working config into `load_lenient`.
    #[serde(default)]
    pub ingest: IngestConfig,
    /// Approved symlink `(link, target)` pairs. Written only by the approval
    /// flows (`ct trust`, the Desktop review prompt) — never hand-edited.
    #[serde(default)]
    pub trusted_links: Vec<TrustedLink>,
    /// Preserved raw JSON for unknown keys (round-trip vehicle).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_keys: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside generation block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_generation: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside embedding block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_embedding: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside privacy block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_privacy: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside the ontology block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_ontology: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside the wiki block. `wiki` is in
    /// `known_keys`, so its unknown nested keys are not covered by
    /// `preserved_keys` and would be dropped by `write()` without this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_wiki: Option<serde_json::Value>,
    /// Raw generation block JSON used when typed deserialization fails.
    /// When set, write() emits this verbatim instead of the typed generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_generation: Option<serde_json::Value>,
    /// Raw embedding block JSON used when typed deserialization fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_embedding: Option<serde_json::Value>,
    /// Raw privacy block JSON used when typed deserialization fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_privacy: Option<serde_json::Value>,
    /// Unknown `ingest` sub-keys (the `preserved_wiki` pattern), captured in
    /// BOTH load arms. `IngestConfig` has no `deny_unknown_fields`, so
    /// without this `write()`'s whole-block re-serialization drops them.
    /// Unknown ingest sub-keys are ALWAYS kept — future-binary keys must
    /// survive every load/write cycle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_ingest: Option<serde_json::Value>,
    /// VERBATIM on-disk `ingest` block marker, set ONLY when salvage dropped
    /// something (degraded load). While set, `write()` LEAVES THE ON-DISK
    /// `ingest` VALUE UNTOUCHED (never re-emits a load-time captured copy —
    /// that would put the user's OLD broken block back over their
    /// in-between fix). This is the only mechanism that preserves a dropped
    /// entry INSIDE the known `folder_ontology` map. ALL ingest-mutating
    /// writers must refuse while this is set: typed mutations would be
    /// silently discarded.
    #[serde(skip)]
    pub raw_ingest: Option<serde_json::Value>,
    /// VERBATIM on-disk `ontology` block marker, set when the ontology block
    /// failed to parse. While set, `write()` leaves the on-disk `ontology`
    /// value untouched — otherwise any unrelated writer overwrites the bad
    /// value with `{"schema": null}` and the degraded state silently
    /// disappears. Cleared ONLY by `replace_ontology` (the deliberate-change
    /// escape hatch).
    #[serde(skip)]
    pub raw_ontology: Option<serde_json::Value>,
    /// Degraded ontology-config state (spec R2.2.4), set by `load_lenient`
    /// and surfaced through `ingest_policy_for_db`. `#[serde(skip)]` — this
    /// is load-time state about the file, not persisted config.
    #[serde(skip)]
    pub ontology_degraded: OntologyDegradedState,
}

impl BrainConfig {
    /// The deliberate schema-change escape hatch (r17-MAJOR-2): clears the
    /// `raw_ontology` degraded marker, sets the typed ontology block, and
    /// writes — bypassing the leave-untouched rule. ONLY
    /// `set_ontology_selection` (Desktop settings) and the onboarding merge
    /// may call this; every other writer leaves the degraded marker alone.
    pub fn replace_ontology(
        &mut self,
        block: crate::ontology_config::OntologyConfigBlock,
        paths: &BrainPaths,
    ) -> Result<()> {
        self.raw_ontology = None;
        // Belt-and-braces purge (opus confirming review m1): the loader can
        // never park `preserved_keys["ontology"]` — "ontology" is in BOTH
        // known_keys lists (`load` ~:1131 and `load_lenient_from_object`
        // ~:1390), so an `ontology` value is consumed by the typed block /
        // raw_ontology salvage, never parked as unknown. Nothing in the
        // crate inserts that key either. This purge is therefore dead code
        // for current writers; it is kept as cheap insurance against a
        // future loader regression re-parking the key, because `write()`
        // merges `preserved_keys` into the root LAST — after the typed
        // ontology block is inserted — so a stale parked value would
        // overwrite the block set here and the deliberate change would
        // silently never reach disk.
        if let Some(pk) = self.preserved_keys.as_mut() {
            if let Some(map) = pk.as_object_mut() {
                map.remove("ontology");
            }
        }
        self.ontology = block;
        self.write(paths)
    }
}

/// Report from lenient load, detailing which fields were silently defaulted.
#[derive(Debug, Clone)]
pub struct LoadReport {
    /// The successfully loaded (or partially defaulted) config.
    pub config: BrainConfig,
    /// One entry per silently-defaulted field.
    pub diagnostics: Vec<String>,
    /// True if generation block was missing and filled by leniency.
    pub generation_missing: bool,
    /// True if embedding block was missing and filled by leniency.
    pub embedding_missing: bool,
    /// True if vault_path was missing.
    pub vault_path_missing: bool,
    /// True if privacy block was missing and filled by leniency.
    pub privacy_missing: bool,
    /// True if an `ontology` block was present but failed to parse (e.g. an
    /// unrecognized `schema` value). Distinct from "absent" — callers that
    /// treat a `None` selection as "never chosen, use the desktop default"
    /// must NOT apply that fallback here, or an invalid selection like
    /// `{"ontology":{"schema":"unknown"}}` would silently start the General
    /// ontology instead of surfacing the parse failure.
    pub ontology_unparseable: bool,
}

/// Serializable summary of which config blocks were silently defaulted
/// during a lenient load.  Designed for frontend consumption so the UI can
/// show user-facing errors instead of silently falling back to defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissingBlocks {
    pub generation: bool,
    pub embedding: bool,
    pub vault_path: bool,
    pub privacy: bool,
}

/// Truncate an offending config value for a diagnostic line so a
/// hand-edited giant string cannot flood the log. Char-boundary safe.
/// Cost is O(MAX + 1) chars, not O(n): we only iterate up to `MAX + 1`
/// characters to decide whether truncation is needed (Copilot follow-up
/// on PR #147 — the earlier `chars().count()` pass walked the whole
/// string, defeating the flood-mitigation intent for huge values).
fn truncate_for_diag(value: &str) -> String {
    const MAX: usize = 120;
    let over_limit = value.chars().take(MAX + 1).count() > MAX;
    if !over_limit {
        value.to_string()
    } else {
        let mut cut: String = value.chars().take(MAX).collect();
        cut.push('…');
        cut
    }
}

impl BrainConfig {
    /// Resolve the ingest tier for a path (F4). Absolute paths are
    /// relativized against the configured `vault_path`; unmatched paths are
    /// [`IngestTier::Full`].
    pub fn ingest_tier_for(&self, path: &str) -> IngestTier {
        self.ingest
            .tier_for_path(path, self.vault_path.as_deref().map(std::path::Path::new))
    }

    /// Load config from disk with no leniency. Malformed top-level JSON is fatal.
    /// Missing or unparseable vault_path is fatal (never masked).
    /// Returns an error if config.json does not exist.
    pub fn load(paths: &BrainPaths) -> Result<BrainConfig> {
        let text = fs::read_to_string(&paths.config_path)?;
        let value: serde_json::Value = serde_json::from_str(&text)?;

        let obj = match value.as_object() {
            Some(o) => o.clone(),
            None => bail!("config.json root must be a JSON object"),
        };

        // vault_path type errors are fatal — validate before attempting deserialize.
        if let Some(vp) = obj.get("vault_path") {
            if !vp.is_string() && !vp.is_null() {
                bail!("vault_path must be a string");
            }
        }

        // Preserve unknown keys for round-trip
        let known_keys = [
            "vault_path",
            "embed_profile",
            "migrated_to_v2",
            "generation",
            "embedding",
            "privacy",
            "ontology",
            "wiki",
            "ingest",
            "trusted_links",
        ];
        let unknown_keys: serde_json::Map<String, serde_json::Value> = obj
            .iter()
            .filter(|(k, _)| !known_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let preserved_keys = if unknown_keys.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(unknown_keys))
        };

        // Capture raw blocks up-front (before re-serializing), so we can restore
        // them verbatim when serde silently defaults unknown enum variants.
        let raw_gen = obj.get("generation").cloned();
        let raw_emb = obj.get("embedding").cloned();
        let raw_priv = obj.get("privacy").cloned();

        // Re-use the already-parsed `value` — `obj` is its inner map,
        // re-serializing back to a Value just to deserialize again is wasted
        // work. The pre-validation above guarantees the root is an object
        // and vault_path is a valid string/null, so the strict deserialize
        // here is the only thing that can fail (unknown enum variants,
        // schema mismatches in typed blocks).
        match serde_json::from_value::<BrainConfig>(value) {
            Ok(mut cfg) => {
                cfg.preserved_keys = preserved_keys;

                // Extract nested unknown keys from generation block
                if let Some(gen_val) = obj.get("generation").and_then(|v| v.as_object()) {
                    let known_gen_keys = [
                        "provider",
                        "model_path",
                        "model_name",
                        "external_url",
                        "api_key",
                        "timeout_secs",
                    ];
                    let unknown: serde_json::Map<String, serde_json::Value> = gen_val
                        .iter()
                        .filter(|(k, _)| !known_gen_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_generation = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from embedding block
                if let Some(emb_val) = obj.get("embedding").and_then(|v| v.as_object()) {
                    let known_emb_keys = ["provider", "external_url"];
                    let unknown: serde_json::Map<String, serde_json::Value> = emb_val
                        .iter()
                        .filter(|(k, _)| !known_emb_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_embedding = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from privacy block
                if let Some(priv_val) = obj.get("privacy").and_then(|v| v.as_object()) {
                    let known_priv_keys = [
                        "mode",
                        "chosen",
                        "ephemeral_disclosure_acknowledged",
                        "migration_disclosure_acknowledged",
                    ];
                    let unknown: serde_json::Map<String, serde_json::Value> = priv_val
                        .iter()
                        .filter(|(k, _)| !known_priv_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_privacy = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from ontology block
                if let Some(ont_val) = obj.get("ontology").and_then(|v| v.as_object()) {
                    let known_ont_keys = ["schema"];
                    let unknown: serde_json::Map<String, serde_json::Value> = ont_val
                        .iter()
                        .filter(|(k, _)| !known_ont_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_ontology = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from wiki block
                if let Some(wiki_val) = obj.get("wiki").and_then(|v| v.as_object()) {
                    let known_wiki_keys = ["deposit_default_tier"];
                    let unknown: serde_json::Map<String, serde_json::Value> = wiki_val
                        .iter()
                        .filter(|(k, _)| !known_wiki_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_wiki = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from ingest block (r13-m2:
                // `IngestConfig` has no `deny_unknown_fields`, so unknown
                // sub-keys parse clean here and `write()`'s whole-block
                // re-serialization would drop them unless the strict arm
                // captures them too — mirroring `preserved_wiki`).
                if let Some(ing_val) = obj.get("ingest").and_then(|v| v.as_object()) {
                    let known_ingest_keys = ["folder_tiers", "folder_ontology", "ontology_default"];
                    let unknown: serde_json::Map<String, serde_json::Value> = ing_val
                        .iter()
                        .filter(|(k, _)| !known_ingest_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_ingest = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Load-time tie scans for the strict path (r19-m1: a
                // salvage-free config never runs the lenient scanner, so
                // the same scan runs here). The resolver stays silent.
                // A tie does NOT set the global degraded flag (I-3: the
                // plan's complete global-trigger list, plan-p11-m2,
                // EXCLUDES ties — a tie holds only paths under the tied
                // prefixes via the resolver's Tie→Hold arm, and refuse
                // sites consult `ontology_ties` directly, plan-p5-m1).
                // Nothing was DROPPED, so `raw_ingest` stays unset —
                // re-emitting the tied map on write is lossless.
                let (_, tie_msgs) = scan_ingest_ties(&cfg.ingest);
                for m in tie_msgs {
                    eprintln!("config: {m}");
                }
                let unmatchable = unmatchable_ontology_key_msgs(&cfg.ingest);
                for m in unmatchable {
                    eprintln!("config: {m}");
                }

                // On the typed-success path, the typed fields are authoritative.
                // Only the lenient-fallback path (below) sets `raw_*`, so callers
                // who mutate `cfg.generation` / `cfg.embedding` / `cfg.privacy`
                // and call `write()` see their mutations land on disk. Unknown
                // enum variants are no longer preserved verbatim here — that
                // behavior was a public-API footgun because it silently dropped
                // typed mutations. See PR #120 review finding.

                Ok(cfg)
            }
            Err(_e) => {
                // Strict load failed.  We pre-validated vault_path above and
                // confirmed the root is an object, so any remaining strict
                // error is from generation/embedding/privacy blocks (unknown
                // enum variants or other schema mismatches).  Fall through to
                // lenient loading to recover, and restore the original raw
                // blocks verbatim so they survive the write cycle.  JSON
                // itself already parsed successfully above, so a Result Err
                // from load_lenient here would only be the non-object-root
                // case (impossible by construction) or vault_path (already
                // pre-validated) — propagate just in case.
                let mut report = BrainConfig::load_lenient(paths)?;
                report.config.raw_generation = raw_gen;
                report.config.raw_embedding = raw_emb;
                report.config.raw_privacy = raw_priv;
                report.config.preserved_keys = preserved_keys;
                Ok(report.config)
            }
        }
    }

    /// Load config with per-field leniency from an ALREADY-READ string.
    ///
    /// Exists for the same-bytes parse rule (r11-m5): `ingest_policy_for_db`
    /// keys its cache on the exact bytes it read and must parse THOSE bytes
    /// — calling a path-based loader would re-read the file and let a
    /// concurrent write store a policy under bytes it was never parsed
    /// from. Same contract as [`Self::load_lenient`], minus the read.
    pub fn load_lenient_from_str(text: &str) -> Result<LoadReport, ConfigError> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(ConfigError::from)?;
        let obj = value
            .as_object()
            .ok_or_else(|| ConfigError::NonObjectRoot {
                actual: root_kind(&value),
            })?
            .clone();
        Self::load_lenient_from_object(obj)
    }

    /// Load config from disk with per-field leniency.
    /// Malformed top-level JSON or a non-object root is fatal and returned as
    /// `Err(ConfigError)`. Missing or unparseable fields (except `vault_path`)
    /// are dropped to defaults; a missing file is `Ok` with all `*_missing`
    /// flags set (callers decide whether missing config is a hard error).
    pub fn load_lenient(paths: &BrainPaths) -> Result<LoadReport, ConfigError> {
        let text = match fs::read_to_string(&paths.config_path) {
            Ok(t) => t,
            // The only IO condition treated as "absent configuration" is a
            // missing file — that is the normal post-onboarding state.
            // Permission-denied, a directory in the path, and any other I/O
            // failure are propagated as `ConfigError::Io` so the startup hook
            // surfaces the real failure instead of silently re-onboarding.
            // Matches the contract documented on `ConfigError` above.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut report = Self::load_lenient_from_str("{}")?;
                report
                    .diagnostics
                    .push(format!("config.json not found: {}", e));
                report.generation_missing = true;
                report.embedding_missing = true;
                report.vault_path_missing = true;
                report.privacy_missing = true;
                return Ok(report);
            }
            Err(e) => return Err(ConfigError::Io(e)),
        };

        Self::load_lenient_from_str(&text)
    }

    /// The lenient-parse core shared by the disk and from-string loaders.
    fn load_lenient_from_object(
        obj: serde_json::Map<String, serde_json::Value>,
    ) -> Result<LoadReport, ConfigError> {
        let mut report = LoadReport {
            config: BrainConfig::default(),
            diagnostics: vec![],
            generation_missing: false,
            embedding_missing: false,
            vault_path_missing: false,
            privacy_missing: false,
            ontology_unparseable: false,
        };

        // Preserve unknown keys for round-trip
        let known_keys = [
            "vault_path",
            "embed_profile",
            "migrated_to_v2",
            "generation",
            "embedding",
            "privacy",
            "ontology",
            "wiki",
            "ingest",
            "trusted_links",
        ];
        let unknown_keys: serde_json::Map<String, serde_json::Value> = obj
            .iter()
            .filter(|(k, _)| !known_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        report.config.preserved_keys = if unknown_keys.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(unknown_keys))
        };

        // Extract nested unknown keys from generation block
        if let Some(gen_val) = obj.get("generation").and_then(|v| v.as_object()) {
            let known_gen_keys = [
                "provider",
                "model_path",
                "model_name",
                "external_url",
                "api_key",
                "timeout_secs",
            ];
            let unknown: serde_json::Map<String, serde_json::Value> = gen_val
                .iter()
                .filter(|(k, _)| !known_gen_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_generation = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from embedding block
        if let Some(emb_val) = obj.get("embedding").and_then(|v| v.as_object()) {
            let known_emb_keys = ["provider", "external_url"];
            let unknown: serde_json::Map<String, serde_json::Value> = emb_val
                .iter()
                .filter(|(k, _)| !known_emb_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_embedding = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from privacy block
        if let Some(priv_val) = obj.get("privacy").and_then(|v| v.as_object()) {
            let known_priv_keys = [
                "mode",
                "chosen",
                "ephemeral_disclosure_acknowledged",
                "migration_disclosure_acknowledged",
            ];
            let unknown: serde_json::Map<String, serde_json::Value> = priv_val
                .iter()
                .filter(|(k, _)| !known_priv_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_privacy = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from ontology block
        if let Some(ont_val) = obj.get("ontology").and_then(|v| v.as_object()) {
            let known_ont_keys = ["schema"];
            let unknown: serde_json::Map<String, serde_json::Value> = ont_val
                .iter()
                .filter(|(k, _)| !known_ont_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_ontology = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from wiki block
        if let Some(wiki_val) = obj.get("wiki").and_then(|v| v.as_object()) {
            let known_wiki_keys = ["deposit_default_tier"];
            let unknown: serde_json::Map<String, serde_json::Value> = wiki_val
                .iter()
                .filter(|(k, _)| !known_wiki_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_wiki = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // vault_path: hard error if present but not a string
        if let Some(vp) = obj.get("vault_path") {
            match vp.as_str() {
                Some(s) => report.config.vault_path = Some(s.to_string()),
                None if !vp.is_null() => return Err(ConfigError::VaultPathNotString),
                _ => report.vault_path_missing = true,
            }
        } else {
            report.vault_path_missing = true;
        }

        // embed_profile: lenient, drops unparseable variants
        if let Some(ep) = obj.get("embed_profile") {
            match serde_json::from_value::<EmbedProfile>(ep.clone()) {
                Ok(p) => report.config.embed_profile = Some(p),
                Err(_) => {
                    report
                        .diagnostics
                        .push("embed_profile unparseable, using default".to_string());
                }
            }
        }

        // migrated_to_v2: lenient
        if let Some(m) = obj.get("migrated_to_v2") {
            if let Some(b) = m.as_bool() {
                report.config.migrated_to_v2 = b;
            } else {
                report
                    .diagnostics
                    .push("migrated_to_v2 not a bool, using false".to_string());
            }
        }

        // generation: complete block must round-trip, or missing
        if let Some(gen) = obj.get("generation") {
            match serde_json::from_value::<GenerationConfig>(gen.clone()) {
                Ok(g) => report.config.generation = g,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("generation block unparseable: {}", e));
                    report.generation_missing = true;
                }
            }
        } else {
            report.generation_missing = true;
        }

        // embedding: complete block must round-trip, or missing
        if let Some(emb) = obj.get("embedding") {
            match serde_json::from_value::<EmbeddingConfig>(emb.clone()) {
                Ok(e) => report.config.embedding = e,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("embedding block unparseable: {}", e));
                    report.embedding_missing = true;
                }
            }
        } else {
            report.embedding_missing = true;
        }

        // privacy: complete block must round-trip, or missing
        if let Some(priv_) = obj.get("privacy") {
            match serde_json::from_value::<PrivacyConfig>(priv_.clone()) {
                Ok(p) => report.config.privacy = p,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("privacy block unparseable: {}", e));
                    report.privacy_missing = true;
                }
            }
        } else {
            report.privacy_missing = true;
        }

        // ontology: an unparseable block is NOT the same as "never chosen" —
        // it must not silently fall back to the desktop default (that would
        // start an ontology the user never selected). The typed block is
        // left at default, `ontology_unparseable` flags it for rung 3
        // (schema intent UNKNOWN → holds, never a silent climb to strict),
        // and `raw_ontology` marks the degraded state so `write()` leaves
        // the on-disk block untouched (otherwise any unrelated writer
        // overwrites the bad value with `{"schema": null}` and the next
        // load silently un-degrades). NOTE (opus re-review m2): non-object
        // `ontology` values are deliberately NOT parked into
        // `preserved_keys` here — with `raw_ontology` set, `write()`
        // already leaves the on-disk `ontology` value untouched (it starts
        // from the file it read), so parking would round-trip a value that
        // the write guard preserves anyway. Only the deliberate-change
        // escape hatch (`replace_ontology`) writes past that guard, and it
        // still purges any legacy parked key defensively (older builds
        // parked one).
        //
        // Note: an unknown `schema` VARIANT fails the WHOLE block
        // deserialize — values do NOT "load as None"; the comment on
        // `OntologyConfigBlock.schema` in ontology_config.rs says exactly
        // this now.
        if let Some(ont) = obj.get("ontology") {
            match serde_json::from_value::<OntologyConfigBlock>(ont.clone()) {
                Ok(o) => report.config.ontology = o,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("ontology block unparseable: {}", e));
                    report.ontology_unparseable = true;
                    report.config.raw_ontology = Some(ont.clone());
                }
            }
        }

        // wiki: lenient. `wiki` is in `known_keys`, so it is excluded from
        // `preserved_keys` — without this branch a config that falls through
        // from the strict path (e.g. an unknown `generation.provider` variant)
        // would silently lose `deposit_default_tier` and every deposit would
        // be classified at the shipped default instead of the configured one.
        if let Some(w) = obj.get("wiki") {
            match serde_json::from_value::<WikiConfig>(w.clone()) {
                Ok(cfg) => report.config.wiki = cfg,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("wiki block unparseable: {}", e));
                }
            }
        }

        // trusted_links: lenient — an unparseable entry is dropped, the rest
        // survive. This is the only mutable-from-config surface for the
        // ledger; a corruption in one entry must not nuke the whole list.
        //
        // ORDER (I-1): this block runs BEFORE the `ingest` block below.
        // The non-object-`ingest` early-return skips everything after it,
        // so trusted_links parsing must already have happened — otherwise
        // the config loads with the empty default ledger and a subsequent
        // `write()` re-serializes `trusted_links: []`, erasing the
        // on-disk approvals. Block processing order is otherwise free.
        //
        // Beyond JSON validity, each entry's `link` must be vault-relative
        // (issue #140). `TrustedLink::link` feeds the walker's
        // `vault_root.join(link)`, and `Path::join` replaces the base on an
        // absolute/rooted argument, so a hand-edited ledger must not smuggle
        // one past the approval write path's guard (PR #144). Same predicate
        // as the write path — one rule, two boundaries. Non-conforming
        // entries are dropped with a diagnostic (fail-closed: the symlink
        // reverts to `Pending` and is never followed), matching the block's
        // existing drop-one-keep-the-rest semantics.
        if let Some(tl) = obj.get("trusted_links").and_then(|v| v.as_array()) {
            let mut kept = Vec::with_capacity(tl.len());
            for entry in tl {
                match serde_json::from_value::<TrustedLink>(entry.clone()) {
                    Ok(e) => {
                        if crate::trusted_links::is_vault_relative_link(&e.link) {
                            kept.push(e);
                        } else {
                            report.diagnostics.push(format!(
                                "trusted_links entry rejected: link {:?} is not vault-relative (absolute, rooted, or contains `..`)",
                                truncate_for_diag(&e.link)
                            ));
                        }
                    }
                    Err(err) => report
                        .diagnostics
                        .push(format!("trusted_links entry unparseable: {}", err)),
                }
            }
            report.config.trusted_links = kept;
        }

        // ingest: lenient (F4 + R2.2.4). Same known_keys reasoning as
        // `wiki`: the block is modeled, so it must not leak into
        // `preserved_keys`. Each modeled key (`folder_tiers`,
        // `folder_ontology`, `ontology_default`) salvages INDEPENDENTLY,
        // present-or-not — a bad tier value must not erase or disable
        // `folder_ontology` entries, and a typical config has no
        // `folder_tiers` at all (the old "all folders full" branch fired
        // for it and would have degraded EVERY mint on the brain). A bad
        // entry is DROPPED (with a diagnostic + the matching degraded
        // skip-field), never salvaged into a legal value: `{"x":"Off"}`
        // must not become strict, and a dropped `off` entry must not
        // silently climb. Unknown sub-keys go to `preserved_ingest` (kept
        // on write). A NON-OBJECT `ingest` value OR a non-object
        // `folder_ontology` value degrades globally (`raw_ingest` set →
        // write leaves the on-disk block untouched).
        // Note the `ingest` BLOCK is `#[serde(default)]` on BrainConfig, so
        // an absent block never enters this branch — no diagnostic.
        if let Some(ing) = obj.get("ingest") {
            let Some(ing_obj) = ing.as_object() else {
                report.diagnostics.push(format!(
                    "ingest block unparseable ({}); ontology config holds, tiers full",
                    root_kind(ing)
                ));
                report.config.raw_ingest = Some(ing.clone());
                report.config.ontology_degraded.global = true;
                // I-1: this return is safe BECAUSE `ingest` is now the LAST
                // processed block — the trusted_links salvage block runs
                // ABOVE it (the I-1 fix moved it there), so the loaded
                // config carries the on-disk ledger and a subsequent
                // write() re-serializes it faithfully. (The pre-I-1 early
                // return here skipped trusted_links parsing below, wiping
                // the ledger on the next write.)
                return Ok(report);
            };
            // Unknown sub-keys → preserved_ingest (always kept).
            let known_ingest_keys = ["folder_tiers", "folder_ontology", "ontology_default"];
            let unknown: serde_json::Map<String, serde_json::Value> = ing_obj
                .iter()
                .filter(|(k, _)| !known_ingest_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_ingest = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };

            if let Some(ft) = ing_obj.get("folder_tiers") {
                match serde_json::from_value::<std::collections::HashMap<String, IngestTier>>(
                    ft.clone(),
                ) {
                    Ok(m) => report.config.ingest.folder_tiers = m,
                    Err(_) => {
                        // Entry-by-entry salvage; drop only bad entries.
                        if let Some(map) = ft.as_object() {
                            for (k, v) in map {
                                match serde_json::from_value::<IngestTier>(v.clone()) {
                                    Ok(tier) => {
                                        report.config.ingest.folder_tiers.insert(k.clone(), tier);
                                    }
                                    Err(e) => report.diagnostics.push(format!(
                                        "ingest.folder_tiers entry {k:?} dropped: {e}"
                                    )),
                                }
                            }
                        } else {
                            report.diagnostics.push(
                                "ingest.folder_tiers unparseable (not an object); tiers full"
                                    .to_string(),
                            );
                        }
                    }
                }
            }

            if let Some(fo) = ing_obj.get("folder_ontology") {
                match serde_json::from_value::<std::collections::HashMap<String, OntologyMode>>(
                    fo.clone(),
                ) {
                    Ok(m) => report.config.ingest.folder_ontology = m,
                    Err(_) => {
                        if let Some(map) = fo.as_object() {
                            for (k, v) in map {
                                match serde_json::from_value::<OntologyMode>(v.clone()) {
                                    Ok(mode) => {
                                        report
                                            .config
                                            .ingest
                                            .folder_ontology
                                            .insert(k.clone(), mode);
                                    }
                                    Err(e) => {
                                        // r15-m1: salvage dropped something →
                                        // raw_ingest set so write() leaves the
                                        // ON-DISK block (bad entry included)
                                        // untouched — the user's hand fix must
                                        // not be overwritten out from under
                                        // them, and the dropped `off` entry
                                        // must not be silently erased.
                                        report.config.raw_ingest = Some(ing.clone());
                                        report
                                            .config
                                            .ontology_degraded
                                            .dropped_prefixes
                                            .push(normalize_key(k));
                                        report.diagnostics.push(format!(
                                            "ingest.folder_ontology entry {k:?} dropped: {e}; mints under it hold until fixed"
                                        ));
                                    }
                                }
                            }
                        } else {
                            report.diagnostics.push(
                                "ingest.folder_ontology unparseable (not an object); ontology config holds"
                                    .to_string(),
                            );
                            report.config.raw_ingest = Some(ing.clone());
                            report.config.ontology_degraded.global = true;
                        }
                    }
                }
            }

            if let Some(od) = ing_obj.get("ontology_default") {
                if od.is_null() {
                    // A hand-written null parses as None = absent (r19-m3).
                } else {
                    match serde_json::from_value::<OntologyMode>(od.clone()) {
                        Ok(mode) => report.config.ingest.ontology_default = Some(mode),
                        Err(e) => {
                            // r15-m1: salvage dropped something → raw_ingest
                            // set (same leave-untouched write rule as a
                            // dropped folder_ontology entry).
                            report.config.raw_ingest = Some(ing.clone());
                            report.config.ontology_degraded.default_dropped = true;
                            report.diagnostics.push(format!(
                                "ingest.ontology_default dropped: {e}; mints that climb to the default hold until fixed"
                            ));
                        }
                    }
                }
            }

            // Load-time tie scans + unmatchable-key diagnostics (the
            // resolver itself is silent — strict-parse configs reach these
            // only here). A tie does NOT set the global degraded flag
            // (I-3: the plan's complete global-trigger list, plan-p11-m2,
            // EXCLUDES ties — the tied prefixes hold via the resolver's
            // Tie→Hold arm (2), unrelated paths resolve normally, and
            // refuse sites consult `ontology_ties` directly, plan-p5-m1).
            let (_, tie_msgs) = scan_ingest_ties(&report.config.ingest);
            report.diagnostics.extend(tie_msgs);
            report
                .diagnostics
                .extend(unmatchable_ontology_key_msgs(&report.config.ingest));
        }

        Ok(report)
    }

    /// Write config to disk using raw-document merge (preserves unknown keys).
    /// - Reads existing JSON as Value tree.
    /// - Overlays modeled sections (generation, embedding, privacy, etc.).
    /// - Writes temp file with unique name, syncs, then renames.
    /// - Malformed existing JSON is an error; file left untouched.
    pub fn write(&self, paths: &BrainPaths) -> Result<()> {
        // Read existing document, if it exists.
        let mut root = if paths.config_path.exists() {
            let text = fs::read_to_string(&paths.config_path)?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("malformed config.json: {}", e))?;

            if !value.is_object() {
                bail!("config.json root must be a JSON object");
            }
            value
        } else {
            serde_json::json!({})
        };

        // Ensure root is an object (checked above, but be explicit for overlay).
        let obj = root.as_object_mut().unwrap();

        // Build modeled sections as Values, then merge preserved nested keys into them
        // before inserting into the root object.
        //
        // If a block failed to deserialize (captured in raw_*), use that verbatim
        // so the unparseable block is preserved unchanged through the write cycle.

        // Generation section
        let gen_value = if let Some(ref raw) = self.raw_generation {
            raw.clone()
        } else {
            let mut gen_value = serde_json::to_value(&self.generation)?;
            if let Some(ref preserved) = self.preserved_generation {
                if let Some(gen_obj) = gen_value.as_object_mut() {
                    if let Some(preserved_obj) = preserved.as_object() {
                        for (k, v) in preserved_obj {
                            gen_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            gen_value
        };

        // Embedding section
        let emb_value = if let Some(ref raw) = self.raw_embedding {
            raw.clone()
        } else {
            let mut emb_value = serde_json::to_value(&self.embedding)?;
            if let Some(ref preserved) = self.preserved_embedding {
                if let Some(emb_obj) = emb_value.as_object_mut() {
                    if let Some(preserved_obj) = preserved.as_object() {
                        for (k, v) in preserved_obj {
                            emb_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            emb_value
        };

        // Privacy section
        let priv_value = if let Some(ref raw) = self.raw_privacy {
            raw.clone()
        } else {
            let mut priv_value = serde_json::to_value(&self.privacy)?;
            if let Some(ref preserved) = self.preserved_privacy {
                if let Some(priv_obj) = priv_value.as_object_mut() {
                    if let Some(preserved_obj) = preserved.as_object() {
                        for (k, v) in preserved_obj {
                            priv_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            priv_value
        };

        // Ontology section. When the ontology block failed to parse
        // (`raw_ontology` set), leave the ON-DISK `ontology` value in
        // `root` untouched: the typed block is a default and re-emitting it
        // would overwrite the user's bad-but-mine value with
        // `{"schema": null}`, silently clearing the degraded state. Only
        // `replace_ontology` (the deliberate-change escape hatch) writes a
        // typed ontology block past this guard.
        if self.raw_ontology.is_none() {
            let mut ont_value = serde_json::to_value(&self.ontology)?;
            if let Some(ref preserved) = self.preserved_ontology {
                if let (Some(ont_obj), Some(preserved_obj)) =
                    (ont_value.as_object_mut(), preserved.as_object())
                {
                    for (k, v) in preserved_obj {
                        ont_obj.insert(k.clone(), v.clone());
                    }
                }
            }
            obj.insert("ontology".to_string(), ont_value);
        }

        // Insert modeled sections with preserved nested keys merged in.
        obj.insert(
            "vault_path".to_string(),
            serde_json::to_value(&self.vault_path)?,
        );
        obj.insert(
            "embed_profile".to_string(),
            serde_json::to_value(&self.embed_profile)?,
        );
        obj.insert(
            "migrated_to_v2".to_string(),
            serde_json::to_value(self.migrated_to_v2)?,
        );
        obj.insert("generation".to_string(), gen_value);
        obj.insert("embedding".to_string(), emb_value);
        obj.insert("privacy".to_string(), priv_value);
        let mut wiki_value = serde_json::to_value(&self.wiki)?;
        if let Some(ref preserved) = self.preserved_wiki {
            if let (Some(wiki_obj), Some(preserved_obj)) =
                (wiki_value.as_object_mut(), preserved.as_object())
            {
                for (k, v) in preserved_obj {
                    wiki_obj.insert(k.clone(), v.clone());
                }
            }
        }
        obj.insert("wiki".to_string(), wiki_value);
        // F4/R2.2.4: persist the ingest-policy block (empty maps serialize
        // as `{}`, keeping the block visible and hand-editable) — UNLESS the
        // load was degraded (`raw_ingest` set): then leave the ON-DISK
        // `ingest` value in `root` untouched. Typed mutations made while
        // degraded would be silently discarded, which is exactly why every
        // ingest-mutating writer must REFUSE while degraded (r8-m3) — the
        // guard here is the backstop, and callers check the flag first.
        if self.raw_ingest.is_none() {
            let mut ingest_value = serde_json::to_value(&self.ingest)?;
            if let (Some(ingest_obj), Some(preserved_obj)) = (
                ingest_value.as_object_mut(),
                self.preserved_ingest.as_ref().and_then(|v| v.as_object()),
            ) {
                for (k, v) in preserved_obj {
                    ingest_obj.insert(k.clone(), v.clone());
                }
            }
            obj.insert("ingest".to_string(), ingest_value);
        }
        obj.insert(
            "trusted_links".to_string(),
            serde_json::to_value(&self.trusted_links)?,
        );

        // Merge preserved top-level keys back in.
        if let Some(ref preserved) = self.preserved_keys {
            if let Some(preserved_obj) = preserved.as_object() {
                for (k, v) in preserved_obj {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        // Write to temp file with unique name.
        let nonce = Uuid::new_v4();
        let pid = std::process::id();
        let tmp_name = format!("config.json.{}.{}.tmp", pid, nonce);
        let parent = paths
            .config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        // A fresh install has no brain dir yet — create it so the first write
        // (e.g. from --onboard) doesn't fail with ENOENT.
        fs::create_dir_all(parent)?;
        let tmp_path = parent.join(&tmp_name);

        let json = serde_json::to_string_pretty(&root)?;

        // Write and sync before rename.
        {
            let mut file = std::fs::File::create(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_data()?;
        }

        // Atomic rename.
        fs::rename(&tmp_path, &paths.config_path)?;

        Ok(())
    }

    /// The configured deposit tier, read from the brain config on disk.
    ///
    /// Convenience for commit-path callers, which have a `Connection` but no
    /// `BrainConfig`. Any load failure degrades to the shipped default rather
    /// than failing the commit — a deposit landing at the default tier is a
    /// working state, a dropped proposal is not.
    pub fn deposit_default_tier_on_disk() -> String {
        let paths = crate::retrieval::resolve_brain_paths();
        match BrainConfig::load_lenient(&paths) {
            Ok(report) => report.config.deposit_default_tier().to_string(),
            Err(e) => {
                eprintln!(
                    "config: could not read wiki.deposit_default_tier ({e}); using {DEFAULT_DEPOSIT_TIER:?}"
                );
                DEFAULT_DEPOSIT_TIER.to_string()
            }
        }
    }

    /// The tier stamped on deposit-ingested entries (spec §3.2).
    ///
    /// Defaults to `"wisdom"`: deposits are agent-written and revisable, and
    /// `"fact"` invokes anchor-truth freeze semantics on agents that routinely
    /// revise. An out-of-vocabulary value falls back rather than reaching the
    /// DB and tripping the V16 CHECK — config is hand-editable.
    pub fn deposit_default_tier(&self) -> &str {
        match self.wiki.deposit_default_tier.as_deref() {
            Some(t) if crate::db::schema::is_valid_tier(t) => {
                // Reborrow from the field so the returned lifetime is `&self`,
                // not the temporary `as_deref` binding.
                self.wiki
                    .deposit_default_tier
                    .as_deref()
                    .unwrap_or(DEFAULT_DEPOSIT_TIER)
            }
            Some(other) => {
                eprintln!(
                    "config: wiki.deposit_default_tier {other:?} is not one of {:?}; using {DEFAULT_DEPOSIT_TIER:?}",
                    crate::db::schema::VALID_TIERS
                );
                DEFAULT_DEPOSIT_TIER
            }
            None => DEFAULT_DEPOSIT_TIER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config carrying only a wiki block. Built from `Default` rather than
    /// `from_str("{}")` on purpose: `BrainConfig`'s typed blocks deliberately
    /// have no `#[serde(default)]`, because that is the gate routing an
    /// incomplete config to `load_lenient` instead of silently accepting it.
    fn cfg_with_deposit_tier(tier: Option<&str>) -> BrainConfig {
        BrainConfig {
            wiki: WikiConfig {
                deposit_default_tier: tier.map(str::to_string),
            },
            ..Default::default()
        }
    }

    #[test]
    fn deposit_default_tier_defaults_to_wisdom() {
        // Deposits are agent-written notes under active revision. 'fact' would
        // invoke the librarian's "do not propose modifications" framing and
        // freeze exactly the content agents keep correcting (spec §3.2).
        assert_eq!(cfg_with_deposit_tier(None).deposit_default_tier(), "wisdom");
    }

    #[test]
    fn deposit_default_tier_can_be_set_to_fact() {
        assert_eq!(
            cfg_with_deposit_tier(Some("fact")).deposit_default_tier(),
            "fact"
        );
    }

    #[test]
    fn invalid_deposit_default_tier_falls_back_to_wisdom() {
        // Config is hand-editable; an out-of-vocabulary value must not reach
        // the DB and trip the V16 CHECK.
        assert_eq!(
            cfg_with_deposit_tier(Some("anchor")).deposit_default_tier(),
            "wisdom"
        );
    }

    /// The regression guard for the strict-vs-lenient asymmetry. A config
    /// missing a typed block must NOT deserialize strictly — that failure is
    /// what routes `load()` into `load_lenient`, which records a diagnostic
    /// and flags the block as missing instead of silently forgetting it.
    #[test]
    fn strict_deserialize_rejects_a_config_missing_typed_blocks() {
        assert!(
            serde_json::from_str::<BrainConfig>("{}").is_err(),
            "an empty config must fail strict deserialize, not default silently"
        );
        assert!(
            serde_json::from_str::<BrainConfig>(r#"{"wiki":{"deposit_default_tier":"fact"}}"#)
                .is_err(),
            "a config with only a wiki block is still missing generation/embedding/privacy"
        );
    }

    // ---- F4 ingest tiers (spec 2026-09-27-vault-ingest-policy) ----

    #[test]
    fn ingest_block_parses_tiers_and_defaults_to_full() {
        let cfg: BrainConfig = serde_json::from_str(
            r#"{"vault_path":"/v","migrated_to_v2":false,"generation":{"provider":"unconfigured","model_name":null,"model_path":null,"external_url":null,"api_key":null,"timeout_secs":null},"embedding":{"provider":"fastembed","external_url":null},"privacy":{"mode":"strict","chosen":true,"ephemeral_disclosure_acknowledged":true,"migration_disclosure_acknowledged":true},
                "ingest":{"folder_tiers":{"operations":"chunks-only","people/tessera/sessions":"none"}}}"#,
        )
        .unwrap();
        assert_eq!(
            cfg.ingest.folder_tiers.get("operations"),
            Some(&IngestTier::ChunksOnly)
        );
        assert_eq!(
            cfg.ingest.folder_tiers.get("people/tessera/sessions"),
            Some(&IngestTier::None)
        );
        // Absent key = full ingestion.
        assert_eq!(cfg.ingest_tier_for("wiki/some-note.md"), IngestTier::Full);
    }

    /// A config with NO ingest block at all resolves every path to `full`
    /// (the universal pre-F4 behavior) and round-trips through write.
    #[test]
    fn absent_ingest_block_means_full_everywhere() {
        let cfg: BrainConfig = serde_json::from_str(
            r#"{"vault_path":"/v","migrated_to_v2":false,"generation":{"provider":"unconfigured","model_name":null,"model_path":null,"external_url":null,"api_key":null,"timeout_secs":null},"embedding":{"provider":"fastembed","external_url":null},"privacy":{"mode":"strict","chosen":true,"ephemeral_disclosure_acknowledged":true,"migration_disclosure_acknowledged":true}}"#,
        )
        .unwrap();
        assert!(cfg.ingest.folder_tiers.is_empty());
        assert_eq!(cfg.ingest_tier_for("anything/at/all.md"), IngestTier::Full);
    }

    /// Longest matching prefix wins; matching is PATH-COMPONENT aware:
    /// a tier keyed `ops` must not capture `ops-archive/…` (review
    /// amendment b). Separators are normalized so Windows spellings match.
    #[test]
    fn longest_component_aware_prefix_wins() {
        let cfg: BrainConfig = serde_json::from_str(
            r#"{"vault_path":"/v","migrated_to_v2":false,"generation":{"provider":"unconfigured","model_name":null,"model_path":null,"external_url":null,"api_key":null,"timeout_secs":null},"embedding":{"provider":"fastembed","external_url":null},"privacy":{"mode":"strict","chosen":true,"ephemeral_disclosure_acknowledged":true,"migration_disclosure_acknowledged":true},
                "ingest":{"folder_tiers":{"ops":"none","ops-archive":"chunks-only","people/tessera":"chunks-only","people/tessera/sessions":"none"}}}"#,
        )
        .unwrap();
        // Sibling that merely SHARES a string prefix must NOT match `ops`.
        assert_eq!(
            cfg.ingest_tier_for("ops-archive/a.md"),
            IngestTier::ChunksOnly
        );
        // Longest prefix wins over a shorter one.
        assert_eq!(
            cfg.ingest_tier_for("people/tessera/sessions/s1.md"),
            IngestTier::None
        );
        assert_eq!(
            cfg.ingest_tier_for("people/tessera/notes.md"),
            IngestTier::ChunksOnly
        );
        assert_eq!(cfg.ingest_tier_for("people/other.md"), IngestTier::Full);
        assert_eq!(cfg.ingest_tier_for("ops/x.md"), IngestTier::None);
        // Backslash spellings normalize onto the `/`-keyed tiers.
        assert_eq!(
            cfg.ingest_tier_for("people\\tessera\\notes.md"),
            IngestTier::ChunksOnly
        );
    }

    /// An unparseable tier VALUE falls back to full (config is hand-editable;
    /// garbage must not disable ingestion of a whole tree). Driven through
    /// `load_lenient_from_str` via a tempdir fixture (r20-m5 — the deleted
    /// `ingest_from_value_lenient` raw-Value surface had no production
    /// caller; the salvage lives inside the lenient loader).
    #[test]
    fn unparseable_tier_value_falls_back_to_full() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = BrainPaths {
            brain_dir: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.json"),
            db_path: tmp.path().join("brain.db"),
        };
        std::fs::write(
            &paths.config_path,
            r#"{"ingest":{"folder_tiers":{"operations":"bogus-tier","notes":"chunks-only"}}}"#,
        )
        .unwrap();
        let report = BrainConfig::load_lenient(&paths).unwrap();
        let cfg = &report.config.ingest;
        // The bogus entry is dropped; the valid one survives.
        assert!(!cfg.folder_tiers.contains_key("operations"));
        assert_eq!(cfg.folder_tiers.get("notes"), Some(&IngestTier::ChunksOnly));
        assert_eq!(cfg.tier_for("operations/a.md"), IngestTier::Full);
    }

    fn tiers(entries: &[(&str, IngestTier)]) -> IngestConfig {
        IngestConfig {
            folder_tiers: entries.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    /// A hand-edited key spelled with `\` separators matches like its `/`
    /// spelling — normalization is symmetric, not path-only.
    #[test]
    fn backslash_configured_key_matches() {
        let cfg = tiers(&[("people\\tessera\\sessions", IngestTier::None)]);
        assert_eq!(
            cfg.tier_for("people/tessera/sessions/s.md"),
            IngestTier::None
        );
        assert_eq!(
            cfg.tier_for("people\\tessera\\sessions\\s.md"),
            IngestTier::None
        );
        assert_eq!(cfg.tier_for("people/tessera/notes.md"), IngestTier::Full);
    }

    /// Prefixes anchor at the VAULT ROOT: a vault whose own ancestors share a
    /// tier key's name, and a same-named folder nested deeper in the vault,
    /// must both stay `full`.
    #[test]
    fn prefixes_anchor_at_vault_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("operations").join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let cfg = tiers(&[("operations", IngestTier::ChunksOnly)]);
        let under = |rel: &str| root.join(rel).to_string_lossy().into_owned();

        // Ancestor `operations/` outside the vault does not match.
        assert_eq!(
            cfg.tier_for_path(&under("documents/note.md"), Some(&root)),
            IngestTier::Full
        );
        // Nested `…/operations/` deeper in the vault does not match.
        assert_eq!(
            cfg.tier_for_path(&under("notes/operations/x.md"), Some(&root)),
            IngestTier::Full
        );
        // The vault-root `operations/` folder does.
        assert_eq!(
            cfg.tier_for_path(&under("operations/brief.md"), Some(&root)),
            IngestTier::ChunksOnly
        );
        // Absolute path with no known root: shipped behavior (full).
        assert_eq!(
            cfg.tier_for_path(&under("operations/brief.md"), None),
            IngestTier::Full
        );
    }

    /// The policy loader follows the db's brain dir, sees hand-edits on the
    /// next call (content-keyed cache), and defaults to full for in-memory dbs.
    #[test]
    fn ingest_policy_for_db_reads_and_refreshes() {
        let brain = tempfile::TempDir::new().unwrap();
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
            ],
            || {
                let db = brain.path().join("brain.db");
                let db = db.to_str().unwrap();
                let cfg_path = brain.path().join("config.json");
                assert!(ingest_policy_for_db(Some(db)).tiers.folder_tiers.is_empty());

                std::fs::write(
                    &cfg_path,
                    r#"{"vault_path":"/v","ingest":{"folder_tiers":{"ops":"none"}}}"#,
                )
                .unwrap();
                let p = ingest_policy_for_db(Some(db));
                assert_eq!(p.tier_for("/v/ops/a.md", None), IngestTier::None);

                // SAME-length edit (`none` → `full`), with the mtime pinned
                // back: a metadata-keyed cache would serve the stale `none`.
                let mtime = std::fs::metadata(&cfg_path).unwrap().modified().unwrap();
                std::fs::write(
                    &cfg_path,
                    r#"{"vault_path":"/v","ingest":{"folder_tiers":{"ops":"full"}}}"#,
                )
                .unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&cfg_path)
                    .unwrap()
                    .set_modified(mtime)
                    .unwrap();
                let p = ingest_policy_for_db(Some(db));
                assert_eq!(p.tier_for("ops/a.md", None), IngestTier::Full);

                assert!(ingest_policy_for_db(None).tiers.folder_tiers.is_empty());
                assert!(ingest_policy_for_db(Some("")).tiers.folder_tiers.is_empty());
            },
        );
    }

    // ── R2.2.x config-core tests (Task 1) ──────────────────────────────────

    use crate::ontology_config::OntologySelection;

    /// Write a config fixture into a tempdir brain and return its paths.
    fn degraded_fixture_dir() -> (tempfile::TempDir, BrainPaths) {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = BrainPaths {
            brain_dir: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.json"),
            db_path: tmp.path().join("brain.db"),
        };
        (tmp, paths)
    }

    fn write_cfg(paths: &BrainPaths, json: &str) {
        std::fs::write(&paths.config_path, json).unwrap();
    }

    const BASE_CFG: &str = r#"{"vault_path":"/v","migrated_to_v2":false,"generation":{"provider":"unconfigured","model_name":null,"model_path":null,"external_url":null,"api_key":null,"timeout_secs":null},"embedding":{"provider":"fastembed","external_url":null},"privacy":{"mode":"strict","chosen":true,"ephemeral_disclosure_acknowledged":true,"migration_disclosure_acknowledged":true}}"#;

    /// R2.2.1/R2.2.2: the four-state resolver mapping — off/strict match,
    /// no-match climb, and empty map short-circuit.
    #[test]
    fn folder_ontology_resolution_modes() {
        let mut cfg = IngestConfig::default();
        cfg.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        cfg.folder_ontology
            .insert("people".to_string(), OntologyMode::Strict);

        assert_eq!(
            cfg.ontology_lookup(
                "ops/a.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Mode(OntologyMode::Off)
        );
        assert_eq!(
            cfg.ontology_lookup(
                "ops/sub/deep.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Mode(OntologyMode::Off)
        );
        assert_eq!(
            cfg.ontology_lookup(
                "people/x.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Mode(OntologyMode::Strict)
        );
        // No match → climb (no off entries, healthy).
        assert_eq!(
            cfg.ontology_lookup(
                "other/x.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Climb
        );
        // `ops` must not capture sibling `ops-archive` (component boundary).
        assert_eq!(
            cfg.ontology_lookup(
                "ops-archive/x.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Climb
        );
        // Empty map short-circuits (climb), mirroring `tier_for_path`.
        assert_eq!(
            IngestConfig::default().ontology_lookup(
                "ops/a.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Climb
        );
    }

    /// Opus tier-3 MAJOR-2: the Unplaceable branch (absolute path, no vault
    /// root) must run the SAME 2b gate as NoMatch — absent default +
    /// unreadable schema intent → Hold. An unplaceable path is strictly
    /// LESS known than a NoMatch path; the pre-fix code Climbed here.
    #[test]
    fn unplaceable_path_holds_when_schema_intent_unparseable() {
        let cfg = IngestConfig::default();
        // Absolute path + no vault root → Unplaceable.
        assert_eq!(
            cfg.ontology_lookup(
                "/elsewhere/x.md",
                None,
                &OntologyDegradedState::default(),
                None,
                true
            ),
            OntologyLookup::Hold,
            "unplaceable + absent default + unparseable schema must Hold (pre-fix: Climbed)"
        );
        // Control: parseable schema intent on the same unplaceable path
        // still climbs (nothing to hold on).
        assert_eq!(
            cfg.ontology_lookup(
                "/elsewhere/x.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Climb
        );
        // Parity with NoMatch: a relative path under the same degraded
        // inputs also holds (the 2b check the branch mirrors).
        assert_eq!(
            cfg.ontology_lookup(
                "elsewhere/x.md",
                None,
                &OntologyDegradedState::default(),
                None,
                true
            ),
            OntologyLookup::Hold
        );
    }

    /// D8: every degraded/off state resolves to Hold, never a climb.
    #[test]
    fn degraded_states_hold_never_climb() {
        let mut cfg = IngestConfig::default();
        cfg.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);

        // (0) global degraded → Hold everywhere.
        assert_eq!(
            cfg.ontology_lookup(
                "ops/a.md",
                None,
                &OntologyDegradedState {
                    global: true,
                    ..Default::default()
                },
                None,
                false
            ),
            OntologyLookup::Hold
        );

        // (1) dropped prefix → Hold under it; a deeper VALID child wins.
        let dropped = OntologyDegradedState {
            dropped_prefixes: vec!["ops".to_string()],
            ..Default::default()
        };
        assert_eq!(
            cfg.ontology_lookup("ops/a.md", None, &dropped, None, false),
            OntologyLookup::Hold
        );
        let mut child = IngestConfig::default();
        child
            .folder_ontology
            .insert("ops/sub".to_string(), OntologyMode::Strict);
        assert_eq!(
            child.ontology_lookup("ops/sub/x.md", None, &dropped, None, false),
            OntologyLookup::Mode(OntologyMode::Strict)
        );
        // Sibling of the dropped prefix gates normally.
        assert_eq!(
            cfg.ontology_lookup("elsewhere/x.md", None, &dropped, None, false),
            OntologyLookup::Climb
        );

        // (3) unplaceable absolute path + any off entry in the map → Hold.
        assert_eq!(
            cfg.ontology_lookup(
                "/nowhere/a.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Hold
        );
        // …but with NO off entries and nothing dropped, unplaceable climbs
        // (the pre-existing shipped behavior for tiers is full; the gate
        // has nothing to protect).
        assert_eq!(
            IngestConfig::default().ontology_lookup(
                "/nowhere/a.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Climb
        );

        // (2a) dropped ontology_default + no folder match → Hold.
        assert_eq!(
            IngestConfig::default().ontology_lookup(
                "other/x.md",
                None,
                &OntologyDegradedState {
                    default_dropped: true,
                    ..Default::default()
                },
                None,
                false
            ),
            OntologyLookup::Hold
        );

        // (2b) absent default + unparseable schema → Hold (degraded, r15-M2).
        assert_eq!(
            IngestConfig::default().ontology_lookup(
                "other/x.md",
                None,
                &OntologyDegradedState::default(),
                None,
                true
            ),
            OntologyLookup::Hold
        );
        // Same but schema present → climb (intent readable).
        assert_eq!(
            IngestConfig::default().ontology_lookup(
                "other/x.md",
                None,
                &OntologyDegradedState::default(),
                Some(OntologySelection::SchemaOrg),
                true
            ),
            OntologyLookup::Climb
        );
    }

    /// R2.2.2 (r18-m1): two keys normalizing to the same prefix with
    /// conflicting modes → tie → Hold; same-value ties resolve to that value.
    #[test]
    fn ontology_tie_conflict_holds_and_same_value_ties_resolve() {
        let mut cfg = IngestConfig::default();
        cfg.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        cfg.folder_ontology
            .insert("ops/".to_string(), OntologyMode::Strict);
        assert_eq!(
            cfg.ontology_lookup(
                "ops/a.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Hold
        );

        let mut same = IngestConfig::default();
        same.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        same.folder_ontology
            .insert("ops/".to_string(), OntologyMode::Off);
        assert_eq!(
            same.ontology_lookup(
                "ops/a.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Mode(OntologyMode::Off)
        );
    }

    /// The load path (lenient) detects the tie and emits the loud
    /// diagnostic, but does NOT set the global degraded flag (I-3: the
    /// plan's complete global-trigger list, plan-p11-m2, EXCLUDES ties —
    /// a tie holds scoped via resolver step (2) and refuse sites consult
    /// `ontology_ties`, plan-p5-m1). r19-m1: strict-parse configs never
    /// pass through salvage, so the scan runs in both arms.
    #[test]
    fn load_flags_tie_diagnostic_without_global_degrade() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"folder_ontology":{"ops":"off","ops/":"strict"}}}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(
            !report.config.ontology_degraded.global,
            "tie must NOT set global degraded (I-3 scoped-tie rule)"
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.contains("folder_ontology tie")),
            "tie diagnostic present: {:?}",
            report.diagnostics
        );
        // Strict arm: same shape (no global, diagnostic on stderr).
        let cfg = BrainConfig::load(&paths).unwrap();
        assert!(!cfg.ontology_degraded.global);
        // The tie stays queryable for refuse sites (plan-p5-m1) and the
        // combined refuse predicate fires on it.
        assert!(!ontology_ties(&cfg.ingest).is_empty());
        assert!(ontology_degraded_or_tied(
            &cfg.ontology_degraded,
            &cfg.ingest
        ));
    }

    /// I-3 anti-over-hold pin: a tie under "ops" must NOT hold an
    /// unrelated path — "docs/x.md" resolves normally (climb — no folder
    /// entry covers it), while paths under the tied prefixes DO hold
    /// (resolver step (2)).
    #[test]
    fn tie_holds_tied_prefixes_but_not_unrelated_paths() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"folder_ontology":{"ops":"off","ops/":"strict"}}}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(report.config.ontology_degraded.global == false);
        assert!(!ontology_ties(&report.config.ingest).is_empty());
        // Tied prefixes: Hold (step (2)).
        assert_eq!(
            report.config.ingest.ontology_lookup(
                "ops/a.md",
                None,
                &report.config.ontology_degraded,
                None,
                false
            ),
            OntologyLookup::Hold,
            "paths under tied prefixes hold (step 2)"
        );
        // Unrelated path: resolves normally (no map entry → climb; NOT the
        // brain-wide Hold the old global flag produced).
        assert_eq!(
            report.config.ingest.ontology_lookup(
                "docs/x.md",
                None,
                &report.config.ontology_degraded,
                None,
                false
            ),
            OntologyLookup::Climb,
            "anti-over-hold: unrelated path must NOT hold because of a tie elsewhere"
        );
    }

    /// I-3 via the policy surface (`ingest_policy_for_db`): a tied config
    /// does NOT flag `ingest_ontology_degraded` (the mint-time step-(0)
    /// global Hold), so resolver step (2) stays the reachable per-path
    /// tie hold; the policy still carries the tied tiers for refuse sites.
    #[test]
    fn policy_does_not_globally_degrade_on_tie() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"folder_ontology":{"ops":"off","ops/":"strict"}},"vault_path":"/v"}"#,
        );
        let db = paths.db_path.to_str().unwrap().to_string();
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let policy = ingest_policy_for_db(Some(&db));
        assert!(
            !policy.ingest_ontology_degraded,
            "tie must not set the global policy flag (I-3)"
        );
        // The tied map still carries through, so refuse sites can query it.
        assert!(!ontology_ties(&policy.tiers).is_empty());
    }

    /// R2.2.3: `off`/`strict` vocabulary; values are lowercase,
    /// case-sensitive (`Off` is a bad value → dropped → degraded).
    #[test]
    fn ontology_mode_values_are_lowercase_and_case_sensitive() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"ontology_default":"Off","folder_ontology":{"x":"Off"}}}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert_eq!(report.config.ingest.ontology_default, None);
        assert!(report.config.ingest.folder_ontology.is_empty());
        // Both keys dropped by salvage — DEGRADED, but scoped (per
        // OntologyDegradedState: `global` is reserved for load-failed /
        // non-object ingest / tie routes, never per-key salvage drops).
        assert!(report.config.ontology_degraded.is_degraded());
        assert!(!report.config.ontology_degraded.global);
        assert!(report.config.ontology_degraded.default_dropped);
        assert_eq!(report.config.ontology_degraded.dropped_prefixes, vec!["x"]);

        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"ontology_default":"strict","folder_ontology":{"x":"off"}}}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert_eq!(
            report.config.ingest.ontology_default,
            Some(OntologyMode::Strict)
        );
        assert_eq!(
            report.config.ingest.folder_ontology.get("x"),
            Some(&OntologyMode::Off)
        );
        assert!(!report.config.ontology_degraded.global);
    }

    /// R2.2.4 (r15-M1): keys salvage INDEPENDENTLY — a bad folder_ontology
    /// entry does not touch folder_tiers, and a config with no folder_tiers
    /// does not degrade globally.
    #[test]
    fn salvage_keys_are_independent() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"folder_ontology":{"good":"off","bad":"Bogus"}}}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        // Only the bad prefix dropped; global NOT degraded.
        assert!(!report.config.ontology_degraded.global);
        assert_eq!(
            report.config.ontology_degraded.dropped_prefixes,
            vec!["bad"]
        );
        assert_eq!(
            report.config.ingest.folder_ontology.get("good"),
            Some(&OntologyMode::Off)
        );
        // Scoped hold: mints under `bad` hold; `good` resolves.
        let pol = report.config.ontology_degraded.clone();
        assert_eq!(
            report
                .config
                .ingest
                .ontology_lookup("bad/a.md", None, &pol, None, false),
            OntologyLookup::Hold
        );
        assert_eq!(
            report
                .config
                .ingest
                .ontology_lookup("good/a.md", None, &pol, None, false),
            OntologyLookup::Mode(OntologyMode::Off)
        );
    }

    /// R2.2.4 (r3-m5 scope): an ingest value that is not an object at all
    /// → GLOBAL degraded.
    #[test]
    fn non_object_ingest_block_degrades_globally() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(&paths, r#"{"ingest":[1,2,3]}"#);
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(report.config.ontology_degraded.global);
    }

    /// R2.2.4: degraded load + write → the on-disk ingest block survives
    /// byte-for-byte (raw_ingest leave-untouched rule, r15-m1).
    #[test]
    fn degraded_write_leaves_ingest_block_untouched() {
        let (tmp, paths) = degraded_fixture_dir();
        let raw = r#"{"ingest":{"folder_ontology":{"x":"Off"}},"vault_path":"/v"}"#;
        write_cfg(&paths, raw);
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        // Strict load() routes through lenient salvage → degraded.
        let mut cfg = BrainConfig::load(&paths).unwrap();
        assert!(cfg.raw_ingest.is_some());
        cfg.write(&paths).unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        // The ingest sub-object is preserved verbatim inside the written doc
        // (plan-p4-m6: `write()` re-serializes pretty-printed, so compare
        // parsed `serde_json::Value`s on `root["ingest"]`, not file bytes).
        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            root.get("ingest").unwrap(),
            &serde_json::json!({"folder_ontology": {"x": "Off"}}),
            "raw ingest survived verbatim: {after}"
        );
        // Next load is still degraded (nothing erased).
        let again = BrainConfig::load(&paths).unwrap();
        assert!(again.raw_ingest.is_some());
    }

    /// Write protection: a typed folder_tiers mutation made while degraded
    /// must NOT silently erase the broken block (r8-m3/r17-m1 refusal
    /// precondition: write() leaves on-disk ingest untouched).
    #[test]
    fn degraded_write_discards_typed_ingest_mutations() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"folder_ontology":{"x":"Off"}},"vault_path":"/v"}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let mut cfg = BrainConfig::load(&paths).unwrap();
        cfg.ingest
            .folder_ontology
            .insert("y".to_string(), OntologyMode::Strict);
        cfg.write(&paths).unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        assert!(
            !after.contains(r#""y""#),
            "typed mutation while degraded must not land: {after}"
        );
    }

    /// Healthy config: raw_ingest is never set and writes persist normally
    /// (matrix: healthy config + typed edit survives a write).
    #[test]
    fn healthy_write_persists_typed_ingest() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(&paths, BASE_CFG);
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let mut cfg = BrainConfig::load(&paths).unwrap();
        assert!(cfg.raw_ingest.is_none());
        cfg.ingest
            .folder_ontology
            .insert("dir".to_string(), OntologyMode::Off);
        cfg.write(&paths).unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        // plan-p4-m6: compare parsed values — `write()` re-serializes
        // pretty-printed, so compact substring checks never match.
        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            root.pointer("/ingest/folder_ontology/dir").unwrap(),
            &serde_json::json!("off"),
            "{after}"
        );
    }

    /// R2.2.4 (r13-m2): unknown ingest sub-keys survive BOTH load arms.
    #[test]
    fn unknown_ingest_subkeys_survive_both_load_arms() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"future_key":1,"folder_tiers":{"a":"full"}},"vault_path":"/v","migrated_to_v2":false,"generation":{"provider":"unconfigured","model_name":null,"model_path":null,"external_url":null,"api_key":null,"timeout_secs":null},"embedding":{"provider":"fastembed","external_url":null},"privacy":{"mode":"strict","chosen":true,"ephemeral_disclosure_acknowledged":true,"migration_disclosure_acknowledged":true}}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        // Strict arm.
        let cfg = BrainConfig::load(&paths).unwrap();
        assert_eq!(cfg.preserved_ingest.as_ref().unwrap()["future_key"], 1);
        cfg.write(&paths).unwrap();
        assert!(
            std::fs::read_to_string(&paths.config_path)
                .unwrap()
                .contains("future_key"),
            "future key survives strict arm write"
        );
        // Lenient arm (break a typed block to force the fallback).
        write_cfg(
            &paths,
            r#"{"ingest":{"future_key":1,"folder_tiers":{"a":"full"}},"generation":{"provider":"no-such-provider","model_name":null,"model_path":null,"external_url":null,"api_key":null,"timeout_secs":null}}"#,
        );
        let cfg = BrainConfig::load(&paths).unwrap();
        assert_eq!(cfg.preserved_ingest.as_ref().unwrap()["future_key"], 1);
        cfg.write(&paths).unwrap();
        assert!(
            std::fs::read_to_string(&paths.config_path)
                .unwrap()
                .contains("future_key"),
            "future key survives lenient arm write"
        );
    }

    /// R2.2.5 (r16-MAJOR-1): an unparseable ontology block sets raw_ontology;
    /// an UNRELATED writer leaves the on-disk ontology value untouched.
    #[test]
    fn unparseable_ontology_survives_unrelated_write() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ontology":{"schema":"bogus-selection"},"vault_path":"/v"}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(report.ontology_unparseable);
        let mut cfg = report.config;
        assert!(cfg.raw_ontology.is_some());
        // Unrelated mutation + write (e.g. approve_link).
        cfg.vault_path = Some("/v2".to_string());
        cfg.write(&paths).unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        // plan-p4-m6: parsed-value assertion — `write()` re-serializes
        // pretty-printed, so compact substring checks never match.
        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            root.pointer("/ontology/schema").unwrap(),
            &serde_json::json!("bogus-selection"),
            "bad schema value must survive an unrelated write: {after}"
        );
        // Next load still degraded.
        assert!(
            BrainConfig::load_lenient(&paths)
                .unwrap()
                .ontology_unparseable
        );
    }

    /// R2.2.5 (r17-MAJOR-2): `replace_ontology` is the only escape hatch.
    #[test]
    fn replace_ontology_clears_degraded_marker_and_writes() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ontology":{"schema":"bogus-selection"},"vault_path":"/v"}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let mut cfg = BrainConfig::load_lenient(&paths).unwrap().config;
        assert!(cfg.raw_ontology.is_some());
        cfg.replace_ontology(
            crate::ontology_config::OntologyConfigBlock {
                schema: Some(OntologySelection::Emergent),
            },
            &paths,
        )
        .unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        // plan-p4-m6: parsed-value assertion (pretty-printed write output).
        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            root.pointer("/ontology/schema").unwrap(),
            &serde_json::json!("emergent"),
            "deliberate change persisted: {after}"
        );
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(!report.ontology_unparseable);
        assert_eq!(
            report.config.ontology.schema,
            Some(OntologySelection::Emergent)
        );
    }

    /// Opus tier-3 MAJOR-1: the loader can no longer park
    /// `preserved_keys["ontology"]` (parking removed — opus re-review m2),
    /// so the purge in `replace_ontology` is belt-and-braces. This test
    /// hand-injects a parked value (m2, opus confirming review) to exercise
    /// the purge for real: `write()` merges `preserved_keys` LAST — after
    /// the typed ontology block insert — so without the purge the stale
    /// value would overwrite the deliberate change on disk (onboarding
    /// silently dropped the user's schema choice). The purge must land the
    /// new schema AND leave unrelated preserved keys intact.
    #[test]
    fn replace_ontology_purges_parked_preserved_ontology_key() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ontology":{"schema":"emergent"},"trusted_links":[{"link":"docs/specs","target":"/vault/docs/specs","approved_at":0}],"x_custom":1}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let mut cfg = BrainConfig::load_lenient(&paths).unwrap().config;
        // The loader never parks "ontology" (it is in known_keys) — assert
        // that honestly, then hand-inject the parked value the purge exists
        // to defend against, simulating a future loader regression.
        assert!(
            cfg.preserved_keys
                .as_ref()
                .and_then(|v| v.get("ontology"))
                .is_none(),
            "loader must not park ontology into preserved_keys"
        );
        {
            let pk = cfg
                .preserved_keys
                .get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            pk.as_object_mut()
                .unwrap()
                .insert("ontology".to_string(), serde_json::json!(5));
        }
        // The unknown top-level key IS parked (it feeds preserved_keys).
        assert_eq!(
            cfg.preserved_keys.as_ref().and_then(|v| v.get("x_custom")),
            Some(&serde_json::json!(1)),
            "unknown top-level key must be parked in preserved_keys for the test to be honest"
        );
        cfg.replace_ontology(
            crate::ontology_config::OntologyConfigBlock {
                schema: Some(OntologySelection::SchemaOrg),
            },
            &paths,
        )
        .unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        // plan-p4-m6: parsed-value assertion (pretty-printed write output).
        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            root.pointer("/ontology/schema").unwrap(),
            &serde_json::json!("schema-org"),
            "replace_ontology's typed block must reach disk, not lose to the parked value: {after}"
        );
        // The trusted_links ledger survives the round-trip.
        assert_eq!(
            root.pointer("/trusted_links/0/link").unwrap(),
            &serde_json::json!("docs/specs"),
            "trusted_links entry must survive the replace_ontology write: {after}"
        );
        // And the unknown top-level key survives the purge too.
        assert_eq!(
            root.get("x_custom"),
            Some(&serde_json::json!(1)),
            "unknown top-level key must survive replace_ontology's purge: {after}"
        );
        // And the file is now healthy: no degraded reload.
        assert_ne!(root.get("ontology"), Some(&serde_json::json!(5)));
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(!report.ontology_unparseable);
        assert_eq!(
            report.config.ontology.schema,
            Some(OntologySelection::SchemaOrg)
        );
    }

    /// R2.2.4: non-NotFound I/O errors are DEGRADED, missing config is not.
    #[test]
    fn unreadable_config_is_degraded_but_missing_is_not() {
        let (tmp, paths) = degraded_fixture_dir();
        let cfg_path = paths.config_path.clone();
        std::fs::create_dir_all(&cfg_path).unwrap(); // EISDIR on read
        let db = paths.db_path.to_str().unwrap().to_string();
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let policy = ingest_policy_for_db(Some(&db));
        assert!(
            policy.ingest_ontology_degraded,
            "unreadable config must degrade, not silently default"
        );
        // Missing config (NotFound): healthy default, no degraded flag.
        let policy = ingest_policy_for_db(Some("/nonexistent-brain/brain.db"));
        assert!(!policy.ingest_ontology_degraded);
        assert!(policy.ontology_selection.is_none());
    }

    /// R2.2.4: cache is invalidated by a config whose ONLY change is the
    /// ontology block (byte-keyed cache carries the carrier fields).
    #[test]
    fn policy_cache_invalidates_on_ontology_only_change() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(&paths, BASE_CFG);
        let db = paths.db_path.to_str().unwrap().to_string();
        let cfg_path = paths.config_path.clone();
        let _ = tmp;

        let p1 = ingest_policy_for_db(Some(&db));
        assert!(!p1.ingest_ontology_degraded);
        assert!(p1.ontology_selection.is_none());

        // Ontology-only change (same length not required — bytes differ).
        std::fs::write(
            &cfg_path,
            &format!(
                r#"{{"ontology":{{"schema":"off"}},"vault_path":"/v","migrated_to_v2":false,"generation":{{"provider":"unconfigured","model_name":null,"model_path":null,"external_url":null,"api_key":null,"timeout_secs":null}},"embedding":{{"provider":"fastembed","external_url":null}},"privacy":{{"mode":"strict","chosen":true,"ephemeral_disclosure_acknowledged":true,"migration_disclosure_acknowledged":true}}}}"#
            ),
        )
        .unwrap();
        let p2 = ingest_policy_for_db(Some(&db));
        assert_eq!(p2.ontology_selection, Some(OntologySelection::Off));
        // The ONLY delta between the two configs is the ontology block, so
        // the stale-cache failure mode is a policy without the carrier:
        // `ontology_selection` still None means the cache served p1.
        assert_ne!(
            p1.ontology_selection, p2.ontology_selection,
            "cache served stale policy (ontology_selection not refreshed)"
        );
    }

    /// R2.2.8 (r18-MAJOR-1): the watermark hash is canonical — same map in
    /// two insertion orders hashes identically.
    #[test]
    fn watermark_hash_is_insertion_order_independent() {
        // Same key→value mapping, different insertion SEQUENCE (HashMap
        // iteration order follows insertion with serde_json's
        // `preserve_order`; the BTreeMap canonical form must erase it).
        let build = |ops_first: bool| {
            let mut cfg = IngestConfig::default();
            if ops_first {
                cfg.folder_ontology
                    .insert("ops".to_string(), OntologyMode::Off);
                cfg.folder_ontology
                    .insert("people".to_string(), OntologyMode::Strict);
            } else {
                cfg.folder_ontology
                    .insert("people".to_string(), OntologyMode::Strict);
                cfg.folder_ontology
                    .insert("ops".to_string(), OntologyMode::Off);
            }
            cfg
        };
        let a = build(true);
        let b = build(false);
        assert_eq!(
            ontology_config_watermark_hash(&a, None, false),
            ontology_config_watermark_hash(&b, None, false)
        );
        // Key spellings that normalize together hash identically too.
        let mut c = IngestConfig::default();
        c.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        c.folder_ontology
            .insert("people/".to_string(), OntologyMode::Strict);
        assert_eq!(
            ontology_config_watermark_hash(&a, None, false),
            ontology_config_watermark_hash(&c, None, false)
        );
    }

    /// Opus tier-3 MINOR-5 + re-review m1: the keys `""` and `"raw:"` must
    /// both FEED `ontology_config_watermark_hash` through the real function
    /// (no self-checking rebuild of its map) and hash deterministically
    /// across insertion orders (`""` is unmatchable → synthetic `/raw:`
    /// slot; a literal `raw:` key is MATCHABLE → normalized `raw:` slot;
    /// the `/raw:` marker keeps the two apart, unlike the pre-fix `raw:`
    /// marker where insertion order decided whose value won the slot).
    #[test]
    fn watermark_hash_raw_marker_no_collision() {
        // Flipping ONE unmatchable key's value changes the hash: the key
        // participates in the hashed payload via the real function.
        let build = |empty: OntologyMode, raw: OntologyMode| {
            let mut cfg = IngestConfig::default();
            cfg.folder_ontology.insert("".to_string(), empty);
            cfg.folder_ontology.insert("raw:".to_string(), raw);
            cfg
        };
        let base = build(OntologyMode::Off, OntologyMode::Strict);
        let base_hash = ontology_config_watermark_hash(&base, None, false);

        // Flip only the unmatchable "" key.
        let mut empty_flip = base.clone();
        empty_flip
            .folder_ontology
            .insert("".to_string(), OntologyMode::Strict);
        assert_ne!(
            base_hash,
            ontology_config_watermark_hash(&empty_flip, None, false),
            "the unmatchable empty key must feed the watermark hash"
        );

        // Flip only the raw:-named key.
        let mut raw_flip = base.clone();
        raw_flip
            .folder_ontology
            .insert("raw:".to_string(), OntologyMode::Off);
        assert_ne!(
            base_hash,
            ontology_config_watermark_hash(&raw_flip, None, false),
            "the literal raw: key must feed the watermark hash"
        );

        // Determinism: same map built in both insertion orders, same hash
        // (guards the old HashMap-iteration-order flakiness).
        let mut c = IngestConfig::default();
        c.folder_ontology.insert("".to_string(), OntologyMode::Off);
        c.folder_ontology
            .insert("raw:".to_string(), OntologyMode::Strict);
        let mut d = IngestConfig::default();
        d.folder_ontology
            .insert("raw:".to_string(), OntologyMode::Strict);
        d.folder_ontology.insert("".to_string(), OntologyMode::Off);
        assert_eq!(
            ontology_config_watermark_hash(&c, None, false),
            ontology_config_watermark_hash(&d, None, false),
            "raw-key marker collision: insertion order decided the hash"
        );
        // And the marker form can never equal a matchable key's normalized
        // form (the collision-safety argument itself).
        assert_ne!(normalize_key(""), "/raw:");
        assert_ne!(normalize_key("raw:"), "/raw:");
    }

    /// R2.2.8 (r20-m2): entering/leaving tie-degraded changes the hash;
    /// an ontology_default edit changes it; an absent default wires in the
    /// rung-3 inputs (schema switch fires a report).
    #[test]
    fn watermark_hash_tracks_ontology_relevant_inputs() {
        let mut healthy = IngestConfig::default();
        healthy
            .folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);

        // Tie-degraded: hash changes AND is stable across insertion orders
        // (conflicting keys excluded).
        let mut tied = IngestConfig::default();
        tied.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        tied.folder_ontology
            .insert("ops/".to_string(), OntologyMode::Strict);
        let mut tied_alt = IngestConfig::default();
        tied_alt
            .folder_ontology
            .insert("ops/".to_string(), OntologyMode::Strict);
        tied_alt
            .folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        let h_tie = ontology_config_watermark_hash(&tied, None, false);
        assert_ne!(h_tie, ontology_config_watermark_hash(&healthy, None, false));
        assert_eq!(
            h_tie,
            ontology_config_watermark_hash(&tied_alt, None, false)
        );

        // ontology_default set vs absent differs.
        let mut with_default = healthy.clone();
        with_default.ontology_default = Some(OntologyMode::Strict);
        assert_ne!(
            ontology_config_watermark_hash(&healthy, None, false),
            ontology_config_watermark_hash(&with_default, None, false)
        );

        // Absent default: schema participates (Off vs SchemaOrg differ);
        // set default: schema does not.
        assert_ne!(
            ontology_config_watermark_hash(&healthy, Some(OntologySelection::Off), false),
            ontology_config_watermark_hash(&healthy, Some(OntologySelection::SchemaOrg), false)
        );
        assert_eq!(
            ontology_config_watermark_hash(&with_default, Some(OntologySelection::Off), false),
            ontology_config_watermark_hash(
                &with_default,
                Some(OntologySelection::SchemaOrg),
                false
            )
        );
        // Unparseable flag participates when the default is absent.
        assert_ne!(
            ontology_config_watermark_hash(&healthy, None, false),
            ontology_config_watermark_hash(&healthy, None, true)
        );
    }

    // NOTE (m3, opus confirming review): the former hand-rolled rollback
    // tests (`inference_rollback_style_write_preserves_ingest_and_ontology`,
    // `rollback_write_preserves_on_disk_generation`) re-created the rollback
    // by hand (load_lenient → write) instead of driving the real handler;
    // with the rollback writes removed (M1/m4) there is no rollback left to
    // exercise here. Real-handler coverage lives in
    // `inference::tests::{update_provider_init_failure_leaves_disk_untouched,
    // update_provider_write_failure_leaves_disk_untouched}`.

    // ── Tier-2 review fixes (task-1-review-fixes) ─────────────────────────

    /// I-1 regression: a config with a NON-OBJECT `ingest` AND a
    /// trusted_links array → load_lenient → write() → the on-disk
    /// trusted_links value survives (parsed Value equality). The pre-fix
    /// early-return skipped trusted_links parsing below it, so the loaded
    /// config carried the empty default and write() erased the ledger.
    #[test]
    fn non_object_ingest_does_not_erase_trusted_links() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":"bogus","trusted_links":[{"link":"docs/specs","target":"/vault/docs/specs","approved_at":1}],"vault_path":"/v"}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let mut cfg = BrainConfig::load_lenient(&paths).unwrap().config;
        assert!(cfg.ontology_degraded.global, "non-object ingest degrades");
        assert_eq!(
            cfg.trusted_links.len(),
            1,
            "ledger must be salvaged even when ingest is non-object"
        );
        cfg.write(&paths).unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            root.get("trusted_links").unwrap(),
            &serde_json::json!([{"link":"docs/specs","target":"/vault/docs/specs","approved_at":1}]),
            "on-disk trusted_links survived the degraded write cycle: {after}"
        );
    }

    /// I-2 pin (plan-p14-m5): unmatchable keys COUNT in the watermark
    /// hash — retagging an inert key changes it, and adding/removing an
    /// inert key changes it.
    #[test]
    fn watermark_hash_counts_unmatchable_keys() {
        let build = |inert: Option<&str>| {
            let mut cfg = IngestConfig::default();
            cfg.folder_ontology
                .insert("ops".to_string(), OntologyMode::Off);
            if let Some(k) = inert {
                cfg.folder_ontology.insert(k.to_string(), OntologyMode::Off);
            }
            cfg
        };
        // Retagging an inert key ("./ops" → "/junk") CHANGES the hash.
        assert_ne!(
            ontology_config_watermark_hash(&build(Some("./ops")), None, false),
            ontology_config_watermark_hash(&build(Some("/junk")), None, false)
        );
        // Adding an inert key changes it (compared against the base).
        let base = ontology_config_watermark_hash(&build(None), None, false);
        assert_ne!(
            base,
            ontology_config_watermark_hash(&build(Some("./ops")), None, false)
        );
        // A map with ONLY an inert key still hashes (not dropped from the
        // payload), and inert keys are collision-safe against matchable
        // ones: "./ops" hashes under the synthetic raw: form, "ops" under
        // its normalized form — never the same map.
        let mut inert_only = IngestConfig::default();
        inert_only
            .folder_ontology
            .insert("./ops".to_string(), OntologyMode::Off);
        let mut matchable_only = IngestConfig::default();
        matchable_only
            .folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        assert_ne!(
            ontology_config_watermark_hash(&inert_only, None, false),
            ontology_config_watermark_hash(&matchable_only, None, false)
        );
    }

    /// I-4: a tie in one folder must NOT mask a legitimate
    /// `ontology_default`-style edit elsewhere — the tie-degraded hash
    /// encoding keeps non-conflicting keys and only "degraded" flips.
    #[test]
    fn watermark_hash_tie_does_not_mask_unrelated_edit() {
        // Tied map + unrelated live key.
        let build = |other: Option<(&str, OntologyMode)>| {
            let mut cfg = IngestConfig::default();
            cfg.folder_ontology
                .insert("ops".to_string(), OntologyMode::Off);
            cfg.folder_ontology
                .insert("ops/".to_string(), OntologyMode::Strict);
            if let Some((k, m)) = other {
                cfg.folder_ontology.insert(k.to_string(), m);
            }
            cfg
        };
        let tied = ontology_config_watermark_hash(&build(None), None, false);
        let tied_plus = ontology_config_watermark_hash(
            &build(Some(("people", OntologyMode::Off))),
            None,
            false,
        );
        assert_ne!(
            tied, tied_plus,
            "tie in one folder must not mask an unrelated map edit (I-4)"
        );
        // Flipping the unrelated key's VALUE also changes the hash.
        let tied_alt = ontology_config_watermark_hash(
            &build(Some(("people", OntologyMode::Strict))),
            None,
            false,
        );
        assert_ne!(tied_plus, tied_alt);
        // Tie-degraded is stable across insertion orders (excluded keys'
        // surviving value cannot leak HashMap order into the hash).
        let mut a = IngestConfig::default();
        a.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        a.folder_ontology
            .insert("ops/".to_string(), OntologyMode::Strict);
        a.folder_ontology
            .insert("people".to_string(), OntologyMode::Off);
        let mut b = IngestConfig::default();
        b.folder_ontology
            .insert("people".to_string(), OntologyMode::Off);
        b.folder_ontology
            .insert("ops/".to_string(), OntologyMode::Strict);
        b.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        assert_eq!(
            ontology_config_watermark_hash(&a, None, false),
            ontology_config_watermark_hash(&b, None, false)
        );
    }

    /// M-3(a): tier tie conservative-wins via min_by_key, including the
    /// diagnostic string naming the ranking.
    #[test]
    fn tier_tie_conservative_wins_with_diagnostic() {
        let mut cfg = IngestConfig::default();
        cfg.folder_tiers.insert("ops".to_string(), IngestTier::Full);
        cfg.folder_tiers
            .insert("ops/".to_string(), IngestTier::None);
        // Both keys normalize to `ops` at the same depth; conservative wins.
        assert_eq!(cfg.tier_for_path("ops/a.md", None), IngestTier::None);
        assert_eq!(
            cfg.tier_for("ops/a.md"),
            IngestTier::None,
            "min_by_key(tie_rank): none < chunks-only < full"
        );
        // chunks-only vs full → chunks-only.
        let mut cfg2 = IngestConfig::default();
        cfg2.folder_tiers
            .insert("ops".to_string(), IngestTier::Full);
        cfg2.folder_tiers
            .insert("ops/".to_string(), IngestTier::ChunksOnly);
        assert_eq!(cfg2.tier_for("ops/a.md"), IngestTier::ChunksOnly);
        // The load path emits the ranking-naming diagnostic.
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(
            &paths,
            r#"{"ingest":{"folder_tiers":{"ops":"full","ops/":"none"}}}"#,
        );
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(
            report.diagnostics.iter().any(|d| {
                d.contains("folder_tiers tie")
                    && d.contains("most conservative wins")
                    && d.contains("none < chunks-only < full")
            }),
            "tier tie diagnostic present: {:?}",
            report.diagnostics
        );
    }

    /// M-3(b): folder_tiers `full` + folder_ontology `off` on the SAME
    /// prefix → ingest normally (tier full) AND the mint skips the gate
    /// (lookup → Mode(Off)). The two maps gate different layers.
    #[test]
    fn full_tier_and_off_ontology_same_prefix_ingest_and_skip_gate() {
        let mut cfg = IngestConfig::default();
        cfg.folder_tiers.insert("ops".to_string(), IngestTier::Full);
        cfg.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        assert_eq!(
            cfg.tier_for_path("ops/a.md", None),
            IngestTier::Full,
            "tier full: documents ingest normally"
        );
        assert_eq!(
            cfg.ontology_lookup(
                "ops/a.md",
                None,
                &OntologyDegradedState::default(),
                None,
                false
            ),
            OntologyLookup::Mode(OntologyMode::Off),
            "gate off: mints in the subtree skip the gate"
        );
    }

    /// M-3(d): truncated/malformed-JSON config.json → degraded policy
    /// through `parse_ingest_policy_from_bytes` (same-bytes parse rule).
    #[test]
    fn malformed_json_config_degrades_policy() {
        let policy = parse_ingest_policy_from_bytes(b"{\"ingest\":{\"folder_on");
        assert!(
            policy.ingest_ontology_degraded,
            "malformed JSON must degrade, never silently default"
        );
        // Non-UTF-8 bytes degrade too.
        let policy = parse_ingest_policy_from_bytes(&[0xff, 0xfe, 0x00]);
        assert!(policy.ingest_ontology_degraded);
    }

    /// M-3(e): a non-object `folder_ontology` VALUE (the string "off" as
    /// the whole folder_ontology value) → global degrade (raw_ingest set,
    /// write leaves the block untouched).
    #[test]
    fn non_object_folder_ontology_value_degrades_globally() {
        let (tmp, paths) = degraded_fixture_dir();
        write_cfg(&paths, r#"{"ingest":{"folder_ontology":"off"}}"#);
        // tmp stays alive until end of test: dropping it deletes the
        // fixture directory the loads below must read.
        let report = BrainConfig::load_lenient(&paths).unwrap();
        assert!(
            report.config.ontology_degraded.global,
            "non-object folder_ontology degrades globally (no per-key scope to point at)"
        );
        assert!(report.config.raw_ingest.is_some());
        // …and via the policy surface. Built via Value manipulation: the old
        // string-splice fixture (`BASE_CFG.trim_end_matches('}') + …`) ate
        // privacy's closing brace too and produced MALFORMED JSON, so the
        // test passed via the parse-error path, not the non-object
        // folder_ontology path. Parse, set `ingest.folder_ontology` to the
        // non-object string "off", re-serialize — an otherwise valid config
        // whose only fault is the non-object VALUE.
        let mut root: serde_json::Value = serde_json::from_str(BASE_CFG).unwrap();
        root["ingest"]["folder_ontology"] = serde_json::json!("off");
        write_cfg(&paths, &root.to_string());
        let db = paths.db_path.to_str().unwrap().to_string();
        let policy = ingest_policy_for_db(Some(&db));
        assert!(policy.ingest_ontology_degraded);
    }

    /// M-3(f): dropped-child-under-valid-parent direction — dropped
    /// "ops/sub" + valid "ops" → paths under ops/sub HOLD, ops itself
    /// follows "ops" (the valid parent's mode still decides).
    #[test]
    fn dropped_child_under_valid_parent_holds_only_under_child() {
        let mut cfg = IngestConfig::default();
        cfg.folder_ontology
            .insert("ops".to_string(), OntologyMode::Off);
        let dropped = OntologyDegradedState {
            dropped_prefixes: vec!["ops/sub".to_string()],
            ..Default::default()
        };
        // Under the dropped child → Hold, despite the valid parent.
        assert_eq!(
            cfg.ontology_lookup("ops/sub/a.md", None, &dropped, None, false),
            OntologyLookup::Hold
        );
        // Deeper than the dropped child with no valid deeper entry → Hold.
        assert_eq!(
            cfg.ontology_lookup("ops/sub/deep/b.md", None, &dropped, None, false),
            OntologyLookup::Hold
        );
        // ops itself (NOT under the dropped child) follows "ops".
        assert_eq!(
            cfg.ontology_lookup("ops/a.md", None, &dropped, None, false),
            OntologyLookup::Mode(OntologyMode::Off)
        );
    }

    /// M-2: a dropped UNMATCHABLE key with a usable path form ("./ops")
    /// holds under its usable prefix — the diagnostic's "mints under it
    /// hold" promise is honest. A key with NO usable form ("/", "..")
    /// holds nothing and its diagnostic says "stays inert".
    #[test]
    fn dropped_unmatchable_key_holds_under_usable_form_or_nothing() {
        // "./ops" dropped → paths under ops (usable form) hold.
        let dot = OntologyDegradedState {
            dropped_prefixes: vec!["./ops".to_string()],
            ..Default::default()
        };
        assert_eq!(
            IngestConfig::default().ontology_lookup("ops/a.md", None, &dot, None, false),
            OntologyLookup::Hold,
            "usable form of './ops' holds"
        );
        assert_eq!(
            IngestConfig::default().ontology_lookup("ops/sub/deep.md", None, &dot, None, false),
            OntologyLookup::Hold
        );
        // Unrelated path still resolves (climb).
        assert_eq!(
            IngestConfig::default().ontology_lookup("other/x.md", None, &dot, None, false),
            OntologyLookup::Climb
        );
        // "/" or ".." dropped → no usable prefix → holds nothing.
        let slash = OntologyDegradedState {
            dropped_prefixes: vec!["/".to_string()],
            ..Default::default()
        };
        assert_eq!(
            IngestConfig::default().ontology_lookup("ops/a.md", None, &slash, None, false),
            OntologyLookup::Climb,
            "a key with no usable path form holds nothing"
        );
    }
}
