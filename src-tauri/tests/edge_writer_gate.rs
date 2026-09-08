//! Architecture gate: the set of code paths that INSERT into `llm_wiki_edges`
//! is frozen, and CI fails when it changes.
//!
//! ## Why this exists
//!
//! `llm_wiki_edges` endpoints must be **live** — present, with `deleted_at IS
//! NULL`, in `llm_wiki_entries`, `curated_entities`, or `llm_wiki_tasks`.
//! `edge_purge` deliberately retains a **half-live** edge (one endpoint dead,
//! one alive) so the surviving side keeps its connection, which means a writer
//! that admits a dead endpoint mints a row **no cascade ever collects**. The
//! guard is `edge_purge::endpoint_is_live`, and every production writer must
//! call it before inserting.
//!
//! Enforcing that guard is a one-line change. *Noticing* that a new writer
//! exists is the part that kept failing. The design spec twice asserted a
//! writer inventory that the tree did not match: it named `commit_edge_add` as
//! the only production writer while `apply_import`'s edge loop had been
//! inserting unchecked endpoints all along, and it declared the `"self"`
//! endpoint branch exempt while that branch was resolving soft-deleted
//! entities into live-looking edges. Both were found by review, not by CI,
//! because nothing mechanical was watching. Every test in the suite pins the
//! behavior of writers we already knew about; none of them would notice a
//! third one being added tomorrow.
//!
//! This test is that missing check. It is a source scan, not a behavioral
//! test: it cannot tell whether a writer is *correct*, only whether the set of
//! writers has changed without anyone updating the baseline below.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Production writers, as `(file relative to src-tauri/, enclosing symbol)`.
///
/// Deliberately keyed by symbol rather than line number: the spec's line-number
/// inventory drifted into pointing at unrelated code within three commits, and
/// a gate that fails on every unrelated edit above a writer gets muted rather
/// than heeded.
const EXPECTED_PRODUCTION_WRITERS: &[(&str, &str)] = &[
    ("src/db/commit.rs", "commit_edge_add"),
    ("src/db/bundle_apply.rs", "apply_import"),
];

/// `#[cfg(test)]` fixture inserts per file. These are harmless — a fixture
/// writes into a scratch in-memory database — but they are counted so a new
/// production writer cannot hide by being miscounted as a fixture.
const EXPECTED_FIXTURE_COUNTS: &[(&str, usize)] = &[
    ("src/db/bundle_apply.rs", 4),
    ("src/db/commit.rs", 3),
    ("src/db/connections.rs", 3),
    ("src/db/edge_purge.rs", 2),
    ("src/db/bundle_io.rs", 2),
    ("src/db/wiki_forget.rs", 1),
    ("src/db/wisdom.rs", 3),
    ("src/lib.rs", 3),
    ("src/wiki_graph.rs", 1),
];

const REMEDIATION: &str = "\n\
    ── What to do ────────────────────────────────────────────────────────────\n\
    An INSERT into `llm_wiki_edges` appeared, moved, or vanished.\n\
    \n\
    If you added a PRODUCTION writer:\n\
      Prefer routing the write through an existing one — `commit_edge_add`\n\
      (proposal commits) or `apply_import`'s edge loop (OKF bundle import).\n\
      Between them they cover both ways edges legitimately enter the graph.\n\
    \n\
      If a third writer is genuinely required, it MUST reject dead endpoints\n\
      before inserting:\n\
    \n\
          if !edge_purge::endpoint_is_live(conn, source_id)?\n\
              || !edge_purge::endpoint_is_live(conn, target_id)? {\n\
              /* drop the edge and record it; do not insert */\n\
          }\n\
    \n\
      `edge_purge` retains a HALF-LIVE edge on purpose, so an edge written\n\
      with one dead endpoint and one live one dangles for as long as its live\n\
      partner survives and no cascade ever collects it. Skipping this check\n\
      does not fail loudly; it corrupts the graph quietly.\n\
    \n\
      Then add it to EXPECTED_PRODUCTION_WRITERS in this file, and say in the\n\
      commit message why a third writer was needed.\n\
    \n\
    If you added or removed a TEST FIXTURE:\n\
      Just update EXPECTED_FIXTURE_COUNTS below to the reported number.\n\
      Fixtures write to throwaway in-memory databases and need no guard.\n\
    \n\
    Keep docs/superpowers/specs/2026-09-08-wiki-edge-integrity-wave-design.md\n\
    §1.1 in step with whatever you change here.\n\
    ──────────────────────────────────────────────────────────────────────────";

#[derive(Debug)]
struct EdgeInsert {
    file: String,
    line: usize,
    symbol: String,
    in_test_module: bool,
}

