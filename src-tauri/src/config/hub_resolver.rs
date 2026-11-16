//! Shared hub resolver for the wisdom areas feature (spec 2026-11-14-areas
//! §9.3 core + §6.8.2). ONE home for the deposit-time rule
//! ([`resolve_deposit_hub`]), the CLI scoped-match rule
//! ([`resolve_scoped`]), and the Librarian ID-based rule
//! ([`resolve_hub_id`]) — so deposit-time and Librarian-time answers can
//! never diverge.
//!
//! Diagnostics are DATA, not stderr (tier-3 r3 MAJOR-3): the lib fills
//! `ScopedMatch::diagnostics` and `DepositHub::Unresolved` reason strings;
//! the CLI prints them (task 5). This module performs NO logging.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};
use rusqlite::{OptionalExtension, Connection};

use super::WisdomConfig;
use crate::db::merge_duplicates::{resolve_redirect_chain, ChainResolution};

/// The outcome of resolving a deposit target (or an entity id) to its
/// wisdom repo hub (spec §6.8.2). `Unresolved` CARRIES the reason string
/// (tier-3 r5 m2): the reason surfaces in the reply (`hub_reason:
/// unresolved`, §6.8.5) and the §11.2 no-owner-signal count — never on
/// stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepositHub {
    /// No wisdom block (UNSCOPED delivery downstream).
    Unconfigured,
    /// No configured checkout root prefixes the path.
    NoPrefix,
    /// The path matched a root but the hub could not bind: redirect cycle,
    /// dead survivor, or a same-survivor collision between two configured
    /// keys (§3.3). Carries the reason for diagnostics-as-data.
    Unresolved(String),
    /// The entity id of the surviving repo hub.
    Hub(String),
}

/// A successful scoped match for the CLI `--path=` side (spec §9.2/§9.3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopedMatch {
    /// The merge-group (survivor first, then losers) of the matched repo
    /// hub — the repo scope band of the query.
    pub repo_group: Vec<String>,
    /// The merge-group of the configured org hub; `None` when unconfigured
    /// OR unresolvable (arm omitted, diagnostic recorded).
    pub org_group: Option<Vec<String>>,
    /// The area name whose glob pattern matched the path RELATIVE to the
    /// checkout root (§3.2).
    pub area: Option<String>,
    /// Data lines naming omissions/failures (org arm omitted, …) for the
    /// CLI to print (tier-3 r3 MAJOR-3). Empty when healthy.
    pub diagnostics: Vec<String>,
}

/// Parse a `.ct-areas.yaml` body: `areas: { name: [glob patterns], ... }`
/// (spec §3.1–§3.5). Malformed input is an ERROR — no partial state.
pub fn parse_areas_file(text: &str) -> Result<AreasFile> {
    let v: serde_yaml::Value =
        serde_yaml::from_str(text).context("areas file is not valid YAML")?;
    let root = v.as_mapping().context("areas file must be a YAML mapping")?;
    let key = serde_yaml::Value::String("areas".into());
    let areas = root
        .get(&key)
        .and_then(|x| x.as_mapping())
        .context("areas file must contain an `areas` mapping (name -> patterns)")?;
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (k, v) in areas {
        let name = k
            .as_str()
            .context("area name must be a string")?
            .to_string();
        let pats = v.as_sequence().context("area patterns must be a list")?;
        let mut ps = Vec::new();
        for p in pats {
            ps.push(p.as_str().context("area pattern must be a string")?.to_string());
        }
        out.push((name, ps));
    }
    Ok(AreasFile { areas: out })
}

/// An on-disk `.ct-areas.yaml` (spec §3.1): an ORDERED name -> glob
/// patterns mapping (§3.2: the first area whose pattern matches wins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreasFile {
    pub areas: Vec<(String, Vec<String>)>,
}

struct CacheEntry {
    mtime: SystemTime,
    file: AreasFile,
}

/// Process-wide mtime-gated areas-file cache (tier-3 r1 MAJOR-2 / r2 m4):
/// ONE implementation, instantiated once per process, used by BOTH readers
/// (the one-shot `ct` and the sidecar/app). A file is re-read ONLY when its
/// mtime changed. A failed parse is never cached — no partial state.
struct AreasCache {
    entries: Mutex<HashMap<PathBuf, CacheEntry>>,
}

static AREAS_CACHE: OnceLock<AreasCache> = OnceLock::new();

fn areas_cache() -> &'static AreasCache {
    AREAS_CACHE.get_or_init(|| AreasCache {
        entries: Mutex::new(HashMap::new()),
    })
}

