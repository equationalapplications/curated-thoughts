//! Embed-scheme vocabulary for wisdom embeddings (issue #265 follow-up).
//!
//! Spec: docs/superpowers/specs/2026-10-07-issue265-wisdom-instruction-prefix-design.md.
//!
//! Two schemes exist:
//!
//! - `raw`   — entries and queries embedded verbatim (all pre-#265 rows).
//! - `instr1`— the E5/MTEB query instruction is prepended to **queries only**
//!   (document passages stay raw), the scheme Qwen3 embedding models are
//!   documented for.
//!
//! Scheme is stamped per row (`llm_wiki_entries.embed_scheme`, V26) and the
//! active read scheme in `llm_wiki_meta.wisdom_active_scheme`. Reads are
//! fail-closed: a row written under one scheme is never scored against a
//! query embedded under another (Task 2/3 wire the filters; this module owns
//! the vocabulary and the byte-exact instruction).

use anyhow::{anyhow, Result};
use rusqlite::{Connection, OptionalExtension};

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

/// `llm_wiki_meta` key holding the active read scheme.
pub const ACTIVE_SCHEME_META_KEY: &str = "wisdom_active_scheme";

/// Active read scheme, resolved from `llm_wiki_meta.wisdom_active_scheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// Verbatim (no instruction) — the pre-#265 scheme.
    Raw,
    /// E5/MTEB query instruction on the query side; passages stay raw.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

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
}
