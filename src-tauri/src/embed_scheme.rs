//! Embed-scheme vocabulary for wisdom embeddings (issue #265 follow-up).
//!
//! Spec: docs/superpowers/specs/2026-10-07-issue265-wisdom-instruction-prefix-design.md.
//!
//! Two schemes exist:
//!
//! - `raw`   — entries and queries embedded verbatim (all pre-#265 rows).
//! - `instr1`— the E5/MTEB instruction is prepended to **BOTH sides**:
//!   queries (read path) and document passages (write path, via
//!   `doc_text_for_entry`) — the both-sides "cell E" the spec adopts.
//!
//! Scheme is stamped per row (`llm_wiki_entries.embed_scheme`, V26) and the
//! active read scheme in `llm_wiki_meta.wisdom_active_scheme`. Reads are
//! fail-closed: a row written under one scheme is never scored against a
//! query embedded under another (Task 2/3 wire the filters; this module owns
//! the vocabulary and the byte-exact instruction).

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// The E5/MTEB query instruction, byte-exact as the spec pins it. `\n` is a
/// real newline, there is NO space after `Query:`, and application is direct
/// concatenation (`format!("{prefix}{query}")`) — a Qwen3 mis-instruction is
/// a silent recall regression, so this value must never be "fixed" cosmetically.
pub const QUERY_INSTRUCTION_PREFIX: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:";

/// Scheme value for verbatim (no instruction) embeddings. The V26 default on
/// `llm_wiki_entries.embed_scheme` and the seeded `wisdom_active_scheme`.
pub const SCHEME_RAW: &str = "raw";

/// The instruction scheme, written by new wisdom-deposit embedding runs.
pub const WRITE_SCHEME: &str = "instr1";

/// Floor-key suffix for `instr1` (`<key>:instr1`); raw keys are unchanged.
pub const SCHEME_SUFFIX_INSTR1: &str = ":instr1";

/// The WRITE-scheme document text for an entry: under `instr1` the canonical
/// instruction is prepended (byte-exact, direct concatenation) to the raw
/// `title\n\nbody` prose; under `raw` the text is verbatim.
///
/// This is the parity text function: the sweep, both write-time paths, and the
/// phase-2 pre-embed all derive the embedded text through it, so a stored
/// vector always describes exactly the text the WRITE scheme feeds the
/// provider. One function, not per-site `format!`s — a scheme drift here would
/// silently desync parity from what was actually embedded.
pub fn doc_text_for_entry(title: &str, body: &str) -> String {
    format!("{QUERY_INSTRUCTION_PREFIX}{title}\n\n{body}")
}

/// `llm_wiki_meta` key holding the active read scheme.
pub const ACTIVE_SCHEME_META_KEY: &str = "wisdom_active_scheme";

/// Active read scheme, resolved from `llm_wiki_meta.wisdom_active_scheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// Verbatim (no instruction) — the pre-#265 scheme.
    Raw,
    /// E5/MTEB instruction on BOTH sides: queries (read path, via
    /// `query_text_for_scheme`) and document passages (write path, via
    /// `doc_text_for_entry`).
    Instr1,
}

impl Scheme {
    /// Validate a scheme value read from the database. Unknown values are a
    /// hard error (fail-closed): guessing would silently score across schemes.
    pub fn parse(value: &str) -> Result<Scheme> {
        match value {
            SCHEME_RAW => Ok(Scheme::Raw),
            WRITE_SCHEME => Ok(Scheme::Instr1),
            other => Err(anyhow!(
                "embed_scheme: unknown scheme {other:?} in llm_wiki_meta.{}; \
                 refusing to guess (fail-closed)",
                ACTIVE_SCHEME_META_KEY
            )),
        }
    }

    /// Floor-key suffix: `instr1` keys are `<model_key>:instr1`; raw keys are
    /// unchanged.
    pub fn floor_key_suffix(self) -> &'static str {
        match self {
            Scheme::Raw => "",
            Scheme::Instr1 => SCHEME_SUFFIX_INSTR1,
        }
    }

    /// The scheme's stored representation (`llm_wiki_entries.embed_scheme`
    /// values). Readers bind this — never an inline literal — so the filter
    /// and the stamps cannot drift apart.
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Raw => SCHEME_RAW,
            Scheme::Instr1 => WRITE_SCHEME,
        }
    }
}

