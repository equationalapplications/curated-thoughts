//! Build-time guard: Rust DDL constants must match core-llm-wiki package setupDatabase.

use crate::db::okf_ddl::{LLM_WIKI_PACKAGE_DDL, LLM_WIKI_PREFIX};
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
