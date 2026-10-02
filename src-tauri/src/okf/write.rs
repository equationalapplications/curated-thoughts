//! The ONE vault write-path core (spec v2: `docs/superpowers/specs/2026-08-26-mcp-write-path-okf-frontmatter.md`).
//!
//! Every MCP / Tauri write flows through [`write_note`] or [`upsert_index_entry`];
//! surfaces (`lib.rs` commands, `tool_dispatch.rs` dispatchers) are thin adapters.
//!
//! Contracts implemented here (do not re-implement elsewhere):
//! - Path safety: exclusively `crate::vault::safe_vault_path` — no canonicalize/
//!   `starts_with` hand-rolls in callers (grep gate enforced in CI workflow docs).
//! - Staleness: If-Match style token compare on the EXISTING file's
//!   `updated_at` frontmatter value. File mtimes are NEVER consulted.
//! - Atomic durability: temp-file + rename via `crate::vault::safe_write_bytes`.
//! - Index entry matching: whole-line `## {name}` scan. No regex, no `(?m)`,
//!   no substring `find` — a line equals the header iff `line == "## {name}"`.
//! - Pinned block format (spec v2 §C.4):
//!   `## {name}` / `[[{path}]]` / `- Type: {type}` / `- Key: value`… lines.
//! - Errors use the pinned string shapes: `path_outside_vault`,
//!   `invalid_frontmatter:{detail}`, `stale_update:{current}`,
//!   `index_not_found:{path}`, `invalid_entry_name`, `write_error:{io}`,
//!   `shrink_refused:{existing}:{new}: re-read the note and resend the full
//!   body`, `compaction_marker:{marker}: rephrase and resend without
//!   compaction artifacts`, `key_drop_refused:{keys}: …` and
//!   `key_drop_refused:unrepresentable:{keys}: …` (issue #245 — an edit
//!   dropping existing frontmatter keys). Display strings ARE the contract; see each
//!   variant's `#[error]` for the authoritative shape.
//!   When the EXISTING file's frontmatter cannot be read for the If-Match
//!   check, the write is refused with `invalid_frontmatter:existing_unparsable:{parse|no_fence|no_token}`
//!   (`parse` = duplicate/malformed token, `no_fence` = no frontmatter fence,
//!   `no_token` = fence with no `updated_at`). No-fence/no-token notes stay
//!   permanently refused over MCP — use the report-only repair scan to find
//!   them; the tool never rewrites a file it cannot token-verify.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use chrono::SecondsFormat;
use serde_json::Value;

use crate::vault::{
    safe_vault_path, PathMode, SafePathError, AGENTS_DEPOSIT_DIR, IMMUTABLE_DIR,
    NOTE_WRITABLE_SUBDIRS, READABLE_SUBDIRS, RECORDS_DIR, WIKI_DIR,
};

/// Top-level vault folders `write_note` may target (F2 allow-list, spec
/// 2026-09-27-vault-ingest-policy). The first path component must be one of
/// these; anything else — e.g. a deposit to the retired flat `agents/…`
/// layout — is refused before any filesystem access, naming the roots.
/// Within `immutable-source-files`, writes stay constrained to the
/// `immutable-source-files/agents/**` deposit prefix by `NOTE_WRITABLE_SUBDIRS`.
pub const NOTE_WRITABLE_ROOTS: &[&str] = &[IMMUTABLE_DIR, RECORDS_DIR, WIKI_DIR];

/// Context-compaction markers (issue #240): text a truncated agent payload
/// can carry into a note. A create containing any of these is refused; an
/// edit may only introduce a marker the existing content already contains.
pub const COMPACTION_MARKERS: &[&str] = &["[SKILL_PRUNED]", "HERMES-CONTEXT-COMPRESSION"];
/// Edits of notes whose body is smaller than this are never shrink-guarded:
/// a legitimate full rewrite of a small note must stay possible without
/// `allow_shrink` (the incident this guard replays was 12,860 bytes).
pub const MIN_GUARDED_BODY_BYTES: usize = 1024;

/// Every frontmatter key the typed `OkfFrontmatter` can render. ONE list,
/// shared by `check_round_trip`, [`rendered_key_set`] and the issue #245
/// key-drop guard — a second copy is exactly the drift the shared helper
/// exists to prevent. Pre-sorted ascending (issue #231 review bonus): the
/// exact-set zip in `check_round_trip` compares against this order directly,
/// so the literal must never be reshuffled without keeping it sorted.
const KNOWN_KEYS: [&str; 8] = [
    "created_at",
    "entity_type",
    "okf_version",
    "profile",
    "supersedes",
    "tags",
    "title",
    "updated_at",
];

use super::{
    parse_frontmatter, render_frontmatter, sha256_hash, validate_frontmatter, OkfFrontmatter,
    UpsertError, UpsertResult, WriteNoteError, WriteNoteResult,
};

/// Render a note document: strict YAML frontmatter fence + body.
///
/// `render_frontmatter` already emits the trailing `---\n`; append the body
/// and ensure at least one terminating newline (added only if missing —
/// trailing newlines are never collapsed).
fn render_document(frontmatter: &OkfFrontmatter, body: &str) -> String {
    let mut doc = render_frontmatter(frontmatter);
    if !body.is_empty() {
        doc.push_str(body);
    }
    if !doc.ends_with('\n') {
        doc.push('\n');
    }
    doc
}

/// Why an existing note's If-Match token could not be read.
#[derive(Debug)]
pub(crate) enum TokenReadError {
    /// No `---` fence found within the 64-line collection cap.
    NoFence,
    /// Fence found but no usable `updated_at:` token line.
    NoToken,
    /// Duplicate `updated_at:` lines or a non-RFC-3339 value.
    Unparsable,
}

impl std::fmt::Display for TokenReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TokenReadError::NoFence => "no_fence",
            TokenReadError::NoToken => "no_token",
            TokenReadError::Unparsable => "parse",
        };
        write!(f, "{s}")
    }
}

/// Read the If-Match token from an existing document.
///
/// The frontmatter fence is collected EXACTLY ONCE (a `lines()` loop with the
/// `take(64)` cap — `str::lines()` strips `\r`, so CRLF notes work) and that
/// ONE buffer feeds BOTH the strict parse and the tolerant fallback, so the
/// two paths can never see different fences. The closing fence must be the
/// EXACT line `---` (`----` or `---foo` is content, not a fence).
///
/// Strict parse FIRST (so hand-edited values like `updated_at: X # note`
/// behave exactly as today); only on failure, a tolerant line-scan with all
/// of: same fence collection incl. the `lines.take(64)` cap; `updated_at:`
/// at column 0; exactly one occurrence; quotes stripped; RFC 3339 required.
/// ONE function serves enforce_staleness AND prev_token so the two can
/// never disagree (differential test in Task 7).
pub(crate) fn read_existing_token(content: &str) -> Result<String, TokenReadError> {
    let Some(inner) = collect_frontmatter_fence(content) else {
        return Err(TokenReadError::NoFence);
    };
    if let Ok(fm) = crate::okf::parse_frontmatter(&inner) {
        if let Some(token) = fm.updated_at {
            return Ok(token);
        }
        return Err(TokenReadError::NoToken);
    }
    // Tolerant fallback (issue #231 healing path) over the SAME buffer the
    // strict parse saw.
    let mut hits: Vec<&str> = Vec::new();
    for line in inner.lines() {
        if let Some(rest) = line.strip_prefix("updated_at:") {
            hits.push(rest.trim());
        }
    }
    if hits.len() != 1 {
        return Err(if hits.is_empty() {
            TokenReadError::NoToken
        } else {
            TokenReadError::Unparsable // duplicates: refuse to pick
        });
    }
    let raw = hits[0].trim();
    let unquoted = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
        .unwrap_or(raw);
    chrono::DateTime::parse_from_rfc3339(unquoted)
        .map(|_| unquoted.to_string())
        .map_err(|_| TokenReadError::Unparsable)
}

/// Collect the text between the opening `---` line and the closing `---`
/// line of a document's frontmatter, or `None` if either fence is missing
/// within the 64-line cap. `str::lines()` handles `\r\n` line endings
/// (it strips a trailing `\r`), so CRLF notes collect identically to LF
/// notes; the closing fence must be the EXACT line `---`.
///
/// The `take(64)` cap is INTENTIONAL and applies to BOTH paths: the guard in
/// `check_round_trip` and the token reader share this one fence view, so a
/// fence that never closes within 64 frontmatter lines is "no fence"
/// everywhere (`existing_unparsable:no_fence`), never a partial parse.
fn collect_frontmatter_fence(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return None;
    }
    let mut closed = false;
    let mut inner = String::new();
    for line in lines.take(64) {
        if line == "---" {
            closed = true;
            break;
        }
        inner.push_str(line);
        inner.push('\n');
    }
    if !closed {
        // Same take(64) cap for both paths: no closing fence within the cap
        // is "no fence".
        return None;
    }
    Some(inner)
}

/// Issue #245 D1: the EXISTING note's frontmatter key set, or `None` when
/// there is no fence (unreachable through `write_note`: `enforce_staleness`
/// refuses `existing_unparsable:no_fence` first). Reads the SAME fence view
/// as the token reader. Three tiers:
/// - strict `serde_yaml::Value` mapping → its keys. NOT `parse_frontmatter`
///   (that silently drops unknown keys). Non-string keys (`1: x`) are
///   collected in Debug form — never rejected: on the existing side they are
///   data to protect, unlike `check_round_trip`'s rendered-output reject.
/// - parse fails or is not a mapping → column-0 line scan
///   ([`line_scan_keys`]), so damaged notes stay guarded, never skipped.
///
/// Both tiers apply the D2 normalization to the KNOWN optional fields only:
/// an empty `tags`/`supersedes` counts ABSENT (see [`strict_value_absent`],
/// [`line_scan_value_absent`]); every other key counts PRESENT.
fn existing_frontmatter_keys(content: &str) -> Option<BTreeSet<String>> {
    let inner = collect_frontmatter_fence(content)?;
    if let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(&inner) {
        let mut keys = BTreeSet::new();
        for (key, value) in &map {
            match key.as_str() {
                Some(name) if strict_value_absent(name, value) => {}
                Some(name) => {
                    keys.insert(name.to_string());
                }
                None => {
                    keys.insert(format!("{key:?}"));
                }
            }
        }
        return Some(keys);
    }
    Some(line_scan_keys(&inner))
}

/// D2 strict-tier ABSENT forms — KNOWN optional fields only. `tags`: null /
/// `~` / empty / `[]` (the renderer omits both `None` and `Some(vec![])`).
/// `supersedes`: null / `~` / `""` (an empty pointer can never be re-sent —
/// `under_deposit("")` refuses it). Everything else is PRESENT.
fn strict_value_absent(key: &str, value: &serde_yaml::Value) -> bool {
    use serde_yaml::Value;
    match key {
        "tags" => {
            matches!(value, Value::Null) || matches!(value, Value::Sequence(s) if s.is_empty())
        }
        "supersedes" => {
            matches!(value, Value::Null) || matches!(value, Value::String(s) if s.is_empty())
        }
        _ => false,
    }
}

/// D1 tier 3: column-0 line scan over a damaged fence. A key line that is
/// followed by a continuation (any indented line, or a `-` list item) is
/// always PRESENT — that keeps block-valued keys from reading as empty.
fn line_scan_keys(inner: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = inner.lines().collect();
    let mut keys = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let Some((key, rest)) = line_scan_key(line) else {
            continue;
        };
        let continued = lines.get(i + 1).is_some_and(|next| {
            next.starts_with(char::is_whitespace) || *next == "-" || next.starts_with("- ")
        });
        if !continued && line_scan_value_absent(&key, rest.trim()) {
            continue;
        }
        keys.insert(key);
    }
    keys
}

/// D1 (d) key-extraction rule: a line is a key line iff it is non-empty,
/// starts at column 0 with a character other than whitespace, `#` or `-`,
/// and contains `:`. Key = text before the FIRST `:`, trimmed, one matching
/// pair of surrounding `"`/`'` stripped. NO character-class restriction
/// (`1`, `my-key`, `some key` are keys). Returns the raw rest after the colon.
fn line_scan_key(line: &str) -> Option<(String, &str)> {
    let first = line.chars().next()?;
    if first.is_whitespace() || first == '#' || first == '-' {
        return None;
    }
    let (raw, rest) = line.split_once(':')?;
    let raw = raw.trim();
    let key = ['"', '\'']
        .iter()
        .find_map(|q| {
            raw.strip_prefix(*q)
                .and_then(|r| r.strip_suffix(*q))
                .filter(|_| raw.len() >= 2)
        })
        .unwrap_or(raw);
    Some((key.to_string(), rest))
}

/// D1 (b) tier-3 ABSENT forms — same known-field restriction as the strict
/// tier. The inline value is compared verbatim after trimming; trailing
/// comments are NOT stripped (`tags: [] # none` counts PRESENT here — the
/// accepted stricter-direction divergence, D1 (c)).
fn line_scan_value_absent(key: &str, value: &str) -> bool {
    match key {
        "tags" => matches!(value, "" | "null" | "~" | "[]"),
        "supersedes" => matches!(value, "" | "null" | "~" | "\"\""),
        _ => false,
    }
}

/// Split `content` into `(frontmatter_inner, body_start_offset)` where
/// `body_start_offset` is the byte offset at which the note body begins
/// (immediately after the closing `---` line). Same fence rules as
/// [`collect_frontmatter_fence`]: exact `---` opener, closing fence within
/// 64 lines, no partial parse on over-cap fences. Returns `None` when there
/// is no fence; callers treat the WHOLE content as body (offset 0).
///
/// The offset is byte-exact on CRLF files — unlike
/// [`collect_frontmatter_fence`], whose `lines()` view drops `\r`, this
/// helper computes offsets on raw bytes, so `content.len() - offset` is the
/// true body byte length (issue #240: the size-drop guard measures bytes).
/// Consumed by [`enforce_size_drop`] (Task 3); no allow needed anymore.
fn split_frontmatter_fence(content: &str) -> Option<(String, usize)> {
    // Review m1: initialize in ONE expression — `let mut offset = 0usize;`
    // followed by unconditional reassignment trips `unused_assignments`,
    // which is fatal under the repo's clippy -D warnings gate.
    let mut offset = if content.as_bytes().starts_with(b"---\r\n") {
        5
    } else if content.as_bytes().starts_with(b"---\n") {
        4
    } else {
        return None;
    };
    let mut inner = String::new();
    for _ in 0..64 {
        if offset >= content.len() {
            return None;
        }
        let line_end = content[offset..]
            .find('\n')
            .map_or(content.len(), |i| offset + i);
        let line = &content[offset..line_end];
        let trimmed = line.strip_suffix('\r').unwrap_or(line);
        let next = if line_end < content.len() {
            line_end + 1
        } else {
            line_end
        };
        if trimmed == "---" {
            return Some((inner, next));
        }
        inner.push_str(trimmed);
        inner.push('\n');
        offset = next;
    }
    None
}

/// True body byte length of a rendered note: everything after the
/// frontmatter fence, or the whole content when fence-less.
/// Consumed by Task 3's guards; see the allow note on
/// [`split_frontmatter_fence`].
fn body_bytes(content: &str) -> usize {
    match split_frontmatter_fence(content) {
        Some((_, offset)) => content.len() - offset,
        None => content.len(),
    }
}

/// Enforce If-Match staleness on the existing file's `updated_at` token.
///
/// Rules (spec v2 §B.2, resolved rulings):
/// - File absent → edit proceeds (this is a create).
/// - Token supplied → must EXACTLY match the existing token, else
///   [`WriteNoteError::StaleUpdate`] carries the current token.
/// - No token supplied but the file exists → refused as stale: an edit
///   requires proof the writer saw the current revision.
fn enforce_staleness(
    existing_content: Option<&str>,
    expected_updated_at: Option<&str>,
) -> Result<(), WriteNoteError> {
    let Some(content) = existing_content else {
        return Ok(()); // create path — nothing to be stale against
    };
    let current = read_existing_token(content)
        .map_err(|e| WriteNoteError::InvalidFrontmatter(format!("existing_unparsable:{}", e)))?;
    match expected_updated_at {
        Some(expected) if expected == current => Ok(()),
        _ => Err(WriteNoteError::StaleUpdate {
            updated_at: current,
        }),
    }
}