/// Resolve the active read scheme from `llm_wiki_meta`. A missing key is the
/// pre-V26 state and defaults to `raw`; a present-but-unknown value is a hard
/// error, never a guess.
pub fn read_scheme(conn: &Connection) -> Result<Scheme> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [ACTIVE_SCHEME_META_KEY],
            |row| row.get(0),
        )
        .optional()?;
    match value.as_deref() {
        None => Ok(Scheme::Raw),
        Some(v) => Scheme::parse(v),
    }
}

/// Floor key for an embed model under a scheme: `instr1` appends the scheme
/// suffix so each (model, scheme) pair calibrates independently; `raw` keys
/// are exactly the pre-#265 keys. `model_key` is a `gate_model_key` result.
pub fn floor_key_for(model_key: &str, scheme: Scheme) -> String {
    format!("{model_key}{}", scheme.floor_key_suffix())
}

/// Query text under a read scheme: under `instr1` the byte-exact instruction
/// is prepended (direct concatenation, IFF the read scheme is `instr1`);
/// under `raw` the text is verbatim. The prefix lands AFTER truncation — it
/// never consumes the 2000-char budget (`truncate_text` runs first on the
/// caller side). This is the read-side parity of `doc_text_for_entry`: both
/// query-prefix call sites derive their text through this one function, so a
/// scheme drift cannot desync the gate from `wiki_search`/`wiki_context`.
pub fn query_text_for_scheme(truncated_query: &str, scheme: Scheme) -> String {
    match scheme {
        Scheme::Raw => truncated_query.to_string(),
        Scheme::Instr1 => format!("{QUERY_INSTRUCTION_PREFIX}{truncated_query}"),
    }
}

/// Read-scheme resolution for generic readers (MCP `wiki_search` /
/// `wiki_context`): on a pre-V26 table shape (no `embed_scheme` column on
/// `llm_wiki_entries`) there is no scheme dimension at all — degrade to raw
/// instead of probing `llm_wiki_meta`, which may not exist either. On the
/// V26+ shape this is exactly `read_scheme` (fail-closed on unknown values).
pub fn read_scheme_for_reader(conn: &Connection) -> Result<Scheme> {
    let cols = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    if cols.iter().any(|c| c == "embed_scheme") {
        read_scheme(conn)
    } else {
        Ok(Scheme::Raw)
    }
}

/// Outcome of [`activate_instr1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateOutcome {
    /// Precondition held and the meta flip executed (one UPDATE).
    Activated,
    /// The read scheme was already `instr1` — idempotent no-op success.
    AlreadyActive,
    /// Refused: `outstanding` live non-null rows are not stamped `instr1`.
    Refused { outstanding: usize },
}