/// Blank out `//` line comments so a comment that merely *mentions* the insert
/// cannot register as one. Preserves byte offsets and newlines so line numbers
/// and later slicing stay accurate.
fn strip_line_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            out.push(b);
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            in_string = true;
            out.push(b);
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            // Blank to end of line, keeping the newline.
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
            continue;
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8(out).expect("ASCII-preserving transform")
}

/// Byte ranges covered by `#[cfg(test)]`-attributed modules.
///
/// Line-based on purpose. The obvious implementation — brace-match forward
/// from the module's opening `{` — counts braces inside string literals too,
/// and this crate's SQL and `format!` strings hold enough of them to close the
/// module hundreds of lines early. That bug is not visible as a crash: it
/// silently reclassifies fixtures as production writers, which is the exact
/// failure this gate exists to prevent. It was caught here only because the
/// baseline disagreed.
///
/// The repo is rustfmt-clean (CI enforces it), so a top-level module's closing
/// brace is the next line that is exactly `}`. That needs no lexer.
fn test_module_ranges(src: &str) -> Vec<(usize, usize)> {
    // Byte offset at which each line starts.
    let mut line_starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let lines: Vec<&str> = src.lines().collect();
    let offset_of =
        |line_idx: usize| -> usize { line_starts.get(line_idx).copied().unwrap_or(src.len()) };

    let mut ranges = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_end() != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        // The attribute also appears on `use` statements, which cover no range.
        let Some(decl) = lines.get(i + 1) else {
            break;
        };
        if !(decl.starts_with("mod ") || decl.starts_with("pub mod ")) {
            i += 1;
            continue;
        }
        let mut close = lines.len() - 1;
        for (j, line) in lines.iter().enumerate().skip(i + 2) {
            if *line == "}" {
                close = j;
                break;
            }
        }
        ranges.push((offset_of(i), offset_of(close)));
        i = close + 1;
    }
    ranges
}

/// Nearest preceding `fn <name>` — the symbol that contains `offset`.
///
/// Good enough to name the writer in a failure message; it does not attempt to
/// resolve nesting or closures.
fn enclosing_fn(src: &str, offset: usize) -> String {
    let head = &src[..offset];
    let mut best = None;
    let mut from = 0;
    while let Some(rel) = head[from..].find("fn ") {
        let at = from + rel;
        from = at + 3;
        let before_ok = at == 0
            || head[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        if !before_ok {
            continue;
        }
        let name: String = head[at + 3..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            best = Some(name);
        }
    }
    best.unwrap_or_else(|| "<unknown>".into())
}

/// True when the text immediately before a `llm_wiki_edges` occurrence is an
/// INSERT introducer: `INSERT INTO`, `INSERT OR IGNORE INTO`, `INSERT OR
/// REPLACE INTO`, across any whitespace or line breaks.
fn is_insert_introducer(preceding: &str) -> bool {
    let normalized = preceding.split_whitespace().collect::<Vec<_>>().join(" ");
    let upper = normalized.to_ascii_uppercase();
    let Some(into_at) = upper.rfind("INTO") else {
        return false;
    };
    let Some(insert_at) = upper[..into_at].rfind("INSERT") else {
        return false;
    };
    // Only OR-conflict clauses may sit between INSERT and INTO.
    upper[insert_at + "INSERT".len()..into_at]
        .split_whitespace()
        .all(|w| {
            matches!(
                w,
                "OR" | "IGNORE" | "REPLACE" | "ABORT" | "FAIL" | "ROLLBACK"
            )
        })
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
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

fn scan() -> Vec<EdgeInsert> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_root = root.join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_root, &mut files);
    files.sort();

    let mut found = Vec::new();
    for path in files {
        let raw = std::fs::read_to_string(&path).expect("source file is readable UTF-8");
        let src = strip_line_comments(&raw);
        let ranges = test_module_ranges(&src);
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        let mut from = 0;
        while let Some(rel_at) = src[from..].find("llm_wiki_edges") {
            let at = from + rel_at;
            from = at + "llm_wiki_edges".len();

            let window_start = at.saturating_sub(64);
            if !is_insert_introducer(&src[window_start..at]) {
                continue;
            }
            found.push(EdgeInsert {
                file: rel.clone(),
                line: src[..at].lines().count(),
                symbol: enclosing_fn(&src, at),
                in_test_module: ranges.iter().any(|(s, e)| at > *s && at < *e),
            });
        }
    }
    found
}