/// Issue #240: refuse a rendered document that introduces a context-
/// compaction marker absent from the existing content (creates: any marker).
/// The scan covers frontmatter AND body on both sides — a note quoting a
/// marker in its title must stay editable. `allow_shrink` does NOT bypass
/// this check.
fn enforce_compaction_markers(
    document: &str,
    existing: Option<&str>,
) -> Result<(), WriteNoteError> {
    let already = existing.unwrap_or("");
    for marker in COMPACTION_MARKERS {
        if document.contains(marker) && !already.contains(marker) {
            return Err(WriteNoteError::CompactionMarkerRejected {
                marker: (*marker).to_string(),
            });
        }
    }
    Ok(())
}

/// Issue #240: refuse an edit whose rendered body shrank below half its
/// size — the signature of a truncated payload — unless the caller
/// explicitly opted in. Applies only to existing bodies ≥
/// [`MIN_GUARDED_BODY_BYTES`]; smaller notes may be fully rewritten.
fn enforce_size_drop(
    existing: Option<&str>,
    document: &str,
    allow_shrink: bool,
) -> Result<(), WriteNoteError> {
    let Some(existing) = existing else {
        return Ok(()); // create path — nothing to shrink against
    };
    let existing_bytes = body_bytes(existing);
    if existing_bytes < MIN_GUARDED_BODY_BYTES {
        return Ok(());
    }
    let new_bytes = body_bytes(document);
    if allow_shrink || new_bytes * 2 >= existing_bytes {
        return Ok(());
    }
    Err(WriteNoteError::ShrinkRefused {
        existing_bytes,
        new_bytes,
    })
}

/// Issue #245: refuse an edit whose payload drops frontmatter keys the
/// existing note carries, unless the caller explicitly confirmed. Compares
/// the existing key set ([`existing_frontmatter_keys`]) against the keys the
/// INCOMING struct renders ([`rendered_key_set`]); `updated_at` is exempt
/// (rotated on every write). KNOWN dropped keys are reported before UNKNOWN
/// ones — `allow_key_drop` bypasses BOTH, so this precedence is what keeps
/// known keys from being lost behind an unrepresentable-key message.
fn enforce_key_preservation(
    existing: Option<&str>,
    incoming: &OkfFrontmatter,
    allow_key_drop: bool,
) -> Result<(), WriteNoteError> {
    let Some(existing_keys) = existing.and_then(existing_frontmatter_keys) else {
        return Ok(()); // create path, or fence-less (refused upstream)
    };
    if allow_key_drop {
        return Ok(());
    }
    let incoming_keys = rendered_key_set(incoming);
    let (mut known, mut unknown) = (Vec::new(), Vec::new());
    // BTreeSet iteration is sorted, so both partitions come out sorted.
    for key in existing_keys {
        if key == "updated_at" || incoming_keys.contains(&key.as_str()) {
            continue;
        }
        if KNOWN_KEYS.contains(&key.as_str()) {
            known.push(key);
        } else {
            unknown.push(key);
        }
    }
    if !known.is_empty() {
        return Err(WriteNoteError::KeyDropRefused { keys: known });
    }
    if !unknown.is_empty() {
        return Err(WriteNoteError::KeyDropUnrepresentable { keys: unknown });
    }
    Ok(())
}

/// True iff `path` is inside the deposit folder at any depth (incl. subfolders,
/// allowed per Kurt's Aug 29 2026 directive; amended spec
/// `2026-08-27-agent-deposit-write-path.md` §AMENDED 2026-08-29).
fn under_deposit(path: &str) -> bool {
    under_any(path, &[AGENTS_DEPOSIT_DIR])
}

/// True iff `path` lies at any depth under one of `allowed_subdirs`.
/// Component-based, so a sibling prefix (`immutable-source-files/agents-evil`)
/// never matches an allowed root (`immutable-source-files/agents`).
///
/// `Component::CurDir` is dropped first: `Path::components` normalizes interior
/// `.` away but KEEPS a leading one, so `./wiki/x.md` would otherwise compare as
/// `[".", "wiki", ...]` and fail to match `wiki`. `safe_vault_path` accepts a
/// leading `./` (it only rejects `ParentDir`/`Prefix`), so this check must too.
fn under_any(path: &str, allowed_subdirs: &[&str]) -> bool {
    let comps: Vec<&str> = Path::new(path)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    allowed_subdirs.iter().any(|sub| {
        let prefix: Vec<&str> = sub.split('/').collect();
        comps.len() > prefix.len() && comps[..prefix.len()] == prefix[..]
    })
}

/// Create `rel_parent` under `vault_root` one component at a time, refusing to
/// traverse a symlinked component.
///
/// `std::fs::create_dir_all` follows symlinks on components that already exist,
/// so a symlink planted inside the vault (by a sync conflict, a restored backup,
/// or the user) would let directories be created *outside* the vault root. The
/// round-two `safe_vault_path` call still rejects the write, so no file is ever
/// written there — but the out-of-vault directories would persist. Creating the
/// chain stepwise keeps every side effect of a rejected write inside the vault.
///
/// NOT atomic: each component is stat'd then created, so a writer racing this
/// loop could swap a just-created directory for a symlink before the next
/// `mkdir` follows it. Closing that window needs `mkdirat(_, O_NOFOLLOW)` per
/// component; `create_dir_all` had the identical exposure, so this is a
/// narrowing, not a guarantee. Local vault write access is required to exploit.
fn create_parents_no_symlink(vault_root: &Path, rel_parent: &Path) -> std::io::Result<()> {
    let mut cur = vault_root.to_path_buf();
    // `rel_parent` is already vetted by safe_vault_path: relative, no `..`, no
    // prefix components — so every component here is a plain name.
    for comp in rel_parent.components() {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("symlinked path component: {}", cur.display()),
                ));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("path component is not a directory: {}", cur.display()),
                ));
            }
            // Only a genuine absence means "create it"; an EACCES/ELOOP from
            // stat must surface as itself, not as a confusing create_dir error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&cur)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// First path component of `path`, ONLY when it is a plain name. A leading
/// `..` (traversal) or `/` (absolute) yields `None` so those shapes fall
/// through to `safe_vault_path`, which maps them to the pinned
/// `PathOutsideVault` error — the F2 gate never re-labels a traversal. A
/// leading `./` is skipped so it does not change which root a path lands in
/// (see `dot_prefixed_path_with_missing_parents_still_writes`).
fn first_path_segment(path: &str) -> Option<String> {
    Path::new(path)
        .components()
        .find(|c| !matches!(c, Component::CurDir))
        .filter(|c| matches!(c, Component::Normal(_)))
        .and_then(|c| c.as_os_str().to_str())
        .map(str::to_string)
}

/// Write a note with OKF frontmatter to the vault (single core, spec v2).
///
/// * `vault_root` — absolute path to the vault root.
/// * `path` — vault-relative path (e.g. `wiki/my-note.md` or
///   `immutable-source-files/agents/my-note.md`). Agent deposits may nest at
///   any depth under `agents/` (per-agent subfolders allowed; amended spec
///   `2026-08-27-agent-deposit-write-path.md` §AMENDED 2026-08-29). Validated
///   with `safe_vault_path(_, _, NOTE_WRITABLE_SUBDIRS, PathMode::MayCreate)`.
///   Missing parent directories are created component-by-component without
///   traversing symlinks (see [`create_parents_no_symlink`]), then the
///   resolution is repeated so every containment/symlink decision stays inside
///   `safe_vault_path`.
/// * `frontmatter` — OKF frontmatter; validated; `updated_at` defaults to
///   now (RFC 3339, UTC) when omitted.
/// * `body` — markdown body; normalized to end with exactly one `\n`.
/// * `expected_updated_at` — If-Match token: required to equal the existing
///   file's token when the file already exists (see [`enforce_staleness`]).
/// * `allow_shrink` — issue #240 confirmation for a >50% body shrink.
/// * `allow_key_drop` — issue #245 confirmation for dropping frontmatter keys
///   the existing note carries (see [`enforce_key_preservation`]).
pub fn write_note(
    vault_root: &Path,
    path: &str,
    frontmatter: &OkfFrontmatter,
    body: &str,
    expected_updated_at: Option<&str>,
    allow_shrink: bool,
    allow_key_drop: bool,
) -> Result<WriteNoteResult, WriteNoteError> {
    validate_frontmatter(frontmatter).map_err(WriteNoteError::InvalidFrontmatter)?;

    // F2 top-level allow-list (spec 2026-09-27-vault-ingest-policy): the
    // FIRST path segment must name an allowed root. Checked before any
    // filesystem access so a structural mistake (the retired flat
    // `agents/…` deposit layout, a stray `people/…` note) fails loudly
    // with the roots named instead of silently forking the ontology.
    // Within `immutable-source-files`, the deposit-prefix constraint
    // (`immutable-source-files/agents/**`) is still enforced below by
    // `NOTE_WRITABLE_SUBDIRS`.
    if let Some(first) = first_path_segment(path) {
        if !NOTE_WRITABLE_ROOTS.contains(&first.as_str()) {
            return Err(WriteNoteError::DisallowedRoot {
                allowed: NOTE_WRITABLE_ROOTS.join(", "),
                first_segment: first,
            });
        }
    }

    // Validate supersession: deposit-to-deposit only, target must exist.
    if let Some(ref supersedes_path) = frontmatter.supersedes {
        // Both ends must be deposits (component-based check; string
        // `starts_with` would accept sibling prefixes like `agents-evil/`).
        if !under_deposit(supersedes_path) {
            return Err(WriteNoteError::InvalidFrontmatter(format!(
                "supersedes must reference a deposit under {}: got {}",
                AGENTS_DEPOSIT_DIR, supersedes_path
            )));
        }
        if !under_deposit(path) {
            return Err(WriteNoteError::InvalidFrontmatter(format!(
                "supersedes is deposit-only: note path must be a deposit under {}: got {}",
                AGENTS_DEPOSIT_DIR, path
            )));
        }

        // Resolve the target with the deposit-only allowlist. MustExist
        // already guarantees is_file() (safe_vault_path rejects dirs and
        // non-regular files), so the resolution error IS the not-found case.
        safe_vault_path(
            vault_root,
            supersedes_path,
            &[AGENTS_DEPOSIT_DIR],
            PathMode::MustExist,
        )
        .map_err(|_| {
            WriteNoteError::InvalidFrontmatter(format!("supersedes_not_found:{}", supersedes_path))
        })?;
    }

    let target = match safe_vault_path(vault_root, path, NOTE_WRITABLE_SUBDIRS, PathMode::MayCreate)
    {
        Ok(target) => target,
        Err(SafePathError::NotFound(ref msg)) if msg.contains("parent directory not found") => {
            // Parent dirs don't exist yet. Path shape was already vetted
            // (absolute/`..`/NUL/dot-enders reject before any FS access), so
            // create the parents and re-resolve; round two re-canonicalizes
            // and enforces containment + symlink rules inside safe_vault_path.
            //
            // Check containment LEXICALLY first: round two would reject an
            // out-of-tree path anyway, but only after the bootstrap had already
            // created the directories, leaving them behind on a rejected write.
            if !under_any(path, NOTE_WRITABLE_SUBDIRS) {
                return Err(WriteNoteError::PathOutsideVault);
            }
            let rel_parent = Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| {
                    WriteNoteError::WriteError(format!(
                        "write_error:missing parent component: {}",
                        path
                    ))
                })?;
            create_parents_no_symlink(vault_root, rel_parent).map_err(|e| {
                WriteNoteError::WriteError(format!("write_error:create_parents_no_symlink: {}", e))
            })?;
            safe_vault_path(vault_root, path, NOTE_WRITABLE_SUBDIRS, PathMode::MayCreate)
                .map_err(map_safe_err_note)?
        }
        Err(e) => return Err(map_safe_err_note(e)),
    };

    // Contract (spec v2 §B.2): never rewrite a file we cannot token-verify.
    // - NotFound → create path (Ok).
    // - Exists but not valid UTF-8 (InvalidData) → the bytes are unparsable
    //   BY CONSTRUCTION; refuse with `existing_unparsable:parse` (the same
    //   reason the repair scan reports it) instead of silently clobbering.
    // - Any other read error → refuse as a write error.
    let existing = match std::fs::read_to_string(&target) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(WriteNoteError::InvalidFrontmatter(
                "existing_unparsable:parse".to_string(),
            ));
        }
        Err(e) => return Err(WriteNoteError::WriteError(format!("write_error:{}", e))),
    };
    enforce_staleness(existing.as_deref(), expected_updated_at)?;

    // Rotate the token on EVERY successful write. Floor: the previous file
    // token, so the successor is always strictly newer even when both calls
    // land inside the same millisecond — a reused token can never verify.
    let prev_token = existing
        .as_deref()
        .and_then(|c| read_existing_token(c).ok());
    let now = chrono::Utc::now();
    let mut fresh = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    if let Some(prev) = prev_token
        .as_deref()
        .and_then(|p| chrono::DateTime::parse_from_rfc3339(p).ok())
    {
        if let Ok(cur) = chrono::DateTime::parse_from_rfc3339(&fresh) {
            if cur <= prev {
                fresh = (prev + chrono::Duration::milliseconds(1))
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
            }
        }
    }
    let mut effective_fm = frontmatter.clone();
    effective_fm.updated_at = Some(fresh.clone());
    if effective_fm.created_at.trim().is_empty() && existing.is_none() {
        return Err(WriteNoteError::InvalidFrontmatter(
            "created_at is required on create".to_string(),
        ));
    }

    let document = render_document(&effective_fm, body);
    check_round_trip(&effective_fm, &document)?;

    // Issue #240/#245 guards — AFTER render (the shrink measurement basis is
    // the rendered body) and BEFORE any bytes hit disk. Order pinned (#245
    // D6): marker (root cause, no bypass) → key-drop (more specific than a
    // shrink) → shrink.
    enforce_compaction_markers(&document, existing.as_deref())?;
    enforce_key_preservation(existing.as_deref(), frontmatter, allow_key_drop)?;
    enforce_size_drop(existing.as_deref(), &document, allow_shrink)?;

    crate::vault::safe_write_bytes(&target, document.as_bytes())
        .map_err(|e| WriteNoteError::WriteError(format!("write_error:{}", e)))?;

    Ok(WriteNoteResult {
        success: true,
        path: path.to_string(),
        sha256: sha256_hash(&document),
        updated_at: fresh,
    })
}

fn map_safe_err_note(e: SafePathError) -> WriteNoteError {
    match e {
        SafePathError::Absolute
        | SafePathError::Traversal
        | SafePathError::Outside
        | SafePathError::InvalidName
        | SafePathError::NotARegularFile => WriteNoteError::PathOutsideVault,
        SafePathError::NotFound(msg) => {
            WriteNoteError::WriteError(format!("write_error:not found: {}", msg))
        }
        SafePathError::Io(e) => WriteNoteError::WriteError(format!("write_error:{}", e)),
    }
}

/// The key set the renderer emits for `fm`, in [`KNOWN_KEYS`] order. serde
/// skips `tags` (None or empty), `updated_at` (None) and `supersedes` (None).
/// Shared by `check_round_trip` and the issue #245 key-drop guard (D2) so
/// the two computations cannot drift.
fn rendered_key_set(fm: &OkfFrontmatter) -> Vec<&'static str> {
    KNOWN_KEYS
        .iter()
        .copied()
        .filter(|k| {
            !((*k == "tags" && fm.tags.as_ref().is_none_or(|t| t.is_empty()))
                || (*k == "updated_at" && fm.updated_at.is_none())
                || (*k == "supersedes" && fm.supersedes.is_none()))
        })
        .collect()
}

