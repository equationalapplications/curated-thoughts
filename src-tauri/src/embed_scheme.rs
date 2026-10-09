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
//! Scheme is stamped per row (`llm_wiki_entries.embed_scheme`, V27) and the
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

/// Scheme value for verbatim (no instruction) embeddings. The V27 default on
/// `llm_wiki_entries.embed_scheme` and the seeded `wisdom_active_scheme`.
pub const SCHEME_RAW: &str = "raw";

/// The instruction scheme, written by new wisdom-deposit embedding runs.
pub const WRITE_SCHEME: &str = "instr1";

/// Floor-key suffix for `instr1` (`<key>:instr1`); raw keys are unchanged.
pub const SCHEME_SUFFIX_INSTR1: &str = ":instr1";

/// The WRITE-scheme document text for an entry. The WRITE scheme is always
/// `instr1`, so this is unconditionally the canonical instruction prepended
/// (byte-exact, direct concatenation) to the `title\n\nbody` prose —
/// exactly `doc_text_for_scheme(title, body, Scheme::Instr1)`. There is no
/// raw write path; a raw document text exists only for calibration and
/// rollback tooling, via [`doc_text_for_scheme`] with `Scheme::Raw`.
///
/// This is the parity text function: the sweep, both write-time paths, and the
/// phase-2 pre-embed all derive the embedded text through it, so a stored
/// vector always describes exactly the text the WRITE scheme feeds the
/// provider. One function, not per-site `format!`s — a scheme drift here would
/// silently desync parity from what was actually embedded.
pub fn doc_text_for_entry(title: &str, body: &str) -> String {
    doc_text_for_scheme(title, body, Scheme::Instr1)
}