/// The production writer set is frozen. See `REMEDIATION` for what to do when
/// this fails.
#[test]
fn production_edge_writers_are_exactly_the_two_guarded_ones() {
    let found = scan();
    let mut actual: Vec<(String, String, usize)> = found
        .iter()
        .filter(|f| !f.in_test_module)
        .map(|f| (f.file.clone(), f.symbol.clone(), f.line))
        .collect();
    actual.sort();

    let expected: Vec<(String, String)> = EXPECTED_PRODUCTION_WRITERS
        .iter()
        .map(|(f, s)| (f.to_string(), s.to_string()))
        .collect();

    let actual_keys: Vec<(String, String)> = actual
        .iter()
        .map(|(f, s, _)| (f.clone(), s.clone()))
        .collect();

    let mut expected_sorted = expected.clone();
    expected_sorted.sort();

    if actual_keys != expected_sorted {
        let rendered = actual
            .iter()
            .map(|(f, s, l)| format!("    {f}:{l}  in `{s}`"))
            .collect::<Vec<_>>()
            .join("\n");
        let want = expected_sorted
            .iter()
            .map(|(f, s)| format!("    {f}  in `{s}`"))
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "production `INSERT INTO llm_wiki_edges` sites changed.\n\n\
             Expected exactly {} production writer(s):\n{}\n\n\
             Found {}:\n{}\n{}",
            expected_sorted.len(),
            want,
            actual.len(),
            if rendered.is_empty() {
                "    (none)".into()
            } else {
                rendered
            },
            REMEDIATION,
        );
    }
}

/// Fixture counts are pinned so a production writer cannot be waved through as
/// "just another test insert".
#[test]
fn test_fixture_edge_inserts_match_the_recorded_baseline() {
    let found = scan();
    let mut actual: BTreeMap<String, usize> = BTreeMap::new();
    for f in found.iter().filter(|f| f.in_test_module) {
        *actual.entry(f.file.clone()).or_insert(0) += 1;
    }

    let expected: BTreeMap<String, usize> = EXPECTED_FIXTURE_COUNTS
        .iter()
        .map(|(f, c)| (f.to_string(), *c))
        .collect();

    if actual != expected {
        let mut diff = String::new();
        let mut files: Vec<&String> = actual.keys().chain(expected.keys()).collect();
        files.sort();
        files.dedup();
        for file in files {
            let a = actual.get(file).copied().unwrap_or(0);
            let e = expected.get(file).copied().unwrap_or(0);
            if a != e {
                diff.push_str(&format!("    {file}: expected {e}, found {a}\n"));
            }
        }
        panic!(
            "`#[cfg(test)]` `INSERT INTO llm_wiki_edges` counts changed.\n\n{}\n{}",
            diff, REMEDIATION,
        );
    }
}

/// The scanner is the gate, so its own classification is worth pinning: if
/// `strip_line_comments` or `test_module_ranges` broke, both gates above would
/// pass vacuously by finding nothing at all.
#[test]
fn scanner_finds_both_categories() {
    let found = scan();
    assert!(
        found.iter().any(|f| !f.in_test_module),
        "scanner found no production edge inserts at all — it is broken, and \
         the gates above would pass vacuously"
    );
    assert!(
        found.iter().filter(|f| f.in_test_module).count() > 5,
        "scanner found almost no test-module edge inserts — `test_module_ranges` \
         is likely misclassifying, which would let a production writer through"
    );
    // `wiki_forget.rs` has exactly one edge insert and it is `seed_edge`,
    // inside `mod tests`. A naive line scan reads it as production. If this
    // flips, the test-module detection regressed.
    let wiki_forget: Vec<&EdgeInsert> = found
        .iter()
        .filter(|f| f.file.ends_with("wiki_forget.rs"))
        .collect();
    assert_eq!(
        wiki_forget.len(),
        1,
        "expected one insert in wiki_forget.rs"
    );
    assert!(
        wiki_forget[0].in_test_module,
        "wiki_forget.rs's `seed_edge` insert is a fixture inside `mod tests`, \
         but the scanner classified it as production"
    );

    // `commit.rs` holds BOTH the production writer and three fixtures, in one
    // file, with the fixtures far below `mod tests`. The first draft of this
    // gate brace-matched through string literals, closed that module ~2700
    // lines early, and promoted all three fixtures to production writers —
    // while the weaker version of this sanity test still passed. Requiring
    // both categories from this one file is what catches that class of bug.
    let commit: Vec<&EdgeInsert> = found
        .iter()
        .filter(|f| f.file.ends_with("db/commit.rs"))
        .collect();
    assert!(
        commit.iter().any(|f| !f.in_test_module),
        "expected commit.rs to hold a production writer"
    );
    assert!(
        commit.iter().filter(|f| f.in_test_module).count() >= 3,
        "expected commit.rs to hold at least 3 fixture inserts inside `mod tests`; \
         found {}. The test-module range detection is likely closing the module \
         early and misreading fixtures as production writers.",
        commit.iter().filter(|f| f.in_test_module).count()
    );
}