/// Pre-write round-trip guard (issue #231): verify the rendered document's
/// frontmatter fence parses back to exactly the effective frontmatter.
///
/// Two checks, both pre-`safe_write_bytes`:
/// 1. Typed round-trip — `parse_frontmatter` on the fence body must equal the
///    effective frontmatter, after normalizing `Some(vec![])` tags to `None`
///    on BOTH sides (render drops empty tag lists; serde default reads the
///    omission back as `None`).
/// 2. Rendered key-set check — parse the fence into a raw
///    `serde_yaml::Mapping` and require the key set to match the known
///    frontmatter keys exactly. `OkfFrontmatter` has no `deny_unknown_fields`,
///    so check 1 alone would silently drop unknown keys; this catches
///    injected keys in the rendered output.
/// 3. Editability check — `read_existing_token` on the WHOLE document must
///    succeed, so a note that renders to something its own token reader
///    cannot read back is refused pre-write rather than bricking the file.
///    The fence view is collected with the same `collect_frontmatter_fence`
///    helper (incl. the 64-line cap) the token reader uses.
///
/// Any mismatch aborts the write with `WriteNoteError::InvalidFrontmatter`.
fn check_round_trip(effective_fm: &OkfFrontmatter, document: &str) -> Result<(), WriteNoteError> {
    // Fence-view parity (issue #231 review): collect the fence with the SAME
    // helper the token reader uses — `take(64)` cap, exact `---` closing
    // line, CRLF via `lines()` — so the guard can never accept a fence view
    // that a later `read_existing_token` would disagree with.
    let fenced = collect_frontmatter_fence(document).ok_or_else(|| {
        // Same condition the token reader maps to NoFence, same error string:
        // one fence condition, one contract error on both call sites.
        WriteNoteError::InvalidFrontmatter("existing_unparsable:no_fence".to_string())
    })?;

    // Check 2 — rendered key set must EQUAL the known key set exactly
    // (catches unknown-key injection the typed struct silently drops, AND
    // missing keys from a tampered render). Any rendered key that is not a
    // string (e.g. `1: x` parses an integer key) is rejected outright — the
    // renderer only ever emits string keys, so a non-string key is injection.
    let parsed_yaml: serde_yaml::Value = serde_yaml::from_str(&fenced).map_err(|e| {
        WriteNoteError::InvalidFrontmatter(format!("round_trip: yaml parse failed: {}", e))
    })?;
    let mapping = parsed_yaml.as_mapping().ok_or_else(|| {
        WriteNoteError::InvalidFrontmatter(
            "round_trip: rendered frontmatter is not a mapping".to_string(),
        )
    })?;
    let mut rendered_keys: Vec<String> = Vec::with_capacity(mapping.len());
    for key in mapping.keys() {
        let Some(key_str) = key.as_str() else {
            return Err(WriteNoteError::InvalidFrontmatter(format!(
                "round_trip: non-string frontmatter key in rendered output: {key:?}"
            )));
        };
        rendered_keys.push(key_str.to_string());
    }
    rendered_keys.sort();
    // Both directions: no unknown key, no missing key (exact set equality
    // against the normalized set the renderer emits — shared helper).
    let expected_keys = rendered_key_set(effective_fm);
    if rendered_keys.len() != expected_keys.len()
        || rendered_keys
            .iter()
            .zip(expected_keys.iter())
            .any(|(a, b)| a != b)
    {
        return Err(WriteNoteError::InvalidFrontmatter(format!(
            "round_trip: key set mismatch — expected: {expected_keys:?}, got: {rendered_keys:?}"
        )));
    }

    // Hardening (issue #231 review): the note we are about to write MUST be
    // readable back by the SAME token reader the next edit will use, to the
    // SAME token. With shared fence views this is unreachable when checks
    // 1–2 pass (defense-in-depth pinning the invariant), but a future edit
    // that breaks parity fails here instead of bricking the file.
    let read_back = read_existing_token(document).map_err(|e| {
        WriteNoteError::InvalidFrontmatter(format!(
            "round_trip: written note would be uneditable: {e}"
        ))
    })?;
    if Some(&read_back) != effective_fm.updated_at.as_ref() {
        return Err(WriteNoteError::InvalidFrontmatter(
            "round_trip: written note would be uneditable: token mismatch".to_string(),
        ));
    }

    // Check 1 — typed round-trip with empty-tags normalization on both sides.
    let mut parsed = parse_frontmatter(&fenced)
        .map_err(|e| WriteNoteError::InvalidFrontmatter(format!("round_trip: {}", e)))?;
    if parsed.tags.as_ref().is_some_and(|t| t.is_empty()) {
        parsed.tags = None;
    }
    let mut expected = effective_fm.clone();
    if expected.tags.as_ref().is_some_and(|t| t.is_empty()) {
        expected.tags = None;
    }
    if parsed != expected {
        return Err(WriteNoteError::InvalidFrontmatter(
            "round_trip: parsed frontmatter does not match effective frontmatter".to_string(),
        ));
    }
    Ok(())
}

/// Validate an index entry name (pinned error: `invalid_entry_name`).
///
/// Rejects empty/whitespace names, surrounding whitespace, newline injection,
/// and `#` (would forge headers or comments).
fn validate_entry_name(entry_name: &str) -> Result<(), UpsertError> {
    // Spec v2: letters, digits, spaces, underscore, hyphen, dot. Everything
    // else (including '#', newlines, leading/trailing whitespace) is refused.
    let valid = !entry_name.is_empty()
        && entry_name.trim() == entry_name
        && entry_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'));
    if valid {
        Ok(())
    } else {
        Err(UpsertError::InvalidEntryName)
    }
}

/// Render the pinned index entry block (spec v2 §C.4). Always `\n`-terminated.
fn render_index_entry_block(
    entry_name: &str,
    entry_path: &str,
    entry_type: &str,
    metadata: Option<&Value>,
) -> Result<String, UpsertError> {
    let mut block = format!(
        "## {}\n[[{}]]\n- Type: {}\n",
        entry_name, entry_path, entry_type
    );
    if let Some(metadata) = metadata {
        if !metadata.is_null() {
            let map = metadata.as_object().ok_or_else(|| {
                UpsertError::InvalidMetadata("metadata must be a JSON object".to_string())
            })?;
            for (key, value) in map {
                let rendered = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let key = key.trim();
                if key.is_empty() || key.contains('\n') {
                    return Err(UpsertError::InvalidMetadata(format!(
                        "invalid metadata key: {:?}",
                        key
                    )));
                }
                // Legacy-visible form: keys render capitalized ("- Status:").
                let mut display = key.to_string();
                if let Some(first) = display.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                block.push_str(&format!("- {}: {}\n", display, rendered.replace('\n', " ")));
            }
        }
    }
    Ok(block)
}

/// Whole-line scan for the entry header. Returns the 0-based line index of
/// `## {entry_name}` or `None`. Exact-match only: no regex, no substrings.
fn find_entry_header_line(content: &str, entry_name: &str) -> Option<usize> {
    let header = format!("## {}", entry_name);
    content.lines().position(|line| line == header)
}

/// Replace-or-append the pinned block using whole-line matching.
///
/// Update: replaces from the matched header line through the line before the
/// next `## ` header (or EOF). Append: one blank line, then the block at EOF.
/// Returns `(new_content, appended, header_line_number_1based)`.
fn upsert_entry_in_content(content: &str, entry_name: &str, block: &str) -> (String, bool, usize) {
    let Some(header_idx) = find_entry_header_line(content, entry_name) else {
        let mut new_content = String::from(content);
        if !new_content.ends_with('\n') && !new_content.is_empty() {
            new_content.push('\n');
        }
        if !new_content.ends_with("\n\n") {
            new_content.push('\n');
        }
        new_content.push_str(block);
        let line_number = content.lines().count() + 2;
        return (new_content, true, line_number);
    };

    let lines: Vec<&str> = content.lines().collect();
    let next_header_idx = lines[header_idx + 1..]
        .iter()
        .position(|line| line.starts_with("## "))
        .map(|offset| header_idx + 1 + offset)
        .unwrap_or(lines.len());

    let mut out = String::new();
    for line in &lines[..header_idx] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(block);
    for line in &lines[next_header_idx..] {
        out.push_str(line);
        out.push('\n');
    }
    (out, false, header_idx + 1)
}

/// Upsert one entry into an existing vault INDEX.md (single core, spec v2).
///
/// * `index_path` — vault-relative; the file MUST exist (never auto-created;
///   pinned error `index_not_found:{path}`).
/// * `entry_path` — vault-relative note path the entry links to; must resolve
///   to a regular file inside the vault.
/// * Matching/replacement semantics: see [`find_entry_header_line`] /
///   [`upsert_entry_in_content`]. Atomic via `safe_write_bytes`.
#[allow(clippy::too_many_arguments)]
pub fn upsert_index_entry(
    vault_root: &Path,
    index_path: &str,
    entry_name: &str,
    entry_path: &str,
    entry_type: &str,
    metadata: Option<&Value>,
) -> Result<UpsertResult, UpsertError> {
    validate_entry_name(entry_name)?;
    if entry_type.trim().is_empty() || entry_type.contains(['\n', '\r']) {
        return Err(UpsertError::InvalidMetadata(format!(
            "invalid entry type: {:?}",
            entry_type
        )));
    }

    let canonical_index = match safe_vault_path(
        vault_root,
        index_path,
        READABLE_SUBDIRS,
        PathMode::MustExist,
    ) {
        Ok(p) => p,
        Err(SafePathError::NotFound(_)) => {
            return Err(UpsertError::IndexNotFound(index_path.to_string()))
        }
        Err(e) => return Err(map_safe_err_upsert(e)),
    };
    let canonical_entry_target = safe_vault_path(
        vault_root,
        entry_path,
        READABLE_SUBDIRS,
        PathMode::MustExist,
    )
    .map_err(map_safe_err_upsert)?;

    let content = std::fs::read_to_string(&canonical_index)
        .map_err(|e| UpsertError::WriteError(format!("write_error:read: {}", e)))?;

    let block = render_index_entry_block(entry_name, entry_path, entry_type, metadata)?;
    let (new_content, appended, line_number) =
        upsert_entry_in_content(&content, entry_name, &block);

    // Sanity: referenced note must exist before the index points at it.
    debug_assert!(canonical_entry_target.exists());

    crate::vault::safe_write_bytes(&canonical_index, new_content.as_bytes())
        .map_err(|e| UpsertError::WriteError(format!("write_error:{}", e)))?;

    Ok(UpsertResult {
        success: true,
        index_path: index_path.to_string(),
        entry_id: entry_name.to_string(),
        appended,
        line_number: Some(line_number),
    })
}