/// Document text for an entry under an explicit scheme: `raw` is the verbatim
/// `title\n\nbody` prose, `instr1` prepends the byte-exact instruction. Only
/// [`doc_text_for_entry`] (the WRITE scheme) feeds production writers; this
/// scheme-parameterised form exists so calibration can build either cell
/// without re-deriving the text by hand.
pub fn doc_text_for_scheme(title: &str, body: &str, scheme: Scheme) -> String {
    match scheme {
        Scheme::Raw => format!("{title}\n\n{body}"),
        Scheme::Instr1 => format!("{QUERY_INSTRUCTION_PREFIX}{title}\n\n{body}"),
    }
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
/// pre-V27 state and defaults to `raw`; a present-but-unknown value is a hard
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
/// `wiki_context`), keeping the table shape: `None` on a pre-V27 table (no
/// `embed_scheme` column on `llm_wiki_entries`) — there is no scheme
/// dimension at all, so readers omit the row filter and embed the query raw,
/// without probing `llm_wiki_meta` (which may not exist either). On the V27+
/// shape this is `Some(read_scheme)` (fail-closed on unknown values).
///
/// Resolve this ONCE per request and hand the value to every scheme-derived
/// step (query prefix, row filter): two independent resolutions could
/// straddle a cutover and pair one scheme's query with another's rows.
pub fn reader_scheme(conn: &Connection) -> Result<Option<Scheme>> {
    let cols = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    if cols.iter().any(|c| c == "embed_scheme") {
        Ok(Some(read_scheme(conn)?))
    } else {
        Ok(None)
    }
}

/// [`reader_scheme`] collapsed to a concrete scheme: the pre-V27 shape is
/// de-facto `raw`.
pub fn read_scheme_for_reader(conn: &Connection) -> Result<Scheme> {
    Ok(reader_scheme(conn)?.unwrap_or(Scheme::Raw))
}

/// The READ-scheme row filter for a gate / `wiki_search` candidate SELECT:
/// `AND embed_scheme = '<scheme>'`, or empty on the pre-V27 shape (`None`).
/// Every candidate reader interpolates this as `{scheme_filter}` inside its
/// SQL literal — the reader inventory test keys on that token — and binds
/// `Scheme::as_str()` (a closed vocabulary, never user input), so the filter
/// and the stamps cannot drift apart.
pub fn scheme_filter_sql(scheme: Option<Scheme>) -> String {
    match scheme {
        Some(s) => format!(" AND embed_scheme = '{}'", s.as_str()),
        None => String::new(),
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
/// The precondition count and the flip run inside ONE `BEGIN IMMEDIATE`
/// transaction: the write lock is held from the count through the upsert, so
/// no writer (e.g. an older binary still stamping `raw`) can land a row
/// between "zero outstanding" and the flip. Idempotent: an already-`instr1`
/// DB is a no-op success.
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
    // `&Connection` (callers hold shared handles), so the transaction is
    // driven by hand rather than `transaction_with_behavior(&mut self)`.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match activate_instr1_locked(conn) {
        Ok(outcome) => {
            conn.execute_batch("COMMIT")?;
            Ok(outcome)
        }
        Err(e) => {
            // The original error is the one worth reporting; a failed
            // rollback leaves the transaction to die with the connection.
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Body of [`activate_instr1`]; runs under the caller's write lock.
fn activate_instr1_locked(conn: &Connection) -> Result<ActivateOutcome> {
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
    /// tests must not depend on extra crates).
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

    /// One Rust string literal: the 1-based line it opens on and its text,
    /// whitespace-collapsed (so layout and rustfmt never change what a check
    /// sees).
    struct Literal {
        line: usize,
        text: String,
    }

    /// Every string literal (`"…"`, `b"…"`, `r#"…"#`) in `src`, as whole
    /// statements regardless of how many lines they span. A small lexer, not a
    /// line-window heuristic: comments are skipped, char literals (`'"'`) and
    /// lifetimes are told apart, escapes are honoured, and raw strings end only
    /// at their own `"` + hash run. Every SQL statement in this crate is a
    /// single literal (dynamic pieces are `{…}` interpolations inside it), so a
    /// literal IS the statement the inventories reason about.
    fn string_literals(src: &str) -> Vec<Literal> {
        let c: Vec<char> = src.chars().collect();
        let is_ident = |ch: char| ch.is_alphanumeric() || ch == '_';
        let mut out = Vec::new();
        let (mut i, mut line) = (0usize, 1usize);
        while i < c.len() {
            match c[i] {
                '\n' => {
                    line += 1;
                    i += 1;
                }
                '/' if c.get(i + 1) == Some(&'/') => {
                    while i < c.len() && c[i] != '\n' {
                        i += 1;
                    }
                }
                '/' if c.get(i + 1) == Some(&'*') => {
                    let mut depth = 0usize;
                    while i < c.len() {
                        if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                            depth += 1;
                            i += 2;
                        } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        } else {
                            line += usize::from(c[i] == '\n');
                            i += 1;
                        }
                    }
                }
                '\'' => {
                    // Char literal (`'"'`, `'\''`, `'x'`) vs lifetime (`'a`).
                    if c.get(i + 1) == Some(&'\\') {
                        i += 2;
                        while i < c.len() && c[i] != '\'' {
                            i += 1;
                        }
                        i += 1;
                    } else if c.get(i + 2) == Some(&'\'') {
                        i += 3;
                    } else {
                        i += 1;
                    }
                }
                '"' => {
                    // Raw string? Walk back over `#`s to an `r` that starts
                    // a token (optionally `br`).
                    let mut k = i;
                    while k > 0 && c[k - 1] == '#' {
                        k -= 1;
                    }
                    let hashes = i - k;
                    let raw = k > 0 && c[k - 1] == 'r' && {
                        let before = k.checked_sub(2).map(|j| c[j]);
                        match before {
                            Some('b') => k < 3 || !is_ident(c[k - 3]),
                            Some(ch) => !is_ident(ch),
                            None => true,
                        }
                    };
                    let open_line = line;
                    let mut text = String::new();
                    i += 1;
                    while i < c.len() {
                        let ch = c[i];
                        if raw {
                            if ch == '"' && (1..=hashes).all(|h| c.get(i + h) == Some(&'#')) {
                                i += 1 + hashes;
                                break;
                            }
                        } else if ch == '\\' {
                            if let Some(&next) = c.get(i + 1) {
                                line += usize::from(next == '\n');
                                text.push(if next == '\n' { ' ' } else { next });
                            }
                            i += 2;
                            continue;
                        } else if ch == '"' {
                            i += 1;
                            break;
                        }
                        line += usize::from(ch == '\n');
                        text.push(ch);
                        i += 1;
                    }
                    out.push(Literal {
                        line: open_line,
                        text: text.split_whitespace().collect::<Vec<_>>().join(" "),
                    });
                }
                _ => i += 1,
            }
        }
        out
    }

    /// 1-based line where the file's test module starts (`#[cfg(test)]`
    /// directly followed by a `mod` item), or `usize::MAX` when there is none.
    /// Literals at/after it are fixtures, not production SQL; a `#[cfg(test)]`
    /// on a lone helper fn does NOT end the production region.
    fn test_module_line(src: &str) -> usize {
        let lines: Vec<&str> = src.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if l.trim() != "#[cfg(test)]" {
                continue;
            }
            let next = lines[i + 1..]
                .iter()
                .map(|l| l.trim())
                .find(|l| !l.is_empty());
            if next.is_some_and(|n| n.starts_with("mod ") || n.starts_with("pub mod ")) {
                return i + 1;
            }
        }
        usize::MAX
    }

    /// `(crate-relative path, production literals)` for every source file.
    fn production_literals() -> Vec<(String, Vec<Literal>)> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        files.sort();
        files
            .iter()
            .filter_map(|file| {
                let content = std::fs::read_to_string(file).ok()?;
                let rel = file
                    .strip_prefix(&src)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string();
                let test_line = test_module_line(&content);
                let lits = string_literals(&content)
                    .into_iter()
                    .filter(|l| l.line < test_line)
                    .collect();
                Some((rel, lits))
            })
            .collect()
    }

    #[test]
    fn string_literal_lexer_sees_whole_statements() {
        // The lexer the inventories stand on: multi-line literals come back
        // whole and whitespace-collapsed; comments, char literals, lifetimes
        // and raw strings never desync it.
        let src = concat!(
            "fn f<'a>(x: &'a str) -> char {\n",
            "    // \"not a literal\"\n",
            "    /* \"nor this\" */\n",
            "    let q = '\"';\n",
            "    let a = \"INSERT INTO t\n",
            "               (x, y)  VALUES (?1, \\\"q\\\")\";\n",
            "    let b = r#\"raw \"inner\" sql\"#;\n",
            "    q\n",
            "}\n",
        );
        let lits = string_literals(src);
        let got: Vec<(usize, &str)> = lits.iter().map(|l| (l.line, l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (5, "INSERT INTO t (x, y) VALUES (?1, \"q\")"),
                (7, "raw \"inner\" sql"),
            ]
        );
        assert_eq!(
            test_module_line("fn a() {}\n#[cfg(test)]\nfn h() {}\n"),
            usize::MAX
        );
        assert_eq!(
            test_module_line("fn a() {}\n#[cfg(test)]\n\nmod tests {}\n"),
            2
        );
    }

    #[test]
    fn writer_inventory_every_embedding_blob_write_stamps_embed_scheme() {
        // Plan-review F4 (docs/superpowers/plans/2026-10-07-issue265-wisdom-
        // instruction-prefix.md): an inventory over the crate's PRODUCTION
        // SQL. Every statement that writes `embedding_blob` into
        // `llm_wiki_entries` must reference `embed_scheme` in the same
        // statement — a vector may never land without its scheme stamp.
        // Statements are whole string literals (see `string_literals`), so
        // reformatting cannot move a stamp "out of the window". Test-module
        // fixtures are out of scope structurally; the stamping writers have
        // dedicated behavioural tests.
        let mut offenders: Vec<String> = Vec::new();
        let mut writers = 0usize;
        for (rel, lits) in production_literals() {
            for lit in lits {
                let t = &lit.text;
                let targets_entries = t.contains("INSERT INTO llm_wiki_entries")
                    || t.contains("UPDATE llm_wiki_entries");
                if !(targets_entries && t.contains("embedding_blob")) {
                    continue;
                }
                writers += 1;
                if !t.contains("embed_scheme") {
                    offenders.push(format!("{rel}:{}: {t}", lit.line));
                }
            }
        }
        assert!(
            writers >= 3,
            "inventory ran cold — expected the production blob writers \
             (embed_sweep, db/commit, db/wisdom), got {writers}"
        );
        assert!(
            offenders.is_empty(),
            "embedding_blob written without an embed_scheme stamp at:\n  {}\n\
             Stamp the statement with embed_scheme (from \
             crate::embed_scheme::WRITE_SCHEME).",
            offenders.join("\n  ")
        );
    }

    #[test]
    fn reader_inventory_every_gate_or_wiki_search_select_filters_embed_scheme() {
        // Plan Task 6 test (e) — mirror of the writer inventory over the READ
        // side: every production SELECT that reads candidate VECTORS out of
        // `llm_wiki_entries` must, in the same statement, either reference
        // `embed_scheme` or interpolate the shared `{scheme_filter}` (from
        // `scheme_filter_sql`) — a vector may never surface through a reader
        // that ignores its scheme stamp.
        //
        // A statement naming `embedding_blob` only as `embedding_blob IS
        // NULL` (the NULL sweep's batch and remaining-count) selects rows that
        // have NO vector and no stamp yet, so it is structurally not a
        // candidate reader — no hand-kept exemption list.
        let mut offenders: Vec<String> = Vec::new();
        let mut readers = 0usize;
        for (rel, lits) in production_literals() {
            for lit in lits {
                let t = &lit.text;
                let reads_candidates = t.contains("SELECT")
                    && t.contains("FROM llm_wiki_entries")
                    && t.replace("embedding_blob IS NULL", "")
                        .contains("embedding_blob")
                    && t.contains("WHERE");
                if !reads_candidates {
                    continue;
                }
                readers += 1;
                if t.contains("embed_scheme") || t.contains("{scheme_filter}") {
                    continue;
                }
                offenders.push(format!("{rel}:{}: {t}", lit.line));
            }
        }
        assert!(
            readers >= 2,
            "inventory ran cold — expected the wisdom_match gate SELECT and \
             the wiki_graph::wiki_search SELECT, got {readers}"
        );
        assert!(
            offenders.is_empty(),
            "gate/wiki_search SELECT reads embedding_blob without an \
             embed_scheme filter at:\n  {}\n\
             Interpolate `{{scheme_filter}}` (from \
             crate::embed_scheme::scheme_filter_sql) into the statement.",
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
        // V27's column and meta seed are ungated (they apply on every open,
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
    fn reader_scheme_degrades_to_raw_on_pre_v27_shape() {
        // A bare connection with a pre-V27 `llm_wiki_entries` (no
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
    fn reader_scheme_keeps_the_table_shape() {
        // Pre-V27: `None` (no column → readers omit the filter, embed raw).
        let bare = Connection::open_in_memory().unwrap();
        bare.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT PRIMARY KEY, embedding_blob BLOB);",
        )
        .unwrap();
        assert_eq!(reader_scheme(&bare).unwrap(), None);
        assert_eq!(scheme_filter_sql(None), "");
        // V27+: the active scheme, and the filter binds its stored form.
        let conn = open_in_memory().unwrap();
        assert_eq!(reader_scheme(&conn).unwrap(), Some(Scheme::Raw));
        assert_eq!(
            scheme_filter_sql(Some(Scheme::Instr1)),
            " AND embed_scheme = 'instr1'"
        );
    }

    #[test]
    fn doc_text_for_scheme_raw_is_verbatim_and_instr1_is_the_write_text() {
        assert_eq!(doc_text_for_scheme("T", "B", Scheme::Raw), "T\n\nB");
        assert_eq!(
            doc_text_for_scheme("T", "B", Scheme::Instr1),
            doc_text_for_entry("T", "B"),
            "the WRITE text is exactly the instr1 doc text"
        );
    }

    #[test]
    fn reader_scheme_fail_closed_on_v27_shape_with_unknown_value() {
        // V27+ shape: `read_scheme_for_reader` is exactly `read_scheme` —
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
    fn reader_scheme_resolves_active_scheme_on_v27_shape() {
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
        // The IMMEDIATE transaction is closed on the refusal path too.
        assert!(
            conn.is_autocommit(),
            "refusal must not leave a transaction open"
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
        assert!(conn.is_autocommit());
        assert_eq!(active_scheme_value(&conn), WRITE_SCHEME);
    }

    #[test]
    fn activate_holds_the_write_lock_from_count_through_flip() {
        // The precondition and the flip are one IMMEDIATE transaction: while
        // another connection holds the write lock (a writer mid-insert),
        // activate cannot even start — it never sees a stale "zero
        // outstanding" and then flips over a freshly landed raw row.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brain.db");
        let conn = crate::db::connection::open_app_db(&path, None).unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        seed_with_blob(&writer, "fact_raw_inflight", SCHEME_RAW);
        conn.busy_timeout(std::time::Duration::from_millis(0))
            .unwrap();
        assert!(
            activate_instr1(&conn, WRITE_SCHEME).is_err(),
            "activate must block on the writer's lock, not race it"
        );
        assert!(conn.is_autocommit());
        writer.execute_batch("COMMIT").unwrap();
        // Once the writer commits, its raw row is counted and refuses.
        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::Refused { outstanding: 1 }
        );
        assert_eq!(read_scheme(&conn).unwrap(), Scheme::Raw);
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
