//! Build-time guard: Rust DDL constants must match core-llm-wiki package setupDatabase.

use crate::db::okf_ddl::{LLM_WIKI_PACKAGE_DDL, LLM_WIKI_PREFIX};
use anyhow::Context;
use rusqlite::Connection;

/// Collapse whitespace so cosmetic formatting differences do not fail the diff.
pub fn normalize_ddl_statement(stmt: &str) -> String {
    stmt.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split a DDL batch into individual `CREATE TABLE` / `CREATE INDEX` statements.
pub fn split_ddl_statements(batch: &str) -> Vec<String> {
    batch
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_ddl_statement)
        .collect()
}

/// Extract setupDatabase DDL from core-llm-wiki dist/index.js, substituting `prefix`.
pub fn extract_setup_database_ddl(js: &str, prefix: &str) -> Vec<String> {
    let marker = "async function setupDatabase(db, prefix)";
    let base = js
        .find(marker)
        .expect("setupDatabase not found in package index.js");
    let rel = js[base..]
        .find("await db.execAsync(`")
        .expect("setupDatabase execAsync block not found");
    let start = base + rel + "await db.execAsync(`".len();
    let end = js[start..]
        .find("  `);")
        .map(|i| start + i)
        .expect("setupDatabase execAsync closing not found");
    let template = &js[start..end];
    let substituted = template.replace("${prefix}", prefix);
    split_ddl_statements(&substituted)
}

/// Compare Rust constants against the pinned package; returns Ok or a human-readable diff.
pub fn package_ddl_matches_rust() -> Result<(), String> {
    let js_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../node_modules/@equationalapplications/core-llm-wiki/dist/index.js");
    let js = std::fs::read_to_string(&js_path).map_err(|e| {
        format!(
            "failed to read {}: {e} (run pnpm install)",
            js_path.display()
        )
    })?;

    let package_stmts = extract_setup_database_ddl(&js, LLM_WIKI_PREFIX);
    let rust_stmts = split_ddl_statements(LLM_WIKI_PACKAGE_DDL);

    let mut package_sorted = package_stmts.clone();
    package_sorted.sort();
    let mut rust_sorted = rust_stmts.clone();
    rust_sorted.sort();

    if package_sorted == rust_sorted {
        return Ok(());
    }

    let only_package: Vec<_> = package_sorted
        .iter()
        .filter(|s| !rust_sorted.contains(s))
        .collect();
    let only_rust: Vec<_> = rust_sorted
        .iter()
        .filter(|s| !package_sorted.contains(s))
        .collect();

    Err(format!(
        "DDL drift between Rust okf_ddl::LLM_WIKI_PACKAGE_DDL and core-llm-wiki package.\n\
         Package-only ({}): {:?}\n\
         Rust-only ({}): {:?}",
        only_package.len(),
        only_package,
        only_rust.len(),
        only_rust,
    ))
}

/// Adds a column to a table if it does not already exist. Safe against
/// re-runs on pre-existing columns (idempotent). Uses `PRAGMA table_info`
/// to check for existence before adding.
pub fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    declared_type: &str,
) -> anyhow::Result<()> {
    ensure_plain_identifier("table", table)?;
    ensure_plain_identifier("column", column)?;
    ensure_plain_identifier("type", declared_type)?;

    // Fail with a clear message if the table itself does not exist, rather
    // than letting the raw SQLite error from PRAGMA table_info propagate.
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .context(format!("add_column_if_missing: failed to check table '{table}'"))?;
    if !table_exists {
        anyhow::bail!("add_column_if_missing: table '{table}' does not exist");
    }

    let info: Vec<String> = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get(1))?
        .filter_map(Result::ok)
        .collect();
    if !info.contains(&column.to_string()) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {declared_type}"),
            [],
        )?;
    }
    Ok(())
}

/// SQLite has no parameter binding for identifiers, so `PRAGMA table_info` and
/// `ALTER TABLE` below must interpolate `table`, `column`, and `declared_type`
/// as text. Every caller passes a hardcoded literal today, but the signature
/// offers no protection against a future caller threading a config- or
/// user-derived name through — at which point the interpolation becomes SQL
/// injection. Reject anything that is not a plain identifier up front.
fn ensure_plain_identifier(kind: &str, value: &str) -> anyhow::Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !value.starts_with(|c: char| c.is_ascii_digit());
    if !valid {
        anyhow::bail!(
            "add_column_if_missing: {kind} '{value}' is not a plain identifier              (expected ASCII letters, digits, or underscore, not starting with a digit)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_llm_wiki_ddl_matches_core_llm_wiki_package() {
        package_ddl_matches_rust().expect("DDL drift — update okf_ddl.rs or pin package version");
    }

    #[test]
    fn normalize_ddl_collapses_whitespace() {
        assert_eq!(
            normalize_ddl_statement("CREATE  TABLE\nfoo ( id TEXT )"),
            "CREATE TABLE foo ( id TEXT )"
        );
    }

    /// `add_column_if_missing`: adding a column to a fresh table succeeds and
    /// the column is readable back.
    #[test]
    fn add_column_if_missing_adds_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", []).unwrap();

        super::add_column_if_missing(&conn, "t", "new_col", "TEXT").unwrap();

        conn.execute("INSERT INTO t (id, new_col) VALUES (1, 'hello')", [])
            .unwrap();
        let val: String = conn
            .query_row("SELECT new_col FROM t WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(val, "hello");
    }

    /// `add_column_if_missing`: re-running on an already-present column returns
    /// Ok without error and without changing any row.
    #[test]
    fn add_column_if_missing_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, existing TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO t (id, existing) VALUES (1, '原始值')", [])
            .unwrap();

        // First call adds the column (no-op since it already exists).
        super::add_column_if_missing(&conn, "t", "existing", "TEXT").unwrap();

        // Second call is also a no-op — still Ok, no rows touched.
        super::add_column_if_missing(&conn, "t", "existing", "TEXT").unwrap();

        let val: String = conn
            .query_row("SELECT existing FROM t WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(val, "原始值");
    }

    /// `add_column_if_missing`: rejects identifiers that are not plain
    /// identifiers, so a future caller threading an externally-derived name
    /// through cannot turn the interpolated DDL into SQL injection.
    #[test]
    fn add_column_if_missing_rejects_non_identifier_arguments() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", []).unwrap();

        let injected = "x); DROP TABLE t; --";
        let err = super::add_column_if_missing(&conn, "t", injected, "TEXT")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a plain identifier"), "got: {err}");

        // The table survived, i.e. nothing was executed.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='t'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "table must not be dropped by a rejected identifier");

        for bad in ["", "1col", "a b", "col-name", "col;"] {
            assert!(
                super::add_column_if_missing(&conn, "t", bad, "TEXT").is_err(),
                "expected rejection for {bad:?}"
            );
        }
    }

    /// `add_column_if_missing`: fails with a contextual error when the table
    /// does not exist, rather than leaking a raw SQLite pragma error.
    #[test]
    fn add_column_if_missing_fails_on_missing_table() {
        let conn = Connection::open_in_memory().unwrap();
        let result =
            super::add_column_if_missing(&conn, "nonexistent_table", "col", "INTEGER");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nonexistent_table"),
            "error message should name the missing table: {msg}"
        );
    }
}