fn map_safe_err_upsert(e: SafePathError) -> UpsertError {
    match e {
        SafePathError::Absolute
        | SafePathError::Traversal
        | SafePathError::Outside
        | SafePathError::InvalidName
        | SafePathError::NotARegularFile => UpsertError::PathOutsideVault,
        SafePathError::NotFound(msg) => {
            UpsertError::WriteError(format!("write_error:not found: {}", msg))
        }
        SafePathError::Io(e) => UpsertError::WriteError(format!("write_error:{}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    /// Issue #245 D4: `write_note` takes two adjacent guard bools — pass them
    /// by NAME, never as literals, so a swap reads wrong in review.
    const NO_SHRINK: bool = false;
    const NO_KEY_DROP: bool = false;

    #[test]
    fn token_read_strict_parse_wins_on_clean_fence() {
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: T\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z # trailing comment\n---\n";
        // Hand-edited trailing comment: strict parser yields the bare value.
        assert_eq!(
            super::read_existing_token(doc).unwrap(),
            "2026-09-25T01:00:00Z"
        );
    }

    #[test]
    fn token_read_tolerant_recovers_from_broken_colon_title() {
        // The issue-#231 shape: unquoted colon title breaks the strict parse.
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Deploy: retro\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\n---\n";
        assert_eq!(
            super::read_existing_token(doc).unwrap(),
            "2026-09-25T01:00:00Z"
        );
    }

    #[test]
    fn token_read_tolerant_rejects_duplicate_or_noncol0_or_bad_rfc3339() {
        let base = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n";
        // duplicate updated_at lines
        assert!(matches!(
            super::read_existing_token(&format!(
                "{base}updated_at: 2026-09-25T01:00:00Z\nupdated_at: 2026-09-25T02:00:00Z\n---\n"
            )),
            Err(super::TokenReadError::Unparsable)
        ));
        // key not at column 0
        assert!(matches!(
            super::read_existing_token(&format!("{base}  updated_at: 2026-09-25T01:00:00Z\n---\n")),
            Err(super::TokenReadError::NoToken)
        ));
        // value not RFC 3339
        assert!(matches!(
            super::read_existing_token(&format!("{base}updated_at: not-a-date\n---\n")),
            Err(super::TokenReadError::Unparsable)
        ));
        // no fence at all
        assert!(matches!(
            super::read_existing_token("just some text\n"),
            Err(super::TokenReadError::NoFence)
        ));
    }

    #[test]
    fn token_read_tolerant_obeys_64_line_cap() {
        // Same take(64) cap as the fence collector (write.rs:61): a closing
        // fence beyond 64 lines is treated exactly like the strict path —
        // no fence ⇒ NoFence.
        let mut doc = String::from("---\ntitle: a: b\n");
        for i in 0..70 {
            doc.push_str(&format!("k{i}: v{i}\n"));
        }
        doc.push_str("---\n");
        assert!(matches!(
            super::read_existing_token(&doc),
            Err(super::TokenReadError::NoFence)
        ));
    }

    /// MAJOR-1 (issue #231 review) — CRLF fixtures must read their token on
    /// the STRICT path (clean fence, CRLF line endings). The pre-review code
    /// used `strip_prefix("---\n")` and turned `---\r\n` into NoFence.
    #[test]
    fn token_read_crlf_fence_strict_path() {
        let doc = "---\r\nokf_version: 0.1\r\nprofile: llm-wiki/1\r\ntitle: T\r\nentity_type: fact\r\ncreated_at: 2026-09-25T00:00:00Z\r\nupdated_at: 2026-09-25T01:00:00Z\r\n---\r\nbody\r\n";
        assert_eq!(
            super::read_existing_token(doc).unwrap(),
            "2026-09-25T01:00:00Z"
        );
    }

    /// MAJOR-1 — CRLF fixtures must ALSO read their token on the tolerant
    /// fallback path (the issue-#231 colon-title shape, CRLF line endings).
    #[test]
    fn token_read_crlf_fence_tolerant_path() {
        let doc = "---\r\nokf_version: 0.1\r\nprofile: llm-wiki/1\r\ntitle: Deploy: retro\r\nentity_type: fact\r\ncreated_at: 2026-09-25T00:00:00Z\r\nupdated_at: 2026-09-25T01:00:00Z\r\n---\r\nbody\r\n";
        assert_eq!(
            super::read_existing_token(doc).unwrap(),
            "2026-09-25T01:00:00Z"
        );
    }

    /// MAJOR-1 — a CRLF note on disk must remain EDITABLE through
    /// `write_note`: scrape the token from the CRLF file and edit again.
    #[test]
    fn crlf_note_remains_editable_through_write_note() {
        let (_g, root) = vault();
        let create = write_note(
            &root,
            "wiki/crlf.md",
            &fm("CRLF Note", None),
            "v1\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .expect("create succeeds");
        let lf = fs::read_to_string(root.join("wiki/crlf.md")).unwrap();
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(lf, crlf, "fixture must actually be CRLF");
        fs::write(root.join("wiki/crlf.md"), &crlf).unwrap();
        let on_disk = fs::read_to_string(root.join("wiki/crlf.md")).unwrap();
        let token = read_existing_token(&on_disk).expect("CRLF token readable");
        assert_eq!(token, create.updated_at);
        let edit = write_note(
            &root,
            "wiki/crlf.md",
            &fm("CRLF Note", None),
            "v2\n",
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        );
        assert!(
            edit.is_ok(),
            "CRLF note must stay editable: {:?}",
            edit.err()
        );
    }

    /// MAJOR-1 (doc contradiction) — the collected-fence close requires the
    /// EXACT line `---`; a `----` rule-off line is NOT a closing fence
    /// (the old `split_once("\n---")` matched it, and paths then disagreed).
    #[test]
    fn fence_close_requires_exact_dashes() {
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n----\n";
        assert!(matches!(
            super::read_existing_token(doc),
            Err(super::TokenReadError::NoFence)
        ));
    }

    #[test]
    fn split_fence_lf_content() {
        let content = "---\ntitle: T\n---\nbody here\n";
        let (inner, offset) = split_frontmatter_fence(content).unwrap();
        assert_eq!(inner, "title: T\n");
        assert_eq!(&content[offset..], "body here\n");
    }

    #[test]
    fn split_fence_crlf_content_byte_exact() {
        let content = "---\r\ntitle: T\r\n---\r\nbody here\r\n";
        let (inner, offset) = split_frontmatter_fence(content).unwrap();
        assert_eq!(inner, "title: T\n");
        assert_eq!(&content[offset..], "body here\r\n");
    }

    #[test]
    fn split_fence_none_without_opener() {
        assert!(split_frontmatter_fence("no fence\n").is_none());
    }

    #[test]
    fn split_fence_none_without_closer_within_cap() {
        let mut content = String::from("---\n");
        for i in 0..70 {
            content.push_str(&format!("k{i}: v\n"));
        }
        assert!(split_frontmatter_fence(&content).is_none());
    }

    #[test]
    fn split_fence_body_bytes_helper_matches() {
        // body_bytes is implemented in THIS task (Task 1 Step 3); this test pins the contract early.
        let content = "---\r\ntitle: T\r\n---\r\n0123456789\r\n";
        assert_eq!(body_bytes(content), "0123456789\r\n".len());
    }

    /// MAJOR-2 (issue #231 review) — a target file that exists but is NOT
    /// valid UTF-8 must be REFUSED (`existing_unparsable:parse`), never
    /// silently clobbered as a "create".
    #[test]
    fn write_refuses_non_utf8_existing_file_without_clobbering() {
        let (_g, root) = vault();
        let target = root.join("wiki/binary.md");
        let original: &[u8] = b"---\r\n\xff\xfe not utf8 \x00---\r\ngarbage\r\n";
        fs::write(&target, original).unwrap();
        let err = write_note(
            &root,
            "wiki/binary.md",
            &fm("Clobber", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .expect_err("non-UTF-8 target must be refused, not overwritten");
        assert!(
            matches!(&err, WriteNoteError::InvalidFrontmatter(detail) if detail == "existing_unparsable:parse"),
            "got: {err}"
        );
        assert_eq!(
            fs::read(&target).unwrap().as_slice(),
            original,
            "refused write must leave the file byte-identical"
        );
    }

    #[test]
    fn enforce_staleness_reports_unparsable_not_stale() {
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n---\n";
        let err = super::enforce_staleness(Some(doc), Some("2026-09-25T01:00:00Z")).unwrap_err();
        assert!(
            matches!(
                &err,
                WriteNoteError::InvalidFrontmatter(detail) if detail == "existing_unparsable:no_token"
            ),
            "got: {err}"
        );
    }

    /// Task 5 — the three existing-unparsable reasons are distinguishable.
    #[test]
    fn existing_unparsable_reasons_are_distinguishable() {
        // parse: strict parse fails (colon title), tolerant finds ONE
        // updated_at line whose value is not RFC 3339.
        let parse_doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: not-a-timestamp\n---\n";
        // no_token: clean fence, parses, but no updated_at line at all.
        let no_token_doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: T\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n---\n";
        // no_fence: no frontmatter fence within the cap.
        let no_fence_doc = "just prose, no fences\n";

        for (doc, reason) in [
            (parse_doc, "existing_unparsable:parse"),
            (no_token_doc, "existing_unparsable:no_token"),
            (no_fence_doc, "existing_unparsable:no_fence"),
        ] {
            let err = super::enforce_staleness(Some(doc), Some("2026-09-25T01:00:00Z"))
                .expect_err("must be refused");
            assert!(
                matches!(
                    &err,
                    WriteNoteError::InvalidFrontmatter(detail) if detail == reason
                ),
                "expected {reason}, got: {err}"
            );
        }
    }

    /// Task 5 — stale edit against a fence the strict parser rejects still
    /// returns StaleUpdate carrying the TOLERANT-read current token
    /// (quote-stripped, RFC 3339).
    #[test]
    fn stale_carries_tolerant_read_current_token() {
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: \"2026-09-25T01:00:00Z\"\n---\n";
        let err = super::enforce_staleness(Some(doc), Some("1999-01-01T00:00:00Z")).unwrap_err();
        assert!(
            matches!(
                &err,
                WriteNoteError::StaleUpdate { updated_at } if updated_at == "2026-09-25T01:00:00Z"
            ),
            "got: {err}"
        );
    }

    fn vault() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("wiki")).unwrap();
        (dir, root)
    }

    fn fm(title: &str, updated_at: Option<&str>) -> OkfFrontmatter {
        OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: title.to_string(),
            entity_type: super::super::EntityType::Fact,
            tags: Some(vec!["test".to_string()]),
            created_at: "2026-08-27T00:00:00Z".to_string(),
            updated_at: updated_at.map(str::to_string),
            supersedes: None,
        }
    }

    /// D1 — create: writes frontmatter + body, fills updated_at, vault-relative path.
    #[test]
    fn d1_create_note_writes_frontmatter_and_hash() {
        let (_g, root) = vault();
        let result = write_note(
            &root,
            "wiki/test-note.md",
            &fm("T", None),
            "Body line.\nSecond.\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert_eq!(result.path, "wiki/test-note.md");
        assert!(result.success);
        // Task 5: the result carries the fresh token written into the file.
        chrono::DateTime::parse_from_rfc3339(&result.updated_at).unwrap();
        let content = fs::read_to_string(root.join("wiki/test-note.md")).unwrap();
        assert!(content.starts_with("---\nokf_version: 0.1\n"));
        assert!(content.contains("updated_at: 20"));
        assert!(content.ends_with("Second.\n"));
        assert_eq!(result.sha256, sha256_hash(&content));
        assert_eq!(
            result.updated_at,
            read_existing_token(&content).unwrap(),
            "result token must equal the token stored in the file"
        );
    }

    /// Task 5 — the success result's `updated_at` is the NEW post-write
    /// token: parseable RFC 3339 and non-empty (rotation checked in d2).
    #[test]
    fn write_note_result_carries_fresh_token() {
        let (_g, root) = vault();
        let result = write_note(
            &root,
            "wiki/tok.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(result.success);
        assert!(!result.updated_at.is_empty());
        chrono::DateTime::parse_from_rfc3339(&result.updated_at).unwrap();
    }

    /// D2 — stale edit without token is refused; token must exact-match.
    #[test]
    fn d2_edit_requires_exact_token() {
        let (_g, root) = vault();
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "v1\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let current =
            read_existing_token(&fs::read_to_string(root.join("wiki/n.md")).unwrap()).unwrap();

        // No token → refused (cannot prove freshness).
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "v2\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(err, WriteNoteError::StaleUpdate { ref updated_at } if updated_at == &current)
        );

        // Wrong token → refused.
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "v2\n",
            Some("1999-01-01T00:00:00Z"),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(err, WriteNoteError::StaleUpdate { ref updated_at } if updated_at == &current)
        );

        // Correct token → succeeds, token rotates.
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "v2\n",
            Some(&current),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let bumped =
            read_existing_token(&fs::read_to_string(root.join("wiki/n.md")).unwrap()).unwrap();
        assert_ne!(current, bumped);
    }

    /// D3 — path traversal is refused by safe_vault_path (no escapes ever).
    #[test]
    fn d3_traversal_rejected() {
        let (_g, root) = vault();
        let err = write_note(
            &root,
            "../outside.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
        let err = write_note(
            &root,
            "/etc/passwd",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
    }

    /// D4 — nested parents are created safely, then re-validated.
    #[test]
    fn d4_creates_missing_parents_safely() {
        let (_g, root) = vault();
        write_note(
            &root,
            "wiki/deep/er/note.md",
            &fm("Deep", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(root.join("wiki/deep/er/note.md").is_file());
    }

    /// D5 — index upsert: create + idempotent update, no duplicates.
    #[test]
    fn d5_upsert_no_duplicates() {
        let (_g, root) = vault();
        write_note(
            &root,
            "wiki/a.md",
            &fm("A", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        fs::write(
            root.join("wiki/INDEX.md"),
            "# Index\n\n## other\n[[b.md]]\n- Type: doc\n",
        )
        .unwrap();

        let r1 =
            upsert_index_entry(&root, "wiki/INDEX.md", "alpha", "wiki/a.md", "fact", None).unwrap();
        assert!(r1.appended);
        let c1 = fs::read_to_string(root.join("wiki/INDEX.md")).unwrap();
        assert_eq!(c1.matches("## alpha\n").count(), 1);

        let r2 = upsert_index_entry(
            &root,
            "wiki/INDEX.md",
            "alpha",
            "wiki/a.md",
            "fact",
            Some(&json!({"status":"live"})),
        )
        .unwrap();
        assert!(!r2.appended);
        let c2 = fs::read_to_string(root.join("wiki/INDEX.md")).unwrap();
        assert_eq!(c2.matches("## alpha\n").count(), 1);
        assert!(c2.contains("- Status: live"));
        assert!(c2.starts_with("# Index\n\n## other\n"));
        assert_eq!(
            r2.line_number,
            Some(7),
            "line numbers are 1-based against the file"
        );
    }

    /// D6 — prefix collisions never match: `## alph` != `## alpha`.
    #[test]
    fn d6_prefix_collision_isolated() {
        let (_g, root) = vault();
        fs::write(root.join("wiki/a.md"), "---\nokf_version: 0.1\n---\n").unwrap();
        fs::write(root.join("wiki/z.md"), "---\nokf_version: 0.1\n---\n").unwrap();
        fs::write(
            root.join("wiki/INDEX.md"),
            "## alpha\n[[a.md]]\n- Type: fact\n\n## alphabet\n[[z.md]]\n- Type: doc\n",
        )
        .unwrap();
        upsert_index_entry(&root, "wiki/INDEX.md", "alpha", "wiki/a.md", "fact", None).unwrap();
        let c = fs::read_to_string(root.join("wiki/INDEX.md")).unwrap();
        assert!(c.contains("## alphabet\n[[z.md]]\n- Type: doc\n"));
        assert_eq!(c.matches("## alpha\n").count(), 1);
        // Substring machines (contains/find) would have corrupted `alphabet`.
    }

    /// D7 — index must exist (no auto-create) and entry refs must be safe.
    #[test]
    fn d7_index_not_auto_created_and_names_validated() {
        let (_g, root) = vault();
        let err = upsert_index_entry(&root, "wiki/MISSING.md", "x", "wiki/a.md", "fact", None)
            .unwrap_err();
        assert!(matches!(err, UpsertError::IndexNotFound(ref p) if p == "wiki/MISSING.md"));

        fs::write(root.join("wiki/a.md"), "---\nokf_version: 0.1\n---\n").unwrap();
        fs::write(root.join("wiki/INDEX.md"), "").unwrap();
        let err =
            upsert_index_entry(&root, "wiki/INDEX.md", "", "wiki/a.md", "fact", None).unwrap_err();
        assert!(matches!(err, UpsertError::InvalidEntryName));
        let err = upsert_index_entry(
            &root,
            "wiki/INDEX.md",
            "bad name!",
            "wiki/a.md",
            "fact",
            None,
        );
        assert!(matches!(err, Err(UpsertError::InvalidEntryName))); // '!' is outside the pinned charset
        upsert_index_entry(
            &root,
            "wiki/INDEX.md",
            "good name",
            "wiki/a.md",
            "fact",
            None,
        )
        .unwrap(); // spaces ARE legal; headers pin exact lines
    }

    /// Block format pin: header/link/type/metadata lines, one per line.
    #[test]
    fn block_format_pinned() {
        let b =
            render_index_entry_block("n", "p.md", "fact", Some(&json!({"status":"live","n":2})))
                .unwrap();
        assert_eq!(b, "## n\n[[p.md]]\n- Type: fact\n- N: 2\n- Status: live\n");
    }

    /// Whole-line matcher ignores indented or commented look-alikes.
    #[test]
    fn matcher_requires_whole_line() {
        let content = "- ## fake\n\ntext ## fake\n## fake extra\n";
        assert_eq!(find_entry_header_line(content, "fake"), None);
        assert_eq!(find_entry_header_line("## fake\n", "fake"), Some(0));
    }

    // ---- Agent deposit write path (spec: 2026-08-27-agent-deposit-write-path.md) ----

    /// Vault fixture with the deposit dir present (post-Phase-2 state).
    fn deposit_vault() -> (TempDir, std::path::PathBuf) {
        let (dir, root) = vault();
        fs::create_dir_all(root.join("immutable-source-files/agents")).unwrap();
        (dir, root)
    }

    /// AD1 — flat deposit write succeeds; file lands under agents/.
    #[test]
    fn ad1_flat_deposit_write_succeeds() {
        let (_g, root) = deposit_vault();
        let result = write_note(
            &root,
            "immutable-source-files/agents/mem.md",
            &fm("Agent memory", None),
            "deposited\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(result.success);
        assert!(root.join("immutable-source-files/agents/mem.md").is_file());
    }

    /// AD2 — nested deposits succeed (amended spec §AMENDED 2026-08-29:
    /// subfolders under `agents/` are allowed, any depth).
    #[test]
    fn ad2_nested_deposit_write_succeeds() {
        let (_g, root) = deposit_vault();
        let result = write_note(
            &root,
            "immutable-source-files/agents/people/tessera/x.md",
            &fm("Nested", None),
            "deposited\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(result.success);
        assert!(root
            .join("immutable-source-files/agents/people/tessera/x.md")
            .is_file());
    }

    /// E2 — deep-path deposit (4 levels under `agents/`) succeeds; missing
    /// intermediate dirs are bootstrapped by the parent-create retry.
    #[test]
    fn e2_deep_nested_deposit_write_succeeds() {
        let (_g, root) = deposit_vault();
        let result = write_note(
            &root,
            "immutable-source-files/agents/products/curated-thoughts/specs/y.md",
            &fm("Deep", None),
            "deposited\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(result.success);
        assert!(root
            .join("immutable-source-files/agents/products/curated-thoughts/specs/y.md")
            .is_file());
    }

    /// AD3 — write outside the deposit prefix is rejected (path safety).
    #[test]
    fn ad3_user_source_write_rejected() {
        let (_g, root) = deposit_vault();
        let err = write_note(
            &root,
            "immutable-source-files/secrets.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
    }

    /// AD3b — a rejected write leaves no directories behind. Restores the
    /// no-side-effect assertion that was dropped with the old flat-layout AD2
    /// test; nothing else in this file covered it.
    #[test]
    fn ad3b_rejected_write_creates_no_dirs() {
        let (_g, root) = deposit_vault();
        let err = write_note(
            &root,
            "immutable-source-files/agents-evil/nested/mem.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
        assert!(!root.join("immutable-source-files/agents-evil").exists());
    }

    /// A leading `./` still resolves. `Path::components` keeps a leading
    /// `CurDir`, so the lexical `under_any` gate on the parent-bootstrap branch
    /// must drop it — `safe_vault_path` accepts `./` (it rejects only `..` and
    /// prefix components), and rejecting it here would be a regression.
    #[test]
    fn dot_prefixed_path_with_missing_parents_still_writes() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "./wiki/deep/er/dot.md",
            &fm("Dot", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(root.join("wiki/deep/er/dot.md").is_file());

        write_note(
            &root,
            "./immutable-source-files/agents/nested/dot.md",
            &fm("Dot", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(root
            .join("immutable-source-files/agents/nested/dot.md")
            .is_file());
    }

    /// AD3c — a symlinked component under `agents/` is never traversed when
    /// bootstrapping parents. `create_dir_all` would follow it and create dirs
    /// outside the vault root (the write itself is still rejected by round-two
    /// containment, but the directories would persist).
    #[cfg(unix)]
    #[test]
    fn ad3c_symlinked_parent_component_creates_nothing_outside() {
        let (_g, root) = deposit_vault();
        let outside = root.parent().unwrap().join("outside-target");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("immutable-source-files/agents/sub"))
            .unwrap();

        let err = write_note(
            &root,
            "immutable-source-files/agents/sub/deep/mem.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();

        assert!(matches!(err, WriteNoteError::WriteError(_)));
        assert!(
            !outside.join("deep").exists(),
            "create_dir_all followed the symlink and escaped the vault"
        );
    }

    // ---- F2 top-level write-root allow-list (spec 2026-09-27-vault-ingest-policy) ----

    /// R1 — `records/…` writes succeed at any depth (new writable root,
    /// never-ingested counterpart of the walker exclusion).
    #[test]
    fn records_root_write_succeeds_at_depth() {
        let (_g, root) = vault();
        let result = write_note(
            &root,
            "records/sessions/people/tessera/x.md",
            &fm("Session", None),
            "deposited\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(result.success);
        assert!(root.join("records/sessions/people/tessera/x.md").is_file());
    }

    /// R2 — a first segment that is not an allowed root is rejected with the
    /// `DisallowedRoot` variant and the message NAMES the allowed roots
    /// (machine-parseable, per the PathOutsideVault MCP error conventions).
    #[test]
    fn retired_flat_agents_layout_is_rejected_naming_roots() {
        let (_g, root) = deposit_vault();
        let err = write_note(
            &root,
            "agents/tessera/mem.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        match &err {
            WriteNoteError::DisallowedRoot {
                first_segment,
                allowed,
            } => {
                assert_eq!(first_segment, "agents");
                for root_name in ["immutable-source-files", "records", "wiki"] {
                    assert!(
                        allowed.contains(root_name),
                        "error must name allowed root {root_name}; got {allowed:?}"
                    );
                }
            }
            other => panic!("expected DisallowedRoot, got {other:?}"),
        }
        // No filesystem trace: the disallowed root must not be created.
        assert!(!root.join("agents").exists());
    }

    /// R2b — the check fires BEFORE any filesystem access, so a nested
    /// not-yet-existing disallowed tree (`people/deep/a.md`) leaves no
    /// directories behind either.
    #[test]
    fn disallowed_root_rejection_creates_no_dirs() {
        let (_g, root) = vault();
        let err = write_note(
            &root,
            "people/deep/notes.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(err, WriteNoteError::DisallowedRoot { .. }),
            "got {err:?}"
        );
        assert!(!root.join("people").exists());
    }

    /// R3 — sibling lookalikes of allowed roots are still rejected
    /// (exact segment match, spec D4 pattern).
    #[test]
    fn root_lookalikes_are_rejected() {
        let (_g, root) = vault();
        for path in ["records-evil/x.md", "wiki-adjacent/x.md", "my.records/x.md"] {
            let err = write_note(
                &root,
                path,
                &fm("T", None),
                "x\n",
                None,
                NO_SHRINK,
                NO_KEY_DROP,
            )
            .unwrap_err();
            assert!(
                matches!(err, WriteNoteError::DisallowedRoot { .. }),
                "{path}: expected DisallowedRoot, got {err:?}"
            );
        }
    }

    /// R4 — `immutable-source-files` outside the deposit prefix is still
    /// rejected (F2 preserves the AD3 constraint; now it fails as Outside
    /// before reaching the deposit allowlist only when the first segment is
    /// not a root — here the first segment IS a root, so the existing
    /// PathOutsideVault shape must hold).
    #[test]
    fn immutable_root_outside_deposit_prefix_still_rejected() {
        let (_g, root) = deposit_vault();
        let err = write_note(
            &root,
            "immutable-source-files/secrets.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
    }

    /// R5 — a leading `./` does not change which root a path lands in
    /// (`./wiki/…` stays allowed; `./agents/…` is still rejected).
    #[test]
    fn dot_prefix_does_not_bypass_root_allowlist() {
        let (_g, root) = vault();
        write_note(
            &root,
            "./wiki/ok.md",
            &fm("D", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let err = write_note(
            &root,
            "./agents/tessera/m.md",
            &fm("D", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(err, WriteNoteError::DisallowedRoot { .. }),
            "got {err:?}"
        );
    }

    /// AD4 — first deposit into a missing agents/ bootstraps parents (lazy path).
    #[test]
    fn ad4_lazy_bootstrap_missing_agents_dir() {
        let (_g, root) = vault(); // no immutable-source-files/agents
        write_note(
            &root,
            "immutable-source-files/agents/first.md",
            &fm("First", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        assert!(root
            .join("immutable-source-files/agents/first.md")
            .is_file());
    }

    /// AD5 — supersedes happy path: target exists → write succeeds and the
    /// supersedes value round-trips through render → parse.
    #[test]
    fn ad5_supersedes_valid_roundtrip() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "immutable-source-files/agents/v1.md",
            &fm("V1", None),
            "old\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let mut m = fm("V2", None);
        m.supersedes = Some("immutable-source-files/agents/v1.md".to_string());
        write_note(
            &root,
            "immutable-source-files/agents/v2.md",
            &m,
            "new\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let raw = fs::read_to_string(root.join("immutable-source-files/agents/v2.md")).unwrap();
        assert!(raw.contains("supersedes: immutable-source-files/agents/v1.md"));
        let parsed = extract_fm(&raw);
        assert_eq!(
            parsed.supersedes.as_deref(),
            Some("immutable-source-files/agents/v1.md")
        );
    }

    /// AD5b — supersedes roundtrip with a NESTED target: both ends inside
    /// `agents/` at depth, containment check passes (amended spec
    /// §AMENDED 2026-08-29).
    #[test]
    fn ad5b_supersedes_nested_target_roundtrip() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "immutable-source-files/agents/people/tessera/v1.md",
            &fm("V1", None),
            "old\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let mut m = fm("V2", None);
        m.supersedes = Some("immutable-source-files/agents/people/tessera/v1.md".to_string());
        write_note(
            &root,
            "immutable-source-files/agents/people/tessera/v2.md",
            &m,
            "new\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let raw =
            fs::read_to_string(root.join("immutable-source-files/agents/people/tessera/v2.md"))
                .unwrap();
        assert!(raw.contains("supersedes: immutable-source-files/agents/people/tessera/v1.md"));
        let parsed = extract_fm(&raw);
        assert_eq!(
            parsed.supersedes.as_deref(),
            Some("immutable-source-files/agents/people/tessera/v1.md")
        );
    }

    /// AD6 — supersedes pointing outside agents/ → InvalidFrontmatter.
    #[test]
    fn ad6_supersedes_outside_deposit_rejected() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "wiki/target.md",
            &fm("T", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let mut m = fm("Evil", None);
        m.supersedes = Some("wiki/target.md".to_string());
        let err = write_note(
            &root,
            "immutable-source-files/agents/e.md",
            &m,
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::InvalidFrontmatter(_)));
    }

    /// AD7 — supersedes to a non-existent deposit → InvalidFrontmatter.
    #[test]
    fn ad7_supersedes_missing_target_rejected() {
        let (_g, root) = deposit_vault();
        let mut m = fm("T", None);
        m.supersedes = Some("immutable-source-files/agents/ghost.md".to_string());
        let err = write_note(
            &root,
            "immutable-source-files/agents/n.md",
            &m,
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        match err {
            WriteNoteError::InvalidFrontmatter(ref detail) => {
                assert!(detail.contains("supersedes_not_found"));
            }
            other => panic!("expected InvalidFrontmatter, got {other:?}"),
        }
    }

    /// AD8 — sibling-prefix attack: `agents-evil/` must NOT pass as a deposit
    /// (string starts_with would accept it; component check must not).
    #[test]
    fn ad8_sibling_prefix_rejected() {
        let (_g, root) = deposit_vault();
        fs::create_dir_all(root.join("immutable-source-files/agents-evil")).unwrap();
        fs::write(root.join("immutable-source-files/agents-evil/x.md"), "x\n").unwrap();
        let mut m = fm("Evil", None);
        m.supersedes = Some("immutable-source-files/agents-evil/x.md".to_string());
        let err = write_note(
            &root,
            "immutable-source-files/agents/e.md",
            &m,
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::InvalidFrontmatter(_)));
    }

    /// AD9 — supersedes is deposit-only: wiki notes cannot carry it.
    #[test]
    fn ad9_wiki_note_cannot_supersede() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "immutable-source-files/agents/v1.md",
            &fm("V1", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let mut m = fm("W", None);
        m.supersedes = Some("immutable-source-files/agents/v1.md".to_string());
        let err =
            write_note(&root, "wiki/w.md", &m, "x\n", None, NO_SHRINK, NO_KEY_DROP).unwrap_err();
        assert!(matches!(err, WriteNoteError::InvalidFrontmatter(_)));
    }

    /// T3.1 — round-trip guard: a valid note passes the guard unchanged.
    #[test]
    fn t3_roundtrip_guard_valid_note_passes() {
        let (_g, root) = vault();
        let result = write_note(
            &root,
            "wiki/t3-a.md",
            &fm("T3 Note", None),
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        );
        assert!(
            result.is_ok(),
            "valid note must pass round-trip guard: {:?}",
            result.err()
        );
    }

    /// T3.2 — round-trip guard: `tags: Some(vec![])` passes. render drops the
    /// empty list; serde default reads the omission back as None — normalize
    /// both sides before comparing.
    #[test]
    fn t3_roundtrip_guard_empty_tags_normalizes() {
        let (_g, root) = vault();
        let mut m = fm("T3 Empty Tags", None);
        m.tags = Some(vec![]);
        let result = write_note(
            &root,
            "wiki/t3-b.md",
            &m,
            "x\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        );
        assert!(
            result.is_ok(),
            "Some(vec![]) tags must normalize to None and pass: {:?}",
            result.err()
        );
    }

    /// T3.3 — round-trip guard: unknown keys that would be injected into the
    /// rendered fence (the typed struct has no deny_unknown_fields, so parse
    /// alone would silently drop them) must be rejected by the key-set check.
    #[test]
    fn t3_roundtrip_guard_rejects_unknown_keys() {
        // Force the guard by monkey-patching the renderer is impossible from a
        // unit test, so exercise the guard helper directly (it is the unit the
        // plan specifies); write_note() composes it pre-write.
        let m = fm("T3 Injected", None);
        let mut rendered = render_frontmatter(&m);
        rendered.insert_str(rendered.len() - 4, "injected_key: pwned\n");
        let err = check_round_trip(&m, &rendered).unwrap_err();
        // MINOR-4 (issue #231 review): both mismatch directions now share one
        // neutral message — distinguish the unknown-key case by the GOT set
        // carrying the injected key the EXPECTED set lacks.
        assert!(
            matches!(err, WriteNoteError::InvalidFrontmatter(ref msg)
                if msg.contains("round_trip: key set mismatch")
                    && msg.contains("injected_key")),
            "unknown-key injection must be rejected with expected/got sets, got: {err:?}"
        );
    }

    /// MINOR-5 (issue #231 review) — the key-set check is an EXACT set
    /// comparison, both directions, and rejects ANY rendered key that is not
    /// a string (e.g. `1: x` renders a non-string key that the old
    /// `filter_map(as_str)` silently dropped).
    #[test]
    fn roundtrip_guard_rejects_non_string_key_injection() {
        let m = fm("T3 NonStringKey", None);
        let mut rendered = render_frontmatter(&m);
        // Insert `1: x` before the closing fence (last 4 chars = "---\n").
        rendered.insert_str(rendered.len() - 4, "1: x\n");
        let err = check_round_trip(&m, &rendered).unwrap_err();
        assert!(
            matches!(err, WriteNoteError::InvalidFrontmatter(ref msg) if msg.contains("round_trip")),
            "non-string key injection must be rejected, got: {err}"
        );
    }

    /// MINOR-5 — exact key-set comparison: a MISSING known key must also be
    /// rejected (set equality, not subset).
    #[test]
    fn roundtrip_guard_rejects_missing_required_key() {
        let m = fm("T3 MissingKey", None);
        let mut rendered = render_frontmatter(&m);
        // Drop the `profile:` line; the fence stays intact.
        rendered = rendered.replace("profile: llm-wiki/1\n", "");
        let err = check_round_trip(&m, &rendered).unwrap_err();
        // MINOR-4: distinguish the missing-key direction by the EXPECTED set
        // still containing `profile` while the GOT set does not.
        assert!(
            matches!(err, WriteNoteError::InvalidFrontmatter(ref msg)
                if msg.contains("round_trip: key set mismatch")
                    && msg.contains(r#"expected: ["created_at", "entity_type", "okf_version", "profile", "tags", "title"]"#)
                    && msg.contains(r#"got: ["created_at", "entity_type", "okf_version", "tags", "title"]"#)),
            "missing key must be rejected with expected/got sets, got: {err:?}"
        );
    }

    /// MINOR-1 (issue #231 review) — the guard uses the SAME fence view as
    /// the token reader, and hardening: a document whose fence is intact but
    /// whose token the reader cannot recover must be refused pre-write
    /// ("written note would be uneditable"), not written to disk bricked.
    #[test]
    fn roundtrip_guard_rejects_note_with_unrecoverable_token() {
        let m = fm("T3 UnrecoverableToken", None);
        // Hand-built document (the guard takes the document as a param, so
        // this is directly testable): fence intact, but `updated_at` is a
        // sequence node — the strict parse of `updated_at` as a string fails
        // and the tolerant fallback sees no column-0 `updated_at:` scalar
        // line, so the token reader returns NoToken. The renderer cannot
        // produce this; the point is that even a synthetic document whose
        // token the reader cannot recover is refused pre-write, never
        // written to disk uneditable.
        let mut doc = render_frontmatter(&m);
        doc.insert_str(doc.len() - 4, "updated_at:\n  - 2026-09-25T01:00:00Z\n");
        // Prove the precondition: the token reader really cannot read this
        // document back.
        assert!(read_existing_token(&doc).is_err());
        let err = check_round_trip(&m, &doc).unwrap_err();
        assert!(
            matches!(err, WriteNoteError::InvalidFrontmatter(ref msg) if msg.contains("round_trip")),
            "unreadable-back document must be rejected pre-write, got: {err}"
        );
    }

    /// MINOR-3 (issue #231 review) — the `take(64)` fence cap is intentional
    /// and applies to BOTH paths: a fence with MORE than 64 frontmatter
    /// lines never closes within the cap, so the guard sees "no fence" and
    /// refuses with `existing_unparsable:no_fence` instead of parsing a
    /// partial view.
    #[test]
    fn roundtrip_guard_refuses_fence_over_64_lines_as_no_fence() {
        let m = fm("T3 FenceOver64", None);
        let mut doc = String::from("---\n");
        for i in 0..70 {
            doc.push_str(&format!("extra_key_{i}: value_{i}\n"));
        }
        doc.push_str("---\nbody\n");
        let err = check_round_trip(&m, &doc).unwrap_err();
        assert!(
            matches!(err, WriteNoteError::InvalidFrontmatter(ref detail) if detail == "existing_unparsable:no_fence"),
            ">64-line fence must be refused as no_fence, got: {err}"
        );
    }

    // ------------------------------------------------------------------
    // Task 7 — adversarial acceptance suite (issue #231, plan §Task 7).
    // Second-edit coverage through the public `write_note` only.
    // ------------------------------------------------------------------

    /// Every adversarial title from the plan's Task 7 list.
    const ADVERSARIAL_TITLES: &[&str] = &[
        "Deploy: retro",
        "2026-09-25T14:00:00Z: deploy retro", // timestamp prefix + colon
        "Trailing colon:",
        "[WIP] retry logic",
        "*foo anchor alias",
        "&x",            // must NOT round-trip to ""
        "#foo",          // must NOT round-trip to ""
        "!foo",          // must NOT round-trip to ""
        "'Hello' world", // partly single-quoted
        "\"Hello\" world",
        "Plan\u{2028}B", // control char (LS) is the ONLY trigger
        "Plan\u{FEFF}B", // BOM parses fine — no escape needed
        "2024",
        "yes", // reserved literals gain quotes (accepted)
    ];

    /// Create a note with `title`, scrape the If-Match token FROM DISK (the
    /// way the issue-#231 reporter did), and edit again with that token.
    fn write_and_edit(title: &str) -> Result<(WriteNoteResult, WriteNoteResult), WriteNoteError> {
        let (_guard, root) = vault();
        let create = write_note(
            &root,
            "wiki/note.md",
            &fm(title, None),
            "v1\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )?;
        let on_disk = fs::read_to_string(root.join("wiki/note.md")).unwrap();
        let token = read_existing_token(&on_disk).expect("token readable after create");
        let edit = write_note(
            &root,
            "wiki/note.md",
            &fm(title, None),
            "v2\n",
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        )?;
        Ok((create, edit))
    }

    /// Task 7 acceptance: EVERY adversarial title survives create → scrape
    /// token from disk → second edit.
    #[test]
    fn second_edit_succeeds_for_every_adversarial_title() {
        for title in ADVERSARIAL_TITLES {
            let (create, edit) = write_and_edit(title).unwrap_or_else(|e| panic!("{title:?}: {e}"));
            assert!(create.success, "{title:?}: create");
            assert!(edit.success, "{title:?}: second edit failed: {edit:?}");
        }
    }

    /// Indicator/reserved titles must round-trip EXACTLY — never collapse to
    /// the empty string (serde_yaml reads bare `&x`/`#foo`/`!foo` as `""`).
    #[test]
    fn adversarial_titles_round_trip_exactly_not_to_empty() {
        for title in [
            "&x",
            "#foo",
            "!foo",
            "*foo anchor alias",
            "Plan\u{2028}B",
            "Plan\u{FEFF}B",
            "2024",
            "yes",
            "2026-09-25T14:00:00Z: deploy retro",
        ] {
            let (_g, root) = vault();
            write_note(
                &root,
                "wiki/n.md",
                &fm(title, None),
                "x\n",
                None,
                NO_SHRINK,
                NO_KEY_DROP,
            )
            .unwrap_or_else(|e| panic!("{title:?}: create failed: {e}"));
            let on_disk = fs::read_to_string(root.join("wiki/n.md")).unwrap();
            let parsed = extract_fm(&on_disk);
            assert_eq!(
                parsed.title, title,
                "title must round-trip exactly (not to empty)"
            );
        }
    }

    /// Legacy broken fixture (pre-fix bytes, unquoted colon title) written
    /// DIRECTLY to disk becomes editable via the tolerant token read — and,
    /// per the Task 6 lesson, heals (it is NOT a repair-scan hit).
    #[test]
    fn legacy_unquoted_colon_note_becomes_editable() {
        let (_g, root) = vault();
        let legacy = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Deploy: retro\nentity_type: fact\ncreated_at: 2026-08-27T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\n---\nbody v1\n";
        fs::write(root.join("wiki/legacy.md"), legacy).unwrap();
        // Scrape the token the way the reporter did: from the raw bytes.
        let token = read_existing_token(legacy).expect("tolerant read recovers the token");
        assert_eq!(token, "2026-09-25T01:00:00Z");
        let result = write_note(
            &root,
            "wiki/legacy.md",
            &fm("Deploy: retro", None),
            "body v2\n",
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .expect("legacy broken fixture must heal on its next edit");
        assert!(result.success);
        // Healed: the title is now quoted on disk and the strict parse works.
        let on_disk = fs::read_to_string(root.join("wiki/legacy.md")).unwrap();
        assert!(
            on_disk.contains("title: \"Deploy: retro\"\n"),
            "healed file must quote the title, got: {on_disk}"
        );
        let parsed = extract_fm(&on_disk);
        assert_eq!(parsed.title, "Deploy: retro");
        assert_ne!(
            parsed.updated_at.as_deref(),
            Some("2026-09-25T01:00:00Z"),
            "token must rotate on the healing edit"
        );
        // Healed note is editable again and NOT reported by the repair scan.
        let hits = crate::okf::repair_scan::scan_unparsable_notes(&root);
        assert!(
            hits.is_empty(),
            "healed note must not be a scan hit: {hits:?}"
        );
    }

    /// Injection titles: a newline inside the title must NEVER smuggle extra
    /// frontmatter keys into the rendered fence. The quoting layer neutralizes
    /// the newline (escaped `\n` inside one double-quoted scalar), so the
    /// acceptance property is: whatever the write outcome, the fence contains
    /// EXACTLY the expected key set, the title round-trips, and no injected
    /// value lands in a typed field.
    #[test]
    fn injection_titles_cannot_bypass_validation() {
        for title in [
            "x\nsupersedes: immutable-source-files/agents/anything.md",
            "x\ntags: [a]",
            "x\nstatus: approved",
        ] {
            let (_g, root) = vault();
            let outcome = write_note(
                &root,
                "wiki/inj.md",
                &fm(title, None),
                "x\n",
                None,
                NO_SHRINK,
                NO_KEY_DROP,
            );
            let on_disk = match outcome {
                Ok(result) => {
                    assert!(result.success, "{title:?}");
                    fs::read_to_string(root.join("wiki/inj.md")).unwrap()
                }
                // Refusal is also a safe outcome — but only via the pinned
                // round_trip guard, never a silent wrong write.
                Err(WriteNoteError::InvalidFrontmatter(detail)) => {
                    assert!(
                        detail.contains("round_trip"),
                        "{title:?}: unexpected refusal detail: {detail}"
                    );
                    continue;
                }
                Err(e) => panic!("{title:?}: unexpected error: {e}"),
            };
            let fenced: String = on_disk
                .lines()
                .skip(1) // opening ---
                .take_while(|l| l != &"---")
                .fold(String::new(), |mut acc, l| {
                    acc.push_str(l);
                    acc.push('\n');
                    acc
                });
            let parsed = parse_frontmatter(&fenced)
                .unwrap_or_else(|e| panic!("{title:?}: fence must parse: {e}"));
            // No injected key landed in a typed field:
            assert_eq!(parsed.title, title, "{title:?}: title must round-trip");
            assert!(
                parsed.supersedes.is_none(),
                "{title:?}: supersedes injected"
            );
            assert_eq!(
                parsed.tags,
                Some(vec!["test".to_string()]),
                "{title:?}: tags must be the intended ones only"
            );
            // No injected key landed in the fence at all (status:, etc.):
            let mut keys: Vec<&str> = fenced
                .lines()
                .filter_map(|l| l.split(':').next())
                .filter(|k| !k.is_empty())
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec![
                    "created_at",
                    "entity_type",
                    "okf_version",
                    "profile",
                    "tags",
                    "title",
                    "updated_at"
                ],
                "{title:?}: fence key set must be exactly the intended one, fence: {fenced:?}"
            );
        }
    }

    /// Differential: wherever the STRICT parser reads a token from a rendered
    /// adversarial doc, the tolerant fallback returns the SAME token; wherever
    /// strict fails, tolerant must still heal (these renders are all fixable).
    #[test]
    fn differential_tolerant_matches_strict_on_clean_notes() {
        for title in ADVERSARIAL_TITLES {
            let m = fm(title, Some("2026-09-25T01:00:00Z"));
            let doc = render_document(&m, "body\n");
            let fenced: String = doc.lines().skip(1).take_while(|l| l != &"---").fold(
                String::new(),
                |mut acc, l| {
                    acc.push_str(l);
                    acc.push('\n');
                    acc
                },
            );
            let strict = parse_frontmatter(&fenced).ok().and_then(|p| p.updated_at);
            let tolerant = read_existing_token(&doc);
            match strict {
                Some(token) => assert_eq!(
                    tolerant.ok().as_deref(),
                    Some(token.as_str()),
                    "{title:?}: tolerant must match strict wherever strict succeeds"
                ),
                None => assert!(
                    tolerant.is_ok(),
                    "{title:?}: strict failed but the tolerant fallback must heal"
                ),
            }
        }
    }

    /// Adversarial tag values (flow context: `,` `]` `"` are mid-value
    /// indicators) survive create → second edit → exact round-trip, and the
    /// empty list normalizes to absent.
    #[test]
    fn adversarial_tags_second_edit() {
        let cases: &[&[&str]] = &[
            &["say \"hi\"", "C:\\p", "a\", \"b"],
            &["a,b", "x]"],
            &[], // Some(vec![]) renders as absent
        ];
        for tags in cases {
            let (_g, root) = vault();
            let mut m = fm("Tagged note", None);
            m.tags = Some(tags.iter().map(|s| s.to_string()).collect());
            write_note(
                &root,
                "wiki/tags.md",
                &m,
                "v1\n",
                None,
                NO_SHRINK,
                NO_KEY_DROP,
            )
            .unwrap_or_else(|e| panic!("create {tags:?}: {e}"));
            let on_disk = fs::read_to_string(root.join("wiki/tags.md")).unwrap();
            let token = read_existing_token(&on_disk).expect("token readable after create");
            let edit = write_note(
                &root,
                "wiki/tags.md",
                &m,
                "v2\n",
                Some(&token),
                NO_SHRINK,
                NO_KEY_DROP,
            )
            .unwrap_or_else(|e| panic!("edit {tags:?}: {e}"));
            assert!(edit.success, "{tags:?}");
            let final_disk = fs::read_to_string(root.join("wiki/tags.md")).unwrap();
            let parsed = extract_fm(&final_disk);
            let expected = if tags.is_empty() {
                None
            } else {
                Some(tags.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            };
            assert_eq!(
                parsed.tags, expected,
                "tags must round-trip exactly through the second edit"
            );
        }
    }

    /// Consistency: every repair-scan hit MUST correspond to a write-path
    /// refusal carrying the SAME `existing_unparsable:<reason>` detail — and
    /// clean notes are neither reported nor refused.
    #[test]
    fn scan_hit_reason_matches_write_path_error() {
        let (_g, root) = vault();
        let cases: &[(&str, &str, &str)] = &[
            (
                "wiki/no_token.md",
                "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: T\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n---\nbody\n",
                "existing_unparsable:no_token",
            ),
            (
                "wiki/no_fence.md",
                "just prose, no fences\n",
                "existing_unparsable:no_fence",
            ),
            (
                "wiki/dup_token.md",
                "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\nupdated_at: 2026-09-25T02:00:00Z\n---\nbody\n",
                "existing_unparsable:parse",
            ),
            (
                "wiki/bad_token.md",
                "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: not-a-date\n---\nbody\n",
                "existing_unparsable:parse",
            ),
            (
                "wiki/clean.md",
                "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Fine\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\n---\nbody\n",
                "",
            ),
        ];
        for (name, content, _) in cases {
            fs::write(root.join(name), content).unwrap();
        }
        // MAJOR-2 — a non-UTF-8 file: the scan reports existing_unparsable:parse
        // (its read_to_string arm) and the write path must refuse with the
        // SAME reason instead of silently clobbering the bytes.
        let binary_name = "wiki/binary.md";
        fs::write(root.join(binary_name), b"\xff\xfe not utf8 \x00").unwrap();
        let hits = crate::okf::repair_scan::scan_unparsable_notes(&root);
        assert_eq!(hits.len(), 5, "exactly the 5 broken notes: {hits:?}");
        for hit in &hits {
            let (name, _, reason) = if hit.path.ends_with("binary.md") {
                (binary_name, "", "existing_unparsable:parse")
            } else {
                *cases
                    .iter()
                    .find(|(n, _, _)| hit.path.ends_with(n))
                    .unwrap_or_else(|| panic!("scan hit {} matches no case", hit.path))
            };
            assert_eq!(hit.reason, *reason, "scan vs contract for {name}");
            // The write path refuses an edit of the same note with the SAME
            // detail (staleness check hits the unreadable token first).
            let err = write_note(
                &root,
                name,
                &fm("Edited", None),
                "x\n",
                Some("1999-01-01T00:00:00Z"),
                NO_SHRINK,
                NO_KEY_DROP,
            )
            .expect_err("edit of unparsable note must be refused");
            assert!(
                matches!(&err, WriteNoteError::InvalidFrontmatter(detail) if *detail == *reason),
                "write path for {name}: expected {reason}, got {err}"
            );
        }
        // The clean note edits fine.
        write_note(
            &root,
            "wiki/clean.md",
            &fm("Fine", None),
            "edited\n",
            Some("2026-09-25T01:00:00Z"),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .expect("clean note stays editable");
    }

    /// Parse a rendered document's frontmatter block.
    fn extract_fm(raw: &str) -> OkfFrontmatter {
        let fenced: String = raw
            .lines()
            .skip(1) // opening ---
            .take_while(|l| l != &"---")
            .fold(String::new(), |mut acc, l| {
                acc.push_str(l);
                acc.push('\n');
                acc
            });
        parse_frontmatter(&fenced).unwrap()
    }

    fn long_body(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("line {i} of a substantial note body\n"))
            .collect()
    }

    #[test]
    fn edit_rejects_truncated_payload_replay_of_incident() {
        // Spec L114-116 exact byte counts (review M3): 12,860 → 505 RENDERED.
        // Bodies are built WITH their trailing newline (render_document adds
        // one only if missing — review R1), so 12859+1 = 12860 on disk,
        // 504+1 = 505 rendered.
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing_body = format!("{}\n", "x".repeat(12859)); // 12,860 bytes rendered
        let new_body = format!("{}\n", "x".repeat(504)); // 505 bytes rendered
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &existing_body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            &new_body,
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("shrink_refused:12860:505"), "{s}");
        assert!(s.contains("re-read the note"), "{s}");
        assert!(!s.contains("allow_shrink"), "{s}");
    }

    #[test]
    fn edit_boundary_new_double_is_allowed() {
        // B1 fix: bodies carry their own trailing newline. Rendered sizes:
        // existing 1023+1 = 1024, new 511+1 = 512 → 512*2 == 1024 → ALLOWED
        // (== must pass). (Old draft used 1024/512 raw; render made them
        // 1025/513, so the equality case never actually tested equality.)
        let existing_body = format!("{}\n", "x".repeat(1023)); // 1024 rendered
        let new_body = format!("{}\n", "x".repeat(511)); // 512 rendered
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &existing_body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            &new_body,
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
    }

    #[test]
    fn edit_boundary_odd_existing_refused() {
        // B1 fix: rendered sizes existing 1024+1 = 1025, new 511+1 = 512 →
        // 512*2 = 1024 < 1025 → refused with the exact prefix. (Old draft's
        // 1025/512 raw bodies rendered 1026/513 → 513*2 = 1026, NOT < 1026,
        // so unwrap_err() panicked — the exact spec L35-37 trap.)
        let existing_body = format!("{}\n", "x".repeat(1024)); // 1025 rendered
        let new_body = format!("{}\n", "x".repeat(511)); // 512 rendered
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &existing_body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            &new_body,
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(err.to_string().starts_with("shrink_refused:1025:512"));
    }

    #[test]
    fn edit_boundary_plus_one_newline_alone_can_refuse() {
        // Spec L128-130 (review B1): a new body WITHOUT a trailing \n that
        // is refused only because render adds the +1. Existing renders 1025
        // (1024+\n). Raw-new 511 renders 512 → 512*2 = 1024 < 1025 →
        // refused — but raw math on 511 gives the same verdict. To pin the
        // +1 ITSELF, use raw-new 512: raw math says 512*2 = 1024 < 1025
        // (refuse), rendered math says 513*2 = 1026 ≥ 1025 (allow). Rendered
        // wins → the write SUCCEEDS. This test fails if anyone switches the
        // measurement basis to raw bodies.
        let existing_body = format!("{}\n", "x".repeat(1024)); // 1025 rendered
        let new_body = "x".repeat(512); // NO newline → renders 513
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &existing_body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            &new_body,
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
    }

    #[test]
    fn small_notes_may_be_fully_rewritten_without_flag() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &"x".repeat(200),
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            "tiny\n",
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
    }

    #[test]
    fn allow_shrink_permits_major_shrink() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &long_body(400),
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let allow_shrink = true;
        let allow_key_drop = false;
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            "deliberate full rewrite\n",
            Some(&created.updated_at),
            allow_shrink,
            allow_key_drop,
        )
        .unwrap();
    }

    #[test]
    fn create_with_marker_is_rejected() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let body = "text [SKILL_PRUNED] more text\n";
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("compaction_marker:[SKILL_PRUNED]"),
            "{err}"
        );
    }

    #[test]
    fn edit_rejects_newly_introduced_marker() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "clean body\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let allow_shrink = true;
        let allow_key_drop = false;
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            "clean body\nHERMES-CONTEXT-COMPRESSION\n",
            Some(&created.updated_at),
            allow_shrink,
            allow_key_drop,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("compaction_marker:HERMES-CONTEXT-COMPRESSION"),
            "{err}"
        );
    }

    #[test]
    fn edit_permits_marker_already_in_existing_frontmatter() {
        // B2 fix (review): write_note CREATE refuses every marker — including
        // in the title — so the seed note must go straight to disk via
        // fs::write with a hand-built valid fence. OkfFrontmatter has NO
        // `description` field (fields: okf_version, profile, title,
        // entity_type, tags, created_at, updated_at, supersedes) — the old
        // draft's `note.description = …` was a compile error; the second
        // marker lives in `tags`.
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing = [
            "---",
            "okf_version: 1",
            "title: \"note about [SKILL_PRUNED]\"",
            "tags: [\"quotes HERMES-CONTEXT-COMPRESSION\"]",
            "created_at: \"2026-09-01T00:00:00Z\"",
            "updated_at: \"2026-09-01T00:00:00Z\"",
            "---",
            "body one",
            "",
        ]
        .join("\n");
        std::fs::create_dir_all(root.join("wiki")).unwrap();
        std::fs::write(root.join("wiki/n.md"), &existing).unwrap();
        let token = read_existing_token(&existing).unwrap();
        // Every legitimate edit re-sends that frontmatter — must NOT be
        // locked (spec D2). Use fm("note about [SKILL_PRUNED]", …) so the new
        // document carries the same markers the existing one has.
        let note = fm("note about [SKILL_PRUNED]", Some(&token));
        write_note(
            &root,
            "wiki/n.md",
            &note,
            "body two\n",
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
    }

    #[test]
    fn edit_rejects_marker_in_existing_body_when_not_resent() {
        // Review B2 addition (spec L119): marker already in the existing
        // BODY, and the edit drops it — allowed, because the guard only
        // refuses NEWLY INTRODUCED markers (document contains ∧ ¬existing
        // contains). Seeded via fs::write for the same create-refusal reason.
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing = [
            "---",
            "okf_version: 1",
            "title: \"t\"",
            "created_at: \"2026-09-01T00:00:00Z\"",
            "updated_at: \"2026-09-01T00:00:00Z\"",
            "---",
            "text [SKILL_PRUNED] from an old compaction",
            "",
        ]
        .join("\n");
        std::fs::create_dir_all(root.join("wiki")).unwrap();
        std::fs::write(root.join("wiki/n.md"), &existing).unwrap();
        let token = read_existing_token(&existing).unwrap();
        write_note(
            &root,
            "wiki/n.md",
            &fm("t", Some(&token)),
            "clean replacement\n",
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
    }

    #[test]
    fn marker_check_runs_before_shrink_check() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &long_body(400),
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            "[SKILL_PRUNED]\n",
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            err.to_string().starts_with("compaction_marker:"),
            "marker must win: {err}"
        );
    }

    #[test]
    fn body_bytes_counts_fence_less_content_whole() {
        // Review m6 (spec L131-135): the fence-less path is UNREACHABLE
        // through write_note — enforce_staleness refuses no_fence first — so
        // this pins the helper directly instead of an end-to-end refusal.
        assert_eq!(body_bytes("no fence\n"), 9);
        assert_eq!(body_bytes(""), 0);
    }

    #[test]
    fn params_allow_shrink_omitted_defaults_false_and_true_parses() {
        // Review m7 (spec L121-122): plumbing tests. Key omitted → false.
        // DEVIATION (plan-vs-code): the plan's fixture used "frontmatter": {}
        // but OkfFrontmatter has no Default and five required fields, so that
        // deserialization fails; the fixture fills the required fields. The
        // asserted behavior (allow_shrink default/parse) is unchanged.
        let fm_json = serde_json::json!({
            "okf_version": "0.1",
            "profile": "llm-wiki/1",
            "title": "T",
            "entity_type": "fact",
            "created_at": "2026-09-01T00:00:00Z"
        });
        let v: serde_json::Value =
            serde_json::json!({ "path": "wiki/n.md", "frontmatter": fm_json, "body": "b" });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(!p.allow_shrink);
        let v: serde_json::Value = serde_json::json!({ "path": "wiki/n.md", "frontmatter": fm_json, "body": "b", "allow_shrink": true });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(p.allow_shrink);
    }

    #[test]
    fn shrink_refusal_reaches_mcp_surface_via_anyhow() {
        // Review m7: a shrink refusal must surface through dispatch's
        // anyhow!("{}") mapping — assert the message survives the mapping
        // and still carries the exact prefix (never "allow_shrink").
        // (The plan shipped this test as a stub; body implemented per the
        // plan's own Arrange/Act/Assert sketch.)
        let (_g, root) = vault();
        let existing_body = "x".repeat(2048);
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &existing_body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        // Rendered new body 100+1 = 101 → 202 < 2049 → shrink_refused.
        let new_body = "y".repeat(100);
        let err = crate::tool_dispatch::dispatch_vault_write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            &new_body,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("shrink_refused:"), "{s}");
        assert!(
            !s.contains("allow_shrink"),
            "must not teach the bypass: {s}"
        );
    }

    #[test]
    fn rendered_length_is_the_basis_trailing_newline_added() {
        // Body of exactly 1024 bytes WITHOUT a trailing newline renders at
        // 1025 — the guard measures the RENDERED form; existing rendered is
        // 2048 → 1025*2 = 2050 ≥ 2048 → allowed. (Raw-body math would also
        // allow here; the odd-existing pair in the CRLF test pins the exact
        // disagree-by-one case.)
        let existing_body = "x".repeat(2048);
        let new_body = "x".repeat(1024); // renders +1 newline
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &existing_body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&created.updated_at)),
            &new_body,
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
    }

    #[test]
    fn crlf_note_measures_byte_exact_body() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing_body = "x".repeat(1100);
        // `_created`: unused in this test (the token is scraped from the CRLF
        // file below); underscore-prefix keeps the clippy -D warnings gate
        // green, matching the plan's own `_g` fixture style.
        let _created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &existing_body,
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let on_disk = std::fs::read_to_string(root.join("wiki/n.md")).unwrap();
        // Convert the stored file to CRLF line endings to simulate a CRLF note.
        let crlf = on_disk.replace('\n', "\r\n");
        std::fs::write(root.join("wiki/n.md"), &crlf).unwrap();
        let token = read_existing_token(&crlf).unwrap();
        // M4 fix: pin the EXACT error, not just the prefix — a prefix-only
        // assert passes even if fence measurement silently drops \r bytes.
        // Existing: raw 1100-body + \n, whole file converted to CRLF → the
        // existing body measures 1102 raw bytes (1100 x's + \r\n — the spec
        // pins RAW rendered bytes, "no normalization step", D4). Rendered
        // new body 549+1 = 550 → 550*2 = 1100 < 1102 → refused. (Plan draft
        // asserted 1101 via a "renderer-normalized" basis that does not
        // exist; the plan's own escape hatch authorizes adjusting the pair —
        // the assert stays exact, per review R1's render semantics.)
        let new_body = "x".repeat(549);
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &new_body,
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("shrink_refused:"), "{s}");
        // Exact-pair assert (guard against silent \r-dropping fence bugs):
        assert_eq!(s.split(':').nth(1), Some("1102"), "{s}");
        assert_eq!(s.split(':').nth(2), Some("550"), "{s}");
    }

    #[test]
    fn shrink_refused_display_has_pinned_shape_without_allow_shrink_hint() {
        let e = WriteNoteError::ShrinkRefused {
            existing_bytes: 12860,
            new_bytes: 505,
        };
        let s = e.to_string();
        assert!(s.starts_with("shrink_refused:12860:505"), "{s}");
        assert!(
            s.contains("re-read the note and resend the full body"),
            "{s}"
        );
        assert!(
            !s.contains("allow_shrink"),
            "must not teach the bypass: {s}"
        );
    }

    #[test]
    fn compaction_marker_display_has_pinned_shape_without_allow_shrink_hint() {
        let e = WriteNoteError::CompactionMarkerRejected {
            marker: "[SKILL_PRUNED]".into(),
        };
        let s = e.to_string();
        assert!(s.starts_with("compaction_marker:[SKILL_PRUNED]"), "{s}");
        assert!(s.contains("rephrase and resend"), "{s}");
        assert!(
            !s.contains("allow_shrink"),
            "must not teach the bypass: {s}"
        );
    }

    // ---- issue #245: frontmatter key-drop guard ----

    /// Well-formed base frontmatter for key-drop fixtures. Index 2 is the
    /// title line, swapped for an unquoted-colon title to DAMAGE the YAML
    /// (strict parse fails; the token reader's tolerant fallback still
    /// reads `updated_at`).
    const KD_BASE: &[&str] = &[
        "okf_version: \"0.1\"",
        "profile: llm-wiki/1",
        "title: T",
        "entity_type: fact",
        "created_at: \"2026-08-27T00:00:00Z\"",
        "updated_at: \"2026-09-25T01:00:00Z\"",
    ];

    /// Build a note: `KD_BASE` (title damaged when `damaged`) + `extra` lines.
    /// Asserts the fixture's damage flag matches reality, so a "damaged"
    /// test can never silently run on the strict tier (or vice versa).
    fn kd_doc(extra: &[&str], damaged: bool) -> String {
        let mut lines: Vec<&str> = KD_BASE.to_vec();
        if damaged {
            lines[2] = "title: Deploy: retro";
        }
        lines.extend_from_slice(extra);
        let doc = format!("---\n{}\n---\nbody\n", lines.join("\n"));
        let inner = collect_frontmatter_fence(&doc).expect("fixture has a fence");
        assert_eq!(
            serde_yaml::from_str::<serde_yaml::Value>(&inner).is_err(),
            damaged,
            "fixture damage flag must match the strict parse: {doc}"
        );
        doc
    }

    fn kd_keys(doc: &str) -> BTreeSet<String> {
        existing_frontmatter_keys(doc).expect("fixture has a fence")
    }

    #[test]
    fn known_keys_is_sorted_ascending() {
        let mut sorted = KNOWN_KEYS;
        sorted.sort_unstable();
        assert_eq!(sorted, KNOWN_KEYS);
    }

    #[test]
    fn rendered_key_set_omits_none_and_empty_optionals() {
        let mut m = fm("T", None);
        m.tags = Some(vec![]);
        assert_eq!(
            rendered_key_set(&m),
            vec![
                "created_at",
                "entity_type",
                "okf_version",
                "profile",
                "title"
            ]
        );
        let mut m = fm("T", Some("2026-09-25T01:00:00Z"));
        m.supersedes = Some("immutable-source-files/agents/v1.md".to_string());
        assert_eq!(rendered_key_set(&m), KNOWN_KEYS.to_vec());
    }

    #[test]
    fn existing_keys_strict_tier_full_set() {
        let keys = kd_keys(&kd_doc(&["tags: [a]"], false));
        let expected: BTreeSet<String> = [
            "created_at",
            "entity_type",
            "okf_version",
            "profile",
            "tags",
            "title",
            "updated_at",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn existing_keys_strict_tier_known_absent_forms() {
        // D2: the exhaustive ABSENT list for the KNOWN optional fields.
        for (line, key) in [
            ("tags:", "tags"),
            ("tags: null", "tags"),
            ("tags: ~", "tags"),
            ("tags: []", "tags"),
            ("tags: [] # none", "tags"),
            ("supersedes:", "supersedes"),
            ("supersedes: null", "supersedes"),
            ("supersedes: ~", "supersedes"),
            ("supersedes: \"\"", "supersedes"),
        ] {
            let keys = kd_keys(&kd_doc(&[line], false));
            assert!(!keys.contains(key), "{line:?} must count ABSENT: {keys:?}");
        }
    }

    #[test]
    fn existing_keys_strict_tier_present_forms() {
        // D2: any other value form counts PRESENT; unknown keys always do.
        for (line, key) in [
            ("tags: \"\"", "tags"),
            ("tags: foo", "tags"),
            ("tags: {}", "tags"),
            (
                "supersedes: immutable-source-files/agents/v1.md",
                "supersedes",
            ),
            ("aliases: []", "aliases"),
            ("aliases: null", "aliases"),
        ] {
            let keys = kd_keys(&kd_doc(&[line], false));
            assert!(keys.contains(key), "{line:?} must count PRESENT: {keys:?}");
        }
    }

    #[test]
    fn existing_keys_strict_tier_non_string_key_is_collected_not_rejected() {
        // D1 (review 2026-10-02): unlike check_round_trip's reject loop, the
        // EXISTING side collects a non-string key (Debug form) and never errors.
        let keys = kd_keys(&kd_doc(&["1: x"], false));
        let debug = format!("{:?}", serde_yaml::Value::Number(1.into()));
        assert!(keys.contains(&debug), "{debug} missing from {keys:?}");
    }

    #[test]
    fn existing_keys_line_scan_tier() {
        // Damaged YAML → tier 3. Title still extracted (key before FIRST ':').
        let keys = kd_keys(&kd_doc(&[], true));
        assert!(
            keys.contains("title") && keys.contains("updated_at"),
            "{keys:?}"
        );

        // Inline absent forms for known optionals count ABSENT…
        for line in [
            "tags: []",
            "tags:",
            "tags: null",
            "tags: ~",
            "supersedes: \"\"",
        ] {
            let keys = kd_keys(&kd_doc(&[line], true));
            let key = line.split(':').next().unwrap();
            assert!(!keys.contains(key), "{line:?} must count ABSENT: {keys:?}");
        }
        // …unless continued on the next line (block sequence stays PRESENT).
        let keys = kd_keys(&kd_doc(&["tags:", "  - a"], true));
        assert!(keys.contains("tags"), "indented continuation: {keys:?}");
        let keys = kd_keys(&kd_doc(&["tags:", "- a"], true));
        assert!(keys.contains("tags"), "`- ` continuation: {keys:?}");
        // Non-empty inline values, and the accepted trailing-comment divergence.
        for line in ["tags: foo", "tags: \"\"", "tags: [] # none", "tags: [a]"] {
            let keys = kd_keys(&kd_doc(&[line], true));
            assert!(
                keys.contains("tags"),
                "{line:?} must count PRESENT: {keys:?}"
            );
        }
        // Unknown keys are PRESENT whatever their value.
        let keys = kd_keys(&kd_doc(&["aliases: []"], true));
        assert!(keys.contains("aliases"), "{keys:?}");
        // Nested block mapping: parent key PRESENT, indented child is not a key.
        let keys = kd_keys(&kd_doc(&["source:", "  url: https://x"], true));
        assert!(keys.contains("source") && !keys.contains("url"), "{keys:?}");
        // D1 (d): no character-class restriction.
        let keys = kd_keys(&kd_doc(&["1: x", "my-key: y"], true));
        assert!(keys.contains("1") && keys.contains("my-key"), "{keys:?}");
    }

    #[test]
    fn line_scan_key_extraction_rule() {
        // D1 (d): column-0, not whitespace/#/-, contains ':'; key = text
        // before the FIRST ':', trimmed, one matching quote pair stripped.
        assert_eq!(
            line_scan_key("\"quoted\": v").map(|(k, _)| k),
            Some("quoted".to_string())
        );
        assert_eq!(
            line_scan_key("'single': v").map(|(k, _)| k),
            Some("single".to_string())
        );
        assert_eq!(
            line_scan_key("a: b: c").map(|(k, r)| (k, r.trim())),
            Some(("a".to_string(), "b: c"))
        );
        assert_eq!(
            line_scan_key("some key: v").map(|(k, _)| k),
            Some("some key".to_string())
        );
        for not_a_key in [
            "# comment: x",
            "- item: x",
            "  indented: x",
            "\tindented: x",
            "",
            "no colon here",
        ] {
            assert!(line_scan_key(not_a_key).is_none(), "{not_a_key:?}");
        }
    }

    #[test]
    fn existing_keys_fence_less_is_none() {
        // Unreachable through write_note (enforce_staleness refuses no_fence
        // first); pinned as defense-in-depth.
        assert!(existing_frontmatter_keys("no fence\n").is_none());
    }

    fn fm_without_tags(token: Option<&str>) -> OkfFrontmatter {
        let mut m = fm("T", token);
        m.tags = None;
        m
    }

    #[test]
    fn key_drop_refused_display_has_pinned_shape_without_flag_name() {
        let e = WriteNoteError::KeyDropRefused {
            keys: vec!["supersedes".into(), "tags".into()],
        };
        assert_eq!(
            e.to_string(),
            "key_drop_refused:supersedes,tags: re-send the complete frontmatter or pass an explicit key-drop confirmation"
        );
        assert!(!e.to_string().contains("allow_key_drop"));
    }

    #[test]
    fn key_drop_unrepresentable_display_has_pinned_shape_without_flag_name() {
        let e = WriteNoteError::KeyDropUnrepresentable {
            keys: vec!["aliases".into(), "type".into()],
        };
        assert_eq!(
            e.to_string(),
            "key_drop_refused:unrepresentable:aliases,type: this note carries keys the writer cannot re-emit; migrate the note to the OKF schema outside this tool, or pass an explicit key-drop confirmation"
        );
        assert!(!e.to_string().contains("allow_key_drop"));
    }

    #[test]
    fn key_preservation_create_path_and_fence_less_pass() {
        assert!(enforce_key_preservation(None, &fm_without_tags(None), false).is_ok());
        // Defense-in-depth: unreachable post-staleness, must not panic.
        assert!(
            enforce_key_preservation(Some("no fence\n"), &fm_without_tags(None), false).is_ok()
        );
    }

    #[test]
    fn key_preservation_refuses_known_drop_names_exactly_that_key() {
        let doc = kd_doc(&["tags: [a]"], false);
        let err = enforce_key_preservation(Some(&doc), &fm_without_tags(None), false).unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
            "{err}"
        );
    }

    #[test]
    fn key_preservation_known_partition_reported_first() {
        // D5 precedence pin (Opus design-c2 MAJOR 1): legacy `type:` + `tags`,
        // payload drops `tags` → KeyDropRefused naming ONLY `tags`.
        let doc = kd_doc(&["type: fact", "tags: [a]"], false);
        let err = enforce_key_preservation(Some(&doc), &fm_without_tags(None), false).unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
            "{err}"
        );
        assert!(!err.to_string().contains("type"), "{err}");
    }

    #[test]
    fn key_preservation_unknown_only_is_unrepresentable() {
        let doc = kd_doc(&["aliases: []"], false);
        let err = enforce_key_preservation(Some(&doc), &fm("T", None), false).unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["aliases"]),
            "{err}"
        );
    }

    #[test]
    fn key_preservation_updated_at_exempt_and_adding_keys_never_refuses() {
        // Existing has no tags; incoming adds tags and carries no updated_at.
        let doc = kd_doc(&[], false);
        assert!(enforce_key_preservation(Some(&doc), &fm("T", None), false).is_ok());
    }

    #[test]
    fn key_preservation_flag_bypasses_both_partitions() {
        let allow_key_drop = true;
        let doc = kd_doc(&["type: fact", "aliases: []", "tags: [a]"], false);
        assert!(
            enforce_key_preservation(Some(&doc), &fm_without_tags(None), allow_key_drop).is_ok()
        );
    }

    /// Seed `rel` with raw `doc` bytes (bypasses write_note — needed for
    /// legacy/damaged/hand-edited fixtures) and return its If-Match token.
    fn kd_seed(root: &Path, rel: &str, doc: &str) -> String {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, doc).unwrap();
        read_existing_token(doc).expect("seed fixture carries a readable token")
    }

    /// Edit with the seed's own body ("body\n") so the shrink guard is inert.
    fn kd_edit(
        root: &Path,
        rel: &str,
        m: &OkfFrontmatter,
        token: &str,
        allow_key_drop: bool,
    ) -> Result<WriteNoteResult, WriteNoteError> {
        let allow_shrink = false;
        write_note(
            root,
            rel,
            m,
            "body\n",
            Some(token),
            allow_shrink,
            allow_key_drop,
        )
    }

    #[test]
    fn key_drop_incident_replay_tags_and_supersedes() {
        let (_g, root) = deposit_vault();
        let v1 = "immutable-source-files/agents/v1.md";
        let v2 = "immutable-source-files/agents/v2.md";
        write_note(
            &root,
            v1,
            &fm("V1", None),
            "old\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let mut m = fm("V2", None);
        m.supersedes = Some(v1.to_string());
        let created = write_note(&root, v2, &m, "body\n", None, NO_SHRINK, NO_KEY_DROP).unwrap();
        // Mangled payload: both optional keys gone.
        let mut mangled = fm_without_tags(Some(&created.updated_at));
        mangled.supersedes = None;
        let err = kd_edit(&root, v2, &mangled, &created.updated_at, NO_KEY_DROP).unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("key_drop_refused:supersedes,tags:"), "{s}");
        assert!(!s.contains("allow_key_drop"), "{s}");
        // Refused write left the file untouched.
        let on_disk = fs::read_to_string(root.join(v2)).unwrap();
        assert_eq!(read_existing_token(&on_disk).unwrap(), created.updated_at);
    }

    #[test]
    fn key_drop_required_field_omission_fails_at_parse_not_guard() {
        // Opus c1 M1: required fields can't reach the guard.
        let v = serde_json::json!({
            "path": "wiki/n.md",
            "frontmatter": {
                "okf_version": "0.1", "profile": "llm-wiki/1", "title": "T",
                "created_at": "2026-09-01T00:00:00Z"
            },
            "body": "b"
        });
        let err = serde_json::from_value::<crate::tool_dispatch::VaultWriteNoteParams>(v)
            .unwrap_err()
            .to_string();
        assert!(err.contains("entity_type"), "{err}");
    }

    #[test]
    fn key_drop_unknown_key_strict_and_line_scan_tiers() {
        for damaged in [false, true] {
            let (_g, root) = vault();
            let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["aliases: []"], damaged));
            let err = kd_edit(
                &root,
                "wiki/n.md",
                &fm("T", Some(&token)),
                &token,
                NO_KEY_DROP,
            )
            .unwrap_err();
            assert!(
                matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["aliases"]),
                "damaged={damaged}: {err}"
            );
            assert!(err.to_string().contains("unrepresentable"), "{err}");
        }
    }

    #[test]
    fn key_drop_mixed_reports_known_first_end_to_end() {
        let (_g, root) = vault();
        let token = kd_seed(
            &root,
            "wiki/n.md",
            &kd_doc(&["type: fact", "tags: [a]"], false),
        );
        let err = kd_edit(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
            "{err}"
        );
    }

    #[test]
    fn key_drop_damaged_yaml_inline_empty_vs_block_tags() {
        // Inline `tags: []` counts absent → dropping it succeeds.
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags: []"], true));
        kd_edit(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .expect("inline-empty tags must count absent");
        // Block-sequence tags is PRESENT → dropping it refuses (guard NOT skipped).
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags:", "  - a"], true));
        let err = kd_edit(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            err.to_string().starts_with("key_drop_refused:tags:"),
            "{err}"
        );
    }

    #[test]
    fn key_drop_damaged_nested_block_mapping_unknown_key() {
        let (_g, root) = vault();
        let token = kd_seed(
            &root,
            "wiki/n.md",
            &kd_doc(&["source:", "  url: https://x"], true),
        );
        let err = kd_edit(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["source"]),
            "{err}"
        );
    }

    #[test]
    fn key_drop_known_absent_forms_never_false_drop() {
        for line in [
            "tags: null",
            "tags: ~",
            "tags: []",
            "supersedes: ~",
            "supersedes: null",
            "supersedes: \"\"",
        ] {
            let (_g, root) = vault();
            let token = kd_seed(&root, "wiki/n.md", &kd_doc(&[line], false));
            kd_edit(
                &root,
                "wiki/n.md",
                &fm_without_tags(Some(&token)),
                &token,
                NO_KEY_DROP,
            )
            .unwrap_or_else(|e| panic!("{line:?} must count absent: {e}"));
        }
    }

    #[test]
    fn key_drop_damaged_present_scalar_forms_refuse() {
        // `tags: foo`, `tags: ""` (non-list), `tags: [] # none` (trailing
        // comment — accepted stricter-direction divergence) all PRESENT.
        for line in ["tags: foo", "tags: \"\"", "tags: [] # none"] {
            let (_g, root) = vault();
            let token = kd_seed(&root, "wiki/n.md", &kd_doc(&[line], true));
            let err = kd_edit(
                &root,
                "wiki/n.md",
                &fm_without_tags(Some(&token)),
                &token,
                NO_KEY_DROP,
            )
            .unwrap_err();
            assert!(
                matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
                "{line:?}: {err}"
            );
        }
        // Non-list tags does not wedge: re-sending non-empty tags succeeds.
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags: \"\""], true));
        kd_edit(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .expect("sending non-empty tags keeps the key");
    }

    #[test]
    fn key_drop_non_string_keys_both_tiers() {
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["1: x"], false));
        let err = kd_edit(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .unwrap_err();
        let debug = format!("{:?}", serde_yaml::Value::Number(1.into()));
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == std::slice::from_ref(&debug)),
            "{err}"
        );
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["1: x", "my-key: y"], true));
        let err = kd_edit(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["1", "my-key"]),
            "{err}"
        );
    }

    #[test]
    fn key_drop_non_deposit_supersedes_drop_needs_flag() {
        // Spec D4 sibling case: a wiki note with a hand-added `supersedes`
        // cannot re-send it (deposit-only), so only the flag gets through.
        // Also pins "drop exactly one key names exactly that key".
        let (_g, root) = vault();
        let token = kd_seed(
            &root,
            "wiki/n.md",
            &kd_doc(
                &[
                    "tags: [a]",
                    "supersedes: immutable-source-files/agents/v1.md",
                ],
                false,
            ),
        );
        let err = kd_edit(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["supersedes"]),
            "{err}"
        );
        let allow_key_drop = true;
        kd_edit(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &token,
            allow_key_drop,
        )
        .expect("explicit confirmation drops the stale pointer");
    }

    #[test]
    fn key_drop_flag_permits_known_and_unrepresentable() {
        let allow_key_drop = true;
        let (_g, root) = vault();
        let token = kd_seed(
            &root,
            "wiki/n.md",
            &kd_doc(&["type: fact", "tags: [a]"], false),
        );
        kd_edit(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&token)),
            &token,
            allow_key_drop,
        )
        .expect("flag bypasses both partitions");
        let on_disk = fs::read_to_string(root.join("wiki/n.md")).unwrap();
        // Line-prefix check: a bare `contains("type:")` would match `entity_type:`.
        assert!(
            !on_disk
                .lines()
                .any(|l| l.starts_with("type:") || l.starts_with("tags:")),
            "{on_disk}"
        );
    }

    #[test]
    fn key_drop_adding_keys_never_refuses() {
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&[], false));
        kd_edit(
            &root,
            "wiki/n.md",
            &fm("T", Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .expect("adding tags is not a drop");
    }

    #[test]
    fn key_drop_crlf_note_refused() {
        let (_g, root) = vault();
        write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "body\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let lf = fs::read_to_string(root.join("wiki/n.md")).unwrap();
        let crlf = lf.replace('\n', "\r\n");
        fs::write(root.join("wiki/n.md"), &crlf).unwrap();
        let token = read_existing_token(&crlf).unwrap();
        let err = kd_edit(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&token)),
            &token,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            err.to_string().starts_with("key_drop_refused:tags:"),
            "{err}"
        );
    }

    #[test]
    fn marker_check_runs_before_key_drop_check() {
        // D6: marker > key-drop.
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags: [a]"], false));
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&token)),
            "[SKILL_PRUNED]\n",
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:"), "{err}");
    }

    #[test]
    fn key_drop_check_runs_before_shrink_check() {
        // D6: key-drop > shrink.
        let (_g, root) = vault();
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &long_body(400),
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&created.updated_at)),
            "tiny\n",
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(
            err.to_string().starts_with("key_drop_refused:"),
            "key-drop must win: {err}"
        );
    }

    #[test]
    fn params_allow_key_drop_omitted_defaults_false_and_true_parses() {
        let fm_json = serde_json::json!({
            "okf_version": "0.1",
            "profile": "llm-wiki/1",
            "title": "T",
            "entity_type": "fact",
            "created_at": "2026-09-01T00:00:00Z"
        });
        let v = serde_json::json!({ "path": "wiki/n.md", "frontmatter": fm_json, "body": "b" });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(!p.allow_key_drop);
        let v = serde_json::json!({ "path": "wiki/n.md", "frontmatter": fm_json, "body": "b", "allow_key_drop": true });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(p.allow_key_drop);
    }

    #[test]
    fn key_drop_refusal_reaches_mcp_surface_via_anyhow() {
        let (_g, root) = vault();
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "body\n",
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let err = crate::tool_dispatch::dispatch_vault_write_note(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&created.updated_at)),
            "body\n",
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("key_drop_refused:tags:"), "{s}");
        assert!(
            !s.contains("allow_key_drop"),
            "must not teach the bypass: {s}"
        );
    }

    #[test]
    fn mcp_vault_write_note_description_teaches_refusals_not_flag() {
        let src = include_str!("../mcp_server.rs");
        let tool = src
            .find("name = \"vault_write_note\"")
            .expect("vault_write_note tool attribute present");
        let after = &src[tool..];
        let open =
            after.find("description = \"").expect("description present") + "description = \"".len();
        let len = after[open..].find('"').expect("description closes");
        let desc = &after[open..open + len];
        assert!(desc.contains("key_drop_refused:{keys}"), "{desc}");
        assert!(
            desc.contains("key_drop_refused:unrepresentable:{keys}"),
            "{desc}"
        );
        assert!(
            !desc.contains("allow_key_drop"),
            "must not teach the bypass: {desc}"
        );
        assert!(
            !desc.contains("allow_shrink"),
            "must not teach the bypass: {desc}"
        );
    }
}
