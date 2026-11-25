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
///
/// Collects EVERY `` await db.execAsync(` `` template block inside the body of
/// `setupDatabase`, in order (9.x ships two; 8.0.2 shipped one). The body is
/// bounded by the first line after the marker that is exactly `}` at column
/// 0 — the dist's function-close shape — so the `execAsync` calls inside the
/// helper functions that follow (`create…IfColumnsExist`, created OUTSIDE the
/// template) are excluded. Each block closes at its own closing backtick +
/// `);`. Blocks are split independently and concatenated.
pub fn extract_setup_database_ddl(js: &str, prefix: &str) -> Vec<String> {
    const MARKER: &str = "async function setupDatabase(db, prefix)";
    const OPEN: &str = "await db.execAsync(`";
    const CLOSE: &str = "`);";

    let base = js
        .find(MARKER)
        .expect("setupDatabase not found in package index.js (marker missing)");
    let after_marker = base + MARKER.len();
    let body_end = {
        let mut offset = after_marker;
        let mut found = None;
        for line in js[after_marker..].split_inclusive('\n') {
            if line.trim_end_matches(['\n', '\r']) == "}" {
                found = Some(offset);
                break;
            }
            offset += line.len();
        }
        found.expect("setupDatabase body end not found (no column-0 `}` line after the marker)")
    };
    let body = &js[after_marker..body_end];

    let mut statements = Vec::new();
    let mut blocks = 0usize;
    let mut cursor = 0usize;
    while let Some(rel) = body[cursor..].find(OPEN) {
        let start = cursor + rel + OPEN.len();
        let end = body[start..]
            .find(CLOSE)
            .map(|i| start + i)
            .unwrap_or_else(|| {
                panic!(
                    "unterminated execAsync template block #{} inside setupDatabase \
                     (no closing backtick + `);` before the body end)",
                    blocks + 1
                )
            });
        let substituted = body[start..end].replace("${prefix}", prefix);
        statements.extend(split_ddl_statements(&substituted));
        blocks += 1;
        cursor = end + CLOSE.len();
    }
    assert!(
        blocks > 0,
        "setupDatabase has zero execAsync template blocks inside its body"
    );
    statements
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

    // Order-SENSITIVE comparison: trigger bodies fragment into
    // order-dependent statements (swapping the enqueue
    // trigger's DELETE/INSERT or moving a body between triggers must
    // FAIL this gate). The extractor returns dist order and the const is
    // verbatim dist order, so identity holds when there is no drift.
    if package_stmts == rust_stmts {
        return Ok(());
    }

    // Drift report uses sorted sets so a pure-order mismatch reads as an
    // explicit order complaint instead of a confusing two-sided diff.
    let mut package_sorted = package_stmts.clone();
    package_sorted.sort();
    let mut rust_sorted = rust_stmts.clone();
    rust_sorted.sort();
    if package_sorted == rust_sorted {
        return Err("DDL drift: statement ORDER differs between \
                    Rust okf_ddl::LLM_WIKI_PACKAGE_DDL and the core-llm-wiki \
                    package while the statement SETS match. Trigger bodies are \
                    order-sensitive — restore the dist order."
            .to_string());
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
        .context(format!(
            "add_column_if_missing: failed to check table '{table}'"
        ))?;
    if !table_exists {
        anyhow::bail!("add_column_if_missing: table '{table}' does not exist");
    }

    let info = existing_columns(conn, table)?;
    if !info.contains(&column.to_string()) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {declared_type}"),
            [],
        )?;
    }
    Ok(())
}

/// Column names of `table` from a single `PRAGMA table_info` snapshot.
/// Shared by the migration gates (V17, V24) and the temporal-index mirror in
/// `okf_ddl` so column detection cannot drift between them. Like the PRAGMA
/// in `add_column_if_missing` above, `table` is interpolated as text, so it
/// gets the same plain-identifier check (see `ensure_plain_identifier`).
pub fn existing_columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    if !is_plain_identifier(table) {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "existing_columns: table '{table}' is not a plain identifier"
        )));
    }
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// SQLite has no parameter binding for identifiers, so `PRAGMA table_info` and
/// `ALTER TABLE` below must interpolate `table`, `column`, and `declared_type`
/// as text. Every caller passes a hardcoded literal today, but the signature
/// offers no protection against a future caller threading a config- or
/// user-derived name through — at which point the interpolation becomes SQL
/// injection. Reject anything that is not a plain identifier up front.
fn ensure_plain_identifier(kind: &str, value: &str) -> anyhow::Result<()> {
    if !is_plain_identifier(value) {
        anyhow::bail!(
            "add_column_if_missing: {kind} '{value}' is not a plain identifier              (expected ASCII letters, digits, or underscore, not starting with a digit)"
        );
    }
    Ok(())
}