/// Cutover (spec §Migration window semantics, mechanism 3): flip the active
/// read scheme to `instr1`. ONLY `instr1` is an accepted target — any other
/// value (including `raw`) is a hard error, fail-closed. Precondition: zero
/// live non-null rows unstamped `instr1` (counted via the scheme sweep's
/// workset query); the refusal carries the outstanding count for the operator.
/// The flip itself is a single upsert on `llm_wiki_meta` — atomic, and
/// idempotent (an already-`instr1` DB is a no-op success).
pub fn activate_instr1(conn: &Connection, target: &str) -> Result<ActivateOutcome> {
    // Fail closed on anything but the WRITE scheme — including `raw`: there
    // is no sanctioned rollback via this command (spec mechanism 4: rollback
    // is a deliberate owner procedure, not an instant revert).
    match Scheme::parse(target) {
        Ok(Scheme::Instr1) => {}
        _ => anyhow::bail!(
            "activate: only {WRITE_SCHEME:?} is a valid target, got {target:?} \
             (fail-closed; rollback is a manual owner procedure)"
        ),
    }
    let outstanding = crate::embed_sweep::count_unstamped_entries(conn)?;
    if outstanding > 0 {
        return Ok(ActivateOutcome::Refused { outstanding });
    }
    if read_scheme(conn)? == Scheme::Instr1 {
        return Ok(ActivateOutcome::AlreadyActive);
    }
    conn.execute(
        "INSERT INTO llm_wiki_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![ACTIVE_SCHEME_META_KEY, WRITE_SCHEME],
    )?;
    Ok(ActivateOutcome::Activated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use std::path::Path;

    #[test]
    fn write_path_text_carries_the_instruction_prefix() {
        // Test (c), spec §4: the WRITE-scheme document text is
        // `<prefix><title>\n\n<body>` — the prefix glued directly onto the
        // title (no space), raw prose untouched.
        let text = doc_text_for_entry("A title", "A body.");
        assert_eq!(
            text,
            format!("{QUERY_INSTRUCTION_PREFIX}A title\n\nA body.")
        );
        assert!(
            text.starts_with(&format!("{QUERY_INSTRUCTION_PREFIX}A title")),
            "the prefix must be directly concatenated onto the title"
        );
        // The doc side differs from the raw text exactly by the prefix.
        assert_eq!(
            text.strip_prefix(QUERY_INSTRUCTION_PREFIX),
            Some("A title\n\nA body.")
        );
    }

    /// Recursively collect `*.rs` files under `dir` (std-only; the inventory
    /// test must not depend on extra crates).
    fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Extract every SQL statement window that starts a string literal with
    /// `INSERT INTO llm_wiki_entries` or `UPDATE llm_wiki_entries`. A window
    /// runs until the statement's closing `)?;` (capped at 80 lines).
    fn sql_windows(lines: &[&str]) -> Vec<(usize, String)> {
        let mut windows = Vec::new();
        let mut i = 0usize;
        while i < lines.len() {
            let trimmed = lines[i].trim_start();
            let is_start = trimmed.starts_with('"')
                && (trimmed.contains("INSERT INTO llm_wiki_entries")
                    || trimmed.contains("UPDATE llm_wiki_entries"));
            if is_start {
                let start = i;
                let mut j = i;
                while j < lines.len() && j - start < 80 {
                    let t = lines[j].trim_end();
                    if t.ends_with(")?;") || t.ends_with("?);") || t.ends_with("\")?;") {
                        break;
                    }
                    j += 1;
                }
                windows.push((start, lines[start..=j.min(lines.len() - 1)].join("\n")));
                i = j + 1;
            } else {
                i += 1;
            }
        }
        windows
    }

    #[test]
    fn writer_inventory_every_embedding_blob_write_stamps_embed_scheme() {
        // Plan-review F4 (docs/superpowers/plans/2026-10-07-issue265-wisdom-
        // instruction-prefix.md): a grep-level inventory over the crate source.
        // Every SQL statement that writes `embedding_blob` must also reference
        // `embed_scheme` in the same statement — a vector may never land
        // without its scheme stamp. Test-seed INSERTs (which stage fixture
        // blobs for unrelated tests) are exempted explicitly by
        // `file: first SQL line`; adding an exemption must be a conscious
        // review decision, never an accident.
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        files.sort();

        let exempt_first_lines = [
            // Test seeds that stage a fixture blob (or none) and do not test
            // the scheme stamp itself; the stamping writers have their own
            // dedicated tests.
            "embed_sweep.rs: \"INSERT INTO llm_wiki_entries (",
            "db/commit.rs: \"INSERT INTO llm_wiki_entries (",
            "db/commit.rs:             \"INSERT INTO llm_wiki_entries (",
            "db/commit.rs:             \"UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id IN ('fact_b', 'fact_c')\",",
            "db/commit.rs:             \"UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fact_a'\",",
            "db/wisdom.rs:             \"INSERT INTO llm_wiki_entries (",
            "wisdom_match.rs:             \"INSERT INTO llm_wiki_entries (",
            "wisdom_match.rs: \"INSERT INTO llm_wiki_entries VALUES ('a', 'ent', 'T', 'B', 'user_stated', ?1, NULL)\",",
            "wiki_graph.rs:             \"INSERT INTO llm_wiki_entries (",
            "tool_dispatch.rs:                 \"INSERT INTO llm_wiki_entries (id, entity_id, title, tier, embedding_blob)",
            // Test seeds staging fixture rows (blob NULL or dummy) for tests
            // that do not exercise the scheme stamp; the stamping writers
            // have dedicated tests.
            "db/drafts.rs:             \"INSERT INTO llm_wiki_entries (",
            "db/edge_purge.rs:             \"INSERT INTO llm_wiki_entries (",
            "db/wiki_forget.rs:             \"INSERT INTO llm_wiki_entries (",
            "lib.rs:             \"INSERT INTO llm_wiki_entries (",
            "wisdom_deposit.rs:                     \"INSERT INTO llm_wiki_entries (",
        ];

        let mut offenders: Vec<String> = Vec::new();
        let mut writer_windows = 0usize;
        for file in &files {
            let rel = file
                .strip_prefix(&src)
                .unwrap_or(file)
                .to_string_lossy()
                .to_string();
            let content = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let lines: Vec<&str> = content.lines().collect();
            for (start, window) in sql_windows(&lines) {
                let writes_blob = window.contains("embedding_blob")
                    && (window.contains("VALUES") || window.contains("SET "));
                if !writes_blob {
                    continue;
                }
                writer_windows += 1;
                if window.contains("embed_scheme") {
                    continue;
                }
                let key = format!("{}: {}", rel, lines[start].trim_end());
                if exempt_first_lines.contains(&key.as_str()) {
                    continue;
                }
                offenders.push(format!("{rel}:{}", start + 1));
            }
        }

        assert!(
            writer_windows >= 5,
            "inventory ran cold — expected the production + seed blob writers, got {writer_windows}"
        );
        assert!(
            offenders.is_empty(),
            "embedding_blob written without an embed_scheme stamp at:\n  {}\n\
             Either stamp the statement with embed_scheme (from \
             crate::embed_scheme::WRITE_SCHEME) or, for a test-seed INSERT, \
             add an explicit exemption in writer_inventory_…",
            offenders.join("\n  ")
        );
    }

    #[test]
    fn reader_inventory_every_gate_or_wiki_search_select_filters_embed_scheme() {
        // Plan Task 6 test (e) — mirror of the writer inventory over the READ
        // side: every production SQL SELECT that reads gate/wiki_search
        // candidates out of `llm_wiki_entries` WITH `embedding_blob` must
        // reference `embed_scheme` in the same statement — a vector may never
        // surface through a reader that ignores its scheme stamp. Exemptions
        // (statement-start line, `file: first SQL line`) are explicit and
        // commented; adding one must be a conscious review decision.
        //
        // EXEMPT (scheme-sweep infrastructure — the sweep's whole job is to
        // find rows whose state does NOT match the WRITE scheme, so pinning
        // `embed_scheme = ?` there would make it a no-op):
        //   embed_sweep.rs — count_unstamped_entries (`embed_scheme != ?1`).
        //   embed_sweep.rs — pending_null_batch: rows with a NULL blob have
        //     no stamp yet; the sweep finds and stamps them.
        // Everything else selected is a fixture/test helper (id- or
        // length-only lookup, no candidate set) filtered out structurally.
        let exempt_first_lines = [
            "embed_sweep.rs:         \"SELECT COUNT(*) FROM llm_wiki_entries",
            "embed_sweep.rs:         \"SELECT id, title, body FROM llm_wiki_entries",
        ];

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        files.sort();

        let mut offenders: Vec<String> = Vec::new();
        let mut reader_windows = 0usize;
        for file in &files {
            let rel = file
                .strip_prefix(&src)
                .unwrap_or(file)
                .to_string_lossy()
                .to_string();
            let content = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let lines: Vec<&str> = content.lines().collect();
            // First `#[cfg(test)]` in the file: anything at/after it is test
            // code (fixtures with id-only lookups), not a production reader.
            let test_start = lines
                .iter()
                .position(|l| l.contains("#[cfg(test)]"))
                .unwrap_or(lines.len());
            for (i, line) in lines.iter().enumerate() {
                if i >= test_start {
                    break;
                }
                let trimmed = line.trim_start();
                if !(trimmed.starts_with('"') && trimmed.contains("SELECT")) {
                    continue;
                }
                // Two windows: the STATEMENT (literal + 11 lines) decides
                // candidacy — `embedding_blob` must appear inside the SQL
                // itself, not in surrounding code (function signatures name
                // parameters `embedding_blob` too). The CONTEXT (16 lines
                // before through 11 after) decides the embed_scheme check —
                // both real readers assemble the scheme filter in the lines
                // around the SQL string (wiki_graph builds it above,
                // wisdom_match appends below).
                let hi = (i + 12).min(lines.len());
                let statement = lines[i..hi].join("\n");
                let lo = i.saturating_sub(16);
                let context = lines[lo..hi].join("\n");
                let reads_candidates = statement.contains("FROM llm_wiki_entries")
                    && statement.contains("embedding_blob")
                    && statement.contains("WHERE");
                if !reads_candidates {
                    continue;
                }
                reader_windows += 1;
                if context.contains("embed_scheme") {
                    continue;
                }
                let key = format!("{rel}: {}", lines[i].trim_end());
                if exempt_first_lines.contains(&key.as_str()) {
                    continue;
                }
                offenders.push(format!("{rel}:{}", i + 1));
            }
        }

        assert!(
            reader_windows >= 2,
            "inventory ran cold — expected the wisdom_match gate SELECT and \
             the wiki_graph::wiki_search SELECT, got {reader_windows}"
        );
        assert!(
            offenders.is_empty(),
            "gate/wiki_search SELECT reads embedding_blob without an \
             embed_scheme filter at:\n  {}\n\
             Either add `AND embed_scheme = <read scheme>` (from \
             crate::embed_scheme::read_scheme) to the WHERE clause or, for a \
             statement that must not filter, add an explicit commented \
             exemption in reader_inventory_…",
            offenders.join("\n  ")
        );
    }

    #[test]
    fn the_sql_blob_writers_reference_the_write_constant() {
        // The three files owning direct SQL blob writes must drive the stamp
        // from the WRITE_SCHEME constant (not a string literal), so a scheme
        // rename is a compile error. (entities_api.rs writes through
        // db/wisdom.rs; schema_guard.rs only pins the column; both
        // graph_reanchor paths funnel through embed_sweep.rs.)
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for rel in ["embed_sweep.rs", "db/commit.rs", "db/wisdom.rs"] {
            let content =
                std::fs::read_to_string(src.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
            assert!(
                content.contains("crate::embed_scheme::WRITE_SCHEME"),
                "{rel} must stamp embed_scheme from the WRITE_SCHEME constant"
            );
        }
    }

    #[test]
    fn instruction_prefix_is_byte_exact() {
        assert_eq!(
            QUERY_INSTRUCTION_PREFIX,
            "Instruct: Given a web search query, retrieve relevant passages \
             that answer the query\nQuery:"
        );
        // No trailing space; ends exactly at the colon.
        assert!(QUERY_INSTRUCTION_PREFIX.ends_with("Query:"));
        assert_eq!(QUERY_INSTRUCTION_PREFIX.matches('\n').count(), 1);
        assert!(!QUERY_INSTRUCTION_PREFIX.contains("\n "));
    }

    #[test]
    fn read_scheme_defaults_to_raw_when_meta_key_missing() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "DELETE FROM llm_wiki_meta WHERE key = 'wisdom_active_scheme'",
            [],
        )
        .unwrap();
        assert_eq!(read_scheme(&conn).unwrap(), Scheme::Raw);
    }

    #[test]
    fn migration_seeds_wisdom_active_scheme_raw() {
        let conn = open_in_memory().unwrap();
        let value: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'wisdom_active_scheme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(value, "raw");
        // V26's column and meta seed are ungated (they apply on every open,
        // rooted or not). The STAMP is gated on V22, which defers on a
        // rootless test open — so a fresh in-memory brain caps at 21.
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 21);
        let column: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('llm_wiki_entries') \
                 WHERE name = 'embed_scheme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(column, 1);
    }

    #[test]
    fn unknown_scheme_is_hard_error() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "UPDATE llm_wiki_meta SET value = 'some_future_scheme' \
             WHERE key = 'wisdom_active_scheme'",
            [],
        )
        .unwrap();
        let err = read_scheme(&conn).unwrap_err();
        assert!(err.to_string().contains("fail-closed"), "{err}");
    }

    #[test]
    fn floor_key_for_both_schemes() {
        let model = "external:qwen/qwen3-embedding-4b";
        assert_eq!(floor_key_for(model, Scheme::Raw), model);
        assert_eq!(
            floor_key_for(model, Scheme::Instr1),
            "external:qwen/qwen3-embedding-4b:instr1"
        );
    }

    #[test]
    fn reader_scheme_degrades_to_raw_on_pre_v26_shape() {
        // A bare connection with a pre-V26 `llm_wiki_entries` (no
        // `embed_scheme` column) and no meta table: a generic reader must
        // degrade to raw, not fail on the missing migration artifacts.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT PRIMARY KEY, embedding_blob BLOB);",
        )
        .unwrap();
        assert_eq!(read_scheme_for_reader(&conn).unwrap(), Scheme::Raw);
    }

    #[test]
    fn reader_scheme_fail_closed_on_v26_shape_with_unknown_value() {
        // V26+ shape: `read_scheme_for_reader` is exactly `read_scheme` —
        // an unknown meta value is a hard error, never a raw fallback.
        let conn = open_in_memory().unwrap();
        conn.execute(
            "UPDATE llm_wiki_meta SET value = 'some_future_scheme' \
             WHERE key = 'wisdom_active_scheme'",
            [],
        )
        .unwrap();
        let err = read_scheme_for_reader(&conn).unwrap_err();
        assert!(err.to_string().contains("fail-closed"), "{err}");
    }

    #[test]
    fn reader_scheme_resolves_active_scheme_on_v26_shape() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "UPDATE llm_wiki_meta SET value = 'instr1' \
             WHERE key = 'wisdom_active_scheme'",
            [],
        )
        .unwrap();
        assert_eq!(read_scheme_for_reader(&conn).unwrap(), Scheme::Instr1);
    }

    #[test]
    fn both_floor_keys_are_calibrated() {
        assert_eq!(
            crate::wisdom_match::gate_floor("external:qwen/qwen3-embedding-4b"),
            Some(0.70)
        );
        assert_eq!(
            crate::wisdom_match::gate_floor("external:qwen/qwen3-embedding-4b:instr1"),
            Some(0.64)
        );
    }

    // -------------------------------------------------------------------------
    // activate_instr1 (issue #265, plan Task 4): refusal / atomic flip /
    // idempotence / fail-closed target validation.
    // -------------------------------------------------------------------------

    fn seed_with_blob(conn: &Connection, id: &str, scheme: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES (?1, 'ent-1', 'T', 'B', '[]', 'inferred',
                       'librarian_inferred', NULL, NULL, 100, 100, NULL, 0,
                       NULL, x'00000000', ?2, NULL)",
            params![id, scheme],
        )
        .unwrap();
    }

    fn active_scheme_value(conn: &Connection) -> String {
        conn.query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = ?1",
            [ACTIVE_SCHEME_META_KEY],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn activate_refuses_while_unstamped_rows_remain() {
        let conn = open_in_memory().unwrap();
        seed_with_blob(&conn, "fact_raw", SCHEME_RAW);
        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::Refused { outstanding: 1 }
        );
        // Refusal must not touch the meta value.
        assert_eq!(active_scheme_value(&conn), SCHEME_RAW);
        assert_eq!(read_scheme(&conn).unwrap(), Scheme::Raw);
    }

    #[test]
    fn activate_succeeds_atomically_when_raw_count_is_zero() {
        let conn = open_in_memory().unwrap();
        // Only instr1-stamped rows and scheme-agnostic NULL blobs: cutover OK.
        seed_with_blob(&conn, "fact_done", WRITE_SCHEME);
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES ('fact_null', 'ent-1', 'T', 'B', '[]', 'inferred',
                       'librarian_inferred', NULL, NULL, 100, 100, NULL, 0,
                       NULL, NULL, 'raw', NULL)",
            [],
        )
        .unwrap();

        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::Activated
        );
        assert_eq!(active_scheme_value(&conn), WRITE_SCHEME);
        assert_eq!(read_scheme(&conn).unwrap(), Scheme::Instr1);
    }

    #[test]
    fn activate_is_idempotent() {
        let conn = open_in_memory().unwrap();
        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::Activated
        );
        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::AlreadyActive
        );
        assert_eq!(active_scheme_value(&conn), WRITE_SCHEME);
    }

    #[test]
    fn activate_rejects_everything_but_instr1() {
        let conn = open_in_memory().unwrap();
        for target in [SCHEME_RAW, "some_future_scheme", "INSTR1", ""] {
            let err = activate_instr1(&conn, target)
                .expect_err("only instr1 is a valid target (fail-closed)");
            assert!(
                err.to_string().contains("fail-closed"),
                "target {target:?}: {err}"
            );
        }
        // Nothing was flipped.
        assert_eq!(active_scheme_value(&conn), SCHEME_RAW);
    }
}
