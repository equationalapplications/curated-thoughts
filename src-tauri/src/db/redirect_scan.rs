//! Structural guard (spec R2.7.5 r21 / r20-m6, Task 7): a source-scan test
//! that walks `src-tauri/src` AND `tools/src` and fails on any
//! `FROM curated_entities` / `JOIN curated_entities` READ that is not on
//! the explicit allowlist below — a newly added query fails CI instead of
//! silently resurrecting merged-away losers that every reader must see
//! through the `live_entities` VIEW.
//!
//! Scan rules:
//! * a READ is a source line containing `FROM curated_entities` or
//!   `JOIN curated_entities` (case-insensitive, `curated_entities` not
//!   followed by an identifier character) that is not a
//!   `DELETE FROM curated_entities` — `INSERT`/`UPDATE` statements name the
//!   table without `FROM`/`JOIN` and pure writers need no entry;
//! * lines inside a `#[cfg(test)]` module are skipped: test assertions
//!   inspect the base table directly by design (they pin the rows readers
//!   must NOT see), and gating them here would force every test module
//!   onto the allowlist, hollowing out the guard.

/// Every production base-table read, each with its one-line reason
/// (spec r21: "each entry names its file and a one-line reason").
///
/// Paths are repo-relative. Entries whose file currently has no read hit
/// are harmless documentation (e.g. `queries.rs` carries only the
/// `clear_vault_tables` DELETEs, which are writers and need no entry — the
/// entry records WHY that file deliberately stays on the base table).
#[allow(dead_code)] // used only by the scan test below
const ALLOWLIST: &[(&str, &str)] = &[
    (
        "src-tauri/src/db/entity_gate.rs",
        "the Task 2 redirect resolver itself must see redirected rows",
    ),
    (
        "src-tauri/src/db/merge_duplicates.rs",
        "the Task 6 merge sweep groups over the base table (R2.7.1)",
    ),
    (
        "src-tauri/src/db/queries.rs",
        "clear_vault_tables deletes vault rows from the base table",
    ),
    (
        "src-tauri/src/db/edge_purge.rs",
        "edge endpoint-liveness must count redirected loser rows or the cascade would drop pre-merge edges (R2.7.5)",
    ),
    (
        "src-tauri/src/db/commit.rs",
        "#189 same-name guard + display/updated_at probes are tombstone-permissive on the base table; ids are resolved in Rust before every keyed mutation",
    ),
    (
        "src-tauri/src/db/bundle_apply.rs",
        "import existence probes stay on the base table (archived-row semantics); bundle ids are resolved in Rust first (r2-m9)",
    ),
    (
        "tools/src/graph_reanchor.rs",
        "orphan predicate mirrors the runtime edge_purge base-table liveness contract",
    ),
    (
        "src-tauri/src/db/schema.rs",
        "the MIGRATION_V26 DDL defines the live_entities VIEW over the base table",
    ),
];

#[cfg(test)]
mod tests {
    use super::ALLOWLIST;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Case-insensitive `FROM curated_entities` / `JOIN curated_entities`
    /// read on this line, excluding `DELETE FROM` (a write).
    fn is_base_table_read(line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        let Some(idx) = lower.find("curated_entities") else {
            return false;
        };
        // `curated_entities` must not be a longer identifier's prefix
        // (e.g. a hypothetical `curated_entities_new`).
        let after = lower[idx + "curated_entities".len()..].chars().next();
        if matches!(after, Some(c) if c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
        let before = &lower[..idx];
        let has_from = before.ends_with("from ") || before.ends_with("from\t");
        let has_join = before.ends_with("join ") || before.ends_with("join\t");
        if !(has_from || has_join) {
            return false;
        }
        // `DELETE FROM curated_entities` is a write, not a read.
        let trimmed = before.trim_end();
        !(has_from && trimmed.ends_with("delete"))
    }

    /// Whether `line` opens a `#[cfg(test)]`-annotated module: we track
    /// test-module regions so test assertions on the base table are not
    /// flagged. `#[cfg(test)]` may sit on the same line or the one before
    /// the `mod` declaration.
    fn opens_test_mod(line: &str, prev_attr: bool) -> bool {
        if prev_attr {
            return line.contains("mod ");
        }
        if line.contains("#[cfg(test)]") {
            return line.contains("mod ");
        }
        false
    }

    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "target" || name.starts_with('.') {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn no_base_table_curated_entities_reads_outside_allowlist() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .expect("src-tauri has a parent")
            .to_path_buf();
        let allowed: HashSet<&str> = ALLOWLIST.iter().map(|(p, _)| *p).collect();

        let mut files = Vec::new();
        walk(&repo_root.join("src-tauri/src"), &mut files);
        walk(&repo_root.join("tools/src"), &mut files);

        let mut violations = Vec::new();
        for file in &files {
            let rel = file
                .strip_prefix(&repo_root)
                .unwrap_or(file)
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(src) = std::fs::read_to_string(file) else {
                violations.push(format!("{rel}: unreadable"));
                continue;
            };
            let mut in_test_mod = false;
            let mut test_depth = 0i32;
            let mut prev_cfg_test_attr = false;
            for (n, line) in src.lines().enumerate() {
                if in_test_mod {
                    // Track braces to find the end of the test module.
                    // Format-string `{}` pairs are balanced, so plain brace
                    // counting stays correct for this codebase's tests.
                    test_depth +=
                        line.matches('{').count() as i32 - line.matches('}').count() as i32;
                    if test_depth <= 0 {
                        in_test_mod = false;
                    }
                    continue;
                }
                if opens_test_mod(line, prev_cfg_test_attr) {
                    in_test_mod = true;
                    test_depth =
                        line.matches('{').count() as i32 - line.matches('}').count() as i32;
                    prev_cfg_test_attr = false;
                    continue;
                }
                prev_cfg_test_attr = line.trim() == "#[cfg(test)]";
                // Skip comments (module docs document the scan itself).
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if is_base_table_read(line) && !allowed.contains(rel.as_str()) {
                    violations.push(format!(
                        "{rel}:{}: base-table read not on the db::redirect_scan \
                         allowlist — select from live_entities instead: {}",
                        n + 1,
                        line.trim()
                    ));
                }
            }
        }
        // The scan must have actually looked at the tree — a pathing
        // mistake must fail loudly, not pass vacuously.
        assert!(
            files.len() > 50,
            "scan found only {} files — repo layout changed?",
            files.len()
        );
        assert!(
            violations.is_empty(),
            "curated_entities reads outside the allowlist (spec R2.7.5 r21):\n{}",
            violations.join("\n")
        );
    }
}