fn is_plain_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !value.starts_with(|c: char| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_llm_wiki_ddl_matches_core_llm_wiki_package() {
        package_ddl_matches_rust().expect("DDL drift — update okf_ddl.rs or pin package version");
    }

    /// Two `execAsync` template blocks inside `setupDatabase`, a non-exec
    /// helper call between them, and a following helper function whose own
    /// `execAsync` must NOT be picked up (the 9.x dist shape, spec step 7).
    const TWO_BLOCK_JS: &str = r#"
function unrelated() {
  return 1;
}

// src/db/schema.ts
async function setupDatabase(db, prefix) {
  await db.execAsync(`
    CREATE TABLE IF NOT EXISTS ${prefix}alpha (
      id TEXT PRIMARY KEY,
      body TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS ${prefix}beta (
      id TEXT PRIMARY KEY
    );
  `);
  await createXIfColumnsExist(db, prefix);
  await db.execAsync(`
    CREATE TABLE IF NOT EXISTS ${prefix}gamma (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      entry_id TEXT NOT NULL,
      processed_at INTEGER
    );

    CREATE INDEX IF NOT EXISTS ${prefix}gamma_pending_idx
      ON ${prefix}gamma (entry_id) WHERE processed_at IS NULL;

    CREATE TRIGGER IF NOT EXISTS ${prefix}alpha_gamma_trg
      AFTER DELETE ON ${prefix}alpha
    BEGIN
      INSERT INTO ${prefix}gamma (entry_id) VALUES (OLD.id);
    END;
  `);
}
async function createXIfColumnsExist(db, prefix) {
  await db.execAsync(`CREATE INDEX IF NOT EXISTS ${prefix}helper_idx ON ${prefix}alpha(body);`);
}
"#;

    /// The 8.0.2 dist shape: one template block, then helper calls.
    const ONE_BLOCK_JS: &str = r#"
async function setupDatabase(db, prefix) {
  await db.execAsync(`
    CREATE TABLE IF NOT EXISTS ${prefix}alpha (
      id TEXT PRIMARY KEY
    );

    CREATE INDEX IF NOT EXISTS ${prefix}alpha_id
      ON ${prefix}alpha (id);
  `);
  await createXIfColumnsExist(db, prefix);
}
async function createXIfColumnsExist(db, prefix) {
  await db.execAsync(`CREATE INDEX IF NOT EXISTS ${prefix}helper_idx ON ${prefix}alpha(id);`);
}
"#;

    /// Rust-side text equivalent to `TWO_BLOCK_JS` block 1 + block 2 with
    /// the `p_` prefix applied (the `LLM_WIKI_PACKAGE_DDL` shape).
    const TWO_BLOCK_RUST: &str = r#"
    CREATE TABLE IF NOT EXISTS p_alpha (
      id TEXT PRIMARY KEY,
      body TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS p_beta (
      id TEXT PRIMARY KEY
    );

    CREATE TABLE IF NOT EXISTS p_gamma (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      entry_id TEXT NOT NULL,
      processed_at INTEGER
    );

    CREATE INDEX IF NOT EXISTS p_gamma_pending_idx
      ON p_gamma (entry_id) WHERE processed_at IS NULL;

    CREATE TRIGGER IF NOT EXISTS p_alpha_gamma_trg
      AFTER DELETE ON p_alpha
    BEGIN
      INSERT INTO p_gamma (entry_id) VALUES (OLD.id);
    END;
"#;

    #[test]
    fn extract_concatenates_every_exec_block_inside_setup_database() {
        let got = extract_setup_database_ddl(TWO_BLOCK_JS, "p_");
        assert_eq!(
            got,
            vec![
                "CREATE TABLE IF NOT EXISTS p_alpha ( id TEXT PRIMARY KEY, body TEXT NOT NULL )",
                "CREATE TABLE IF NOT EXISTS p_beta ( id TEXT PRIMARY KEY )",
                "CREATE TABLE IF NOT EXISTS p_gamma ( id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 entry_id TEXT NOT NULL, processed_at INTEGER )",
                "CREATE INDEX IF NOT EXISTS p_gamma_pending_idx ON p_gamma (entry_id) \
                 WHERE processed_at IS NULL",
                "CREATE TRIGGER IF NOT EXISTS p_alpha_gamma_trg AFTER DELETE ON p_alpha \
                 BEGIN INSERT INTO p_gamma (entry_id) VALUES (OLD.id)",
                "END",
            ],
            "block 1 + block 2, in order, prefix substituted"
        );
        assert!(
            got.iter().all(|s| !s.contains("${prefix}")),
            "prefix must be substituted everywhere: {got:?}"
        );
        assert!(
            got.iter().all(|s| !s.contains("helper_idx")),
            "helper-function execAsync outside setupDatabase must be excluded: {got:?}"
        );
    }

    /// Regression pin: on the single-block (8.0.2) shape the extended
    /// extractor returns exactly what the original first-block extractor did.
    #[test]
    fn extract_single_block_shape_unchanged() {
        assert_eq!(
            extract_setup_database_ddl(ONE_BLOCK_JS, "p_"),
            vec![
                "CREATE TABLE IF NOT EXISTS p_alpha ( id TEXT PRIMARY KEY )",
                "CREATE INDEX IF NOT EXISTS p_alpha_id ON p_alpha (id)",
            ]
        );
    }

    /// Spec 7(b): a `CREATE TRIGGER … BEGIN …; END;` body fragments into the
    /// SAME pieces on the package side (extractor) and the Rust side
    /// (`split_ddl_statements` over the const text): `…; END` → two fragments.
    #[test]
    fn trigger_fragments_identically_on_package_and_rust_sides() {
        let expected_trigger_fragments = vec![
            "CREATE TRIGGER IF NOT EXISTS p_alpha_gamma_trg AFTER DELETE ON p_alpha \
             BEGIN INSERT INTO p_gamma (entry_id) VALUES (OLD.id)"
                .to_string(),
            "END".to_string(),
        ];
        let trigger_tail = |v: Vec<String>| -> Vec<String> {
            let i = v
                .iter()
                .position(|s| s.starts_with("CREATE TRIGGER"))
                .unwrap_or_else(|| panic!("no CREATE TRIGGER fragment in {v:?}"));
            v[i..].to_vec()
        };
        let package = extract_setup_database_ddl(TWO_BLOCK_JS, "p_");
        let rust = split_ddl_statements(TWO_BLOCK_RUST);
        assert_eq!(trigger_tail(package.clone()), expected_trigger_fragments);
        assert_eq!(trigger_tail(rust.clone()), expected_trigger_fragments);
        assert_eq!(package, rust, "whole package/Rust statement lists must match");
    }

    /// A one-line block (`execAsync(\`…;\`);`) inside setupDatabase closes at
    /// its own backtick, not at a later block's two-space-indented close.
    #[test]
    fn extract_handles_one_line_block_inside_setup_database() {
        let js = "async function setupDatabase(db, prefix) {\n\
                  \x20 await db.execAsync(`CREATE TABLE IF NOT EXISTS ${prefix}solo (id TEXT);`);\n\
                  }\n\
                  async function helper(db, prefix) {\n\
                  \x20 await db.execAsync(`\n    CREATE INDEX IF NOT EXISTS ${prefix}helper_idx ON x(id);\n  `);\n\
                  }\n";
        assert_eq!(
            extract_setup_database_ddl(js, "p_"),
            vec!["CREATE TABLE IF NOT EXISTS p_solo (id TEXT)"]
        );
    }

    #[test]
    #[should_panic(expected = "setupDatabase not found")]
    fn extract_panics_when_marker_missing() {
        extract_setup_database_ddl("async function other(db, prefix) {\n}\n", "p_");
    }

    #[test]
    #[should_panic(expected = "zero execAsync template blocks")]
    fn extract_panics_when_setup_database_has_no_block() {
        extract_setup_database_ddl(
            "async function setupDatabase(db, prefix) {\n  await other(db);\n}\n\
             async function h(db, prefix) {\n  await db.execAsync(`CREATE TABLE x (a);`);\n}\n",
            "p_",
        );
    }

    #[test]
    #[should_panic(expected = "unterminated execAsync template block")]
    fn extract_panics_on_unterminated_block() {
        extract_setup_database_ddl(
            "async function setupDatabase(db, prefix) {\n  await db.execAsync(`\n    CREATE TABLE x (a);\n}\n",
            "p_",
        );
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
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        super::add_column_if_missing(&conn, "t", "new_col", "TEXT").unwrap();

        conn.execute("INSERT INTO t (id, new_col) VALUES (1, 'hello')", [])
            .unwrap();
        let val: String = conn
            .query_row("SELECT new_col FROM t WHERE id = 1", [], |r| r.get(0))
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
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])
            .unwrap();

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

    /// `existing_columns`: same identifier contract as `add_column_if_missing`
    /// — an injected table name is rejected before the PRAGMA is prepared.
    #[test]
    fn existing_columns_rejects_non_identifier_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", [])
            .unwrap();

        assert_eq!(
            super::existing_columns(&conn, "t").unwrap(),
            vec!["id".to_string(), "name".to_string()]
        );

        for bad in ["t); DROP TABLE t; --", "", "1t", "a b", "t;"] {
            let err = super::existing_columns(&conn, bad).unwrap_err().to_string();
            assert!(err.contains("not a plain identifier"), "{bad:?}: {err}");
        }

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='t'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "table must not be dropped by a rejected identifier");
    }

    /// `add_column_if_missing`: fails with a contextual error when the table
    /// does not exist, rather than leaking a raw SQLite pragma error.
    #[test]
    fn add_column_if_missing_fails_on_missing_table() {
        let conn = Connection::open_in_memory().unwrap();
        let result = super::add_column_if_missing(&conn, "nonexistent_table", "col", "INTEGER");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nonexistent_table"),
            "error message should name the missing table: {msg}"
        );
    }
}