impl AreasCache {
    fn load(&self, path: &Path) -> Result<AreasFile> {
        let mtime = std::fs::metadata(path)
            .context("areas file not found")?
            .modified()
            .context("areas file mtime unavailable")?;
        let mut entries = self.entries.lock().expect("areas cache lock");
        if let Some(hit) = entries.get(path) {
            if hit.mtime == mtime {
                return Ok(hit.file.clone());
            }
        }
        let text = std::fs::read_to_string(path).context("areas file unreadable")?;
        let file = parse_areas_file(&text)?;
        entries.insert(path.to_path_buf(), CacheEntry { mtime, file: file.clone() });
        Ok(file)
    }
}

/// Load an areas file through the process-wide mtime-gated cache (§3.5):
/// re-read ONLY when the file's mtime changed. Malformed file → error, no
/// partial state (a failed parse is never cached).
pub fn load_areas_file(path: &Path) -> Result<AreasFile> {
    areas_cache().load(path)
}

/// The checkout root of a configured key is the parent directory of its
/// areas-file path (§3.1).
fn checkout_root(areas_file: &Path) -> PathBuf {
    areas_file.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Component-wise prefix test on canonical paths: `root` prefixes `path`
/// only at a path boundary ("`/a/b`" does not prefix "`/a/bc`").
fn is_canonical_prefix(root: &str, path: &str) -> bool {
    if root == path {
        return true;
    }
    let pp = Path::new(path);
    let mut p = pp.components();
    // Walk the ROOT's components; every one must match, and any remaining
    // path components are the tail below the boundary.
    for rc in Path::new(root).components() {
        match p.next() {
            Some(pc) if pc == rc => {}
            _ => return false,
        }
    }
    true
}

/// Resolve `id` through the redirect chain to its live survivor.
/// `None` = unresolvable (cycle, or survivor not live).
fn live_survivor(conn: &Connection, id: &str) -> Option<String> {
    match resolve_redirect_chain(conn, id) {
        Ok(ChainResolution::Cycle(_)) => None,
        Ok(ChainResolution::None) if is_live(conn, id) => Some(id.to_string()),
        Ok(ChainResolution::Survivor(s)) if is_live(conn, &s) => Some(s),
        _ => None,
    }
}

fn is_live(conn: &Connection, id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM live_entities WHERE id = ?1",
        [id],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

/// A configured key's "home" survivor for the §3.3 collision check:
/// chain-resolved (cycle keys are skipped — they cannot silently win).
fn configured_survivors(conn: &Connection, cfg: &WisdomConfig) -> Vec<(String, String)> {
    cfg.areas_paths
        .iter()
        .filter_map(|(id, _)| live_survivor(conn, id).map(|s| (id.clone(), s)))
        .collect()
}

/// Same-survivor collision between two configured keys (§3.3): the
/// §9.3 longest-prefix rule must never silently pick one of two areas
/// files — the whole block is treated as unresolved, FAIL CLOSED.
fn detect_collision(pairs: &[(String, String)]) -> Option<String> {
    for (i, (a_id, a_sur)) in pairs.iter().enumerate() {
        for (b_id, b_sur) in pairs.iter().skip(i + 1) {
            if a_sur == b_sur {
                return Some(format!(
                    "unresolved: same-survivor collision between configured keys {a_id} and {b_id} (both resolve to {a_sur})"
                ));
            }
        }
    }
    None
}

/// The §9.3 longest-prefix match over canonicalized checkout roots: the
/// longest canonicalized root (component-boundary) that prefixes the
/// canonicalized path wins; ties are impossible by the load-time
/// duplicate-directory guard. Returns the matched CONFIGURED key id.
fn longest_prefix_match(cfg: &WisdomConfig, canonical_path: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (id, areas_path) in &cfg.areas_paths {
        let root = checkout_root(areas_path);
        // A not-yet-existing root "can prefix nothing" (§9.3): skip it
        // without failing.
        if !root.exists() {
            continue;
        }
        let canonical_root = super::canonicalize_lenient(&root.to_string_lossy());
        if is_canonical_prefix(&canonical_root, canonical_path) {
            let len = canonical_root.len();
            if best.as_ref().is_none_or(|(bl, _)| len > *bl) {
                best = Some((len, id.clone()));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Deposit-time hub resolution (§6.8.2). `repo_path` is the ABSOLUTE path
/// being deposited (a relative path is the CALLER's refusal; the PARAM
/// itself missing is the DOOR's translation before this is invoked, task 8).
/// No config → `Unconfigured`; no prefix → `NoPrefix`; a matched key
/// resolves through its redirect chain and a live check, unless a
/// same-survivor collision between two configured keys poisons the block.
pub fn resolve_deposit_hub(
    conn: &Connection,
    cfg: Option<&WisdomConfig>,
    repo_path: &str,
) -> DepositHub {
    let Some(cfg) = cfg else {
        return DepositHub::Unconfigured;
    };
    if cfg.areas_paths.is_empty() {
        return DepositHub::Unconfigured;
    }
    // §3.3: a same-survivor collision fails the WHOLE block closed.
    let survivors = configured_survivors(conn, cfg);
    if let Some(reason) = detect_collision(&survivors) {
        return DepositHub::Unresolved(reason);
    }
    let canonical_path = super::canonicalize_lenient(repo_path);
    let Some(id) = longest_prefix_match(cfg, &canonical_path) else {
        return DepositHub::NoPrefix;
    };
    match live_survivor(conn, &id) {
        Some(s) => DepositHub::Hub(s),
        None => DepositHub::Unresolved("unresolved".to_string()),
    }
}

/// CLI `--path=` scoped match (spec §9.2/§9.3): resolve the matched hub
/// to its survivor FIRST, then expand the merge group (`cluster_ids`).
/// `resolved_path` is the canonical path. `None` when no configured root
/// prefixes (the query stays BYTE-IDENTICAL, §9.8 UNSCOPED fence) or when
/// the matched hub / the block cannot bind (collision, cycle, dead
/// survivor).
pub fn resolve_scoped(
    conn: &Connection,
    cfg: &WisdomConfig,
    resolved_path: &str,
) -> Option<ScopedMatch> {
    if cfg.areas_paths.is_empty() {
        return None;
    }
    let survivors = configured_survivors(conn, cfg);
    if detect_collision(&survivors).is_some() {
        return None;
    }
    let hub_id = longest_prefix_match(cfg, resolved_path)?;
    let hub_survivor = live_survivor(conn, &hub_id)?;
    let repo_group = crate::db::entities::cluster_ids(conn, &hub_survivor).ok()?;

    // Area: globset match on the path RELATIVE to the checkout root (§3.2).
    let areas_path = cfg
        .areas_paths
        .iter()
        .find(|(id, _)| *id == hub_id)
        .map(|(_, p)| p.clone());
    let area = areas_path.and_then(|p| match_area(&p, resolved_path));

    // Org band: resolve the configured org hub to its survivor, live-check,
    // then expand; unresolvable → arm omitted + diagnostic line.
    let (org_group, diagnostics) = match cfg.org_entity_id.as_deref() {
        None => (None, Vec::new()),
        Some(org_id) => match live_survivor(conn, org_id) {
            Some(sur) => match crate::db::entities::cluster_ids(conn, &sur) {
                Ok(group) => (Some(group), Vec::new()),
                Err(_) => (
                    None,
                    vec![format!("org hub {org_id} unresolvable; org band omitted")],
                ),
            },
            None => (
                None,
                vec![format!("org hub {org_id} unresolvable; org band omitted")],
            ),
        },
    };

    Some(ScopedMatch {
        repo_group,
        org_group,
        area,
        diagnostics,
    })
}

/// First area (in file order, §3.2) whose GlobSet matches the path
/// relative to the checkout root; `None` when no pattern matches.
fn match_area(areas_path: &Path, resolved_path: &str) -> Option<String> {
    let file = load_areas_file(areas_path).ok()?;
    let root = super::canonicalize_lenient(&checkout_root(areas_path).to_string_lossy());
    let rel = Path::new(resolved_path)
        .strip_prefix(&root)
        .ok()?
        .to_path_buf();
    for (name, patterns) in &file.areas {
        let mut builder = GlobSetBuilder::new();
        let mut ok = true;
        for p in patterns {
            match Glob::new(p) {
                Ok(g) => {
                    builder.add(g);
                }
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let set = builder.build().ok()?;
        if set.is_match(&rel) {
            return Some(name.clone());
        }
    }
    None
}

/// Librarian read-side ID-based rule (spec §6.8.4): resolve `id` through
/// the redirect chain, live-check the survivor, and accept ONLY when the
/// survivor equals a CONFIGURED `wisdom.areas_paths` key's survivor — one
/// home for the rule, so deposit-time and Librarian-time answers agree.
pub fn resolve_hub_id(conn: &Connection, cfg: &WisdomConfig, id: &str) -> DepositHub {
    if cfg.areas_paths.is_empty() {
        return DepositHub::Unconfigured;
    }
    let survivors = configured_survivors(conn, cfg);
    if let Some(reason) = detect_collision(&survivors) {
        return DepositHub::Unresolved(reason);
    }
    let Some(sur) = live_survivor(conn, id) else {
        return DepositHub::Unresolved("unresolved".to_string());
    };
    if survivors.iter().any(|(_, s)| *s == sur) {
        DepositHub::Hub(sur)
    } else {
        DepositHub::Unresolved("unresolved".to_string())
    }
}
