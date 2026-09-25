//! Report-only repair scan: find notes whose frontmatter fails strict parse.
//!
//! Lists notes under `wiki/` and `immutable-source-files/agents/` whose
//! If-Match token cannot be read (the future `existing_unparsable:*` clients
//! of `read_existing_token`). NEVER modifies any file — report-only by
//! contract (issue #231 follow-up).
//!
//! Classification reuses the write path's `read_existing_token` (strict-first,
//! tolerant fallback): a note is reported only when BOTH the strict parse and
//! the tolerant fallback fail, i.e. it would yield `existing_unparsable:*` on
//! a write. Notes that strict-parse fail but heal via the fallback are NOT
//! reported — they are exactly what the healing path fixes.

use std::path::Path;

/// One note whose If-Match token could not be read at all.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnparsableNote {
    pub path: String,
    /// Mirrors the write-path error contract:
    /// `existing_unparsable:<no_fence|no_token|parse>`.
    pub reason: String,
}

/// Walk `wiki/` and `immutable-source-files/agents/` under `vault_root`,
/// reporting every `.md` file whose token read fails.
///
/// REPORT-ONLY: reads files, writes nothing.
pub fn scan_unparsable_notes(vault_root: &Path) -> Vec<UnparsableNote> {
    let mut hits: Vec<UnparsableNote> = Vec::new();
    let scan_roots = [
        vault_root.join("wiki"),
        vault_root.join("immutable-source-files").join("agents"),
    ];
    for root in scan_roots {
        if !root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.path().extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                hits.push(UnparsableNote {
                    path: entry.path().display().to_string(),
                    reason: "existing_unparsable:parse".to_string(),
                });
                continue;
            };
            if let Err(e) = super::write::read_existing_token(&content) {
                hits.push(UnparsableNote {
                    path: entry.path().display().to_string(),
                    reason: format!("existing_unparsable:{e}"),
                });
            }
        }
    }
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full frontmatter with a valid `updated_at`: token read succeeds.
    const CLEAN: &str = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Fine\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\n---\nbody\n";

    #[test]
    fn scan_reports_broken_and_clean_notes_correctly() {
        let tmp = tempfile::tempdir().unwrap();
        let wiki = tmp.path().join("wiki");
        std::fs::create_dir_all(&wiki).unwrap();
        // broken: no `updated_at` at all → no_token even via fallback
        std::fs::write(
            wiki.join("broken.md"),
            CLEAN.replace("updated_at: 2026-09-25T01:00:00Z\n", ""),
        )
        .unwrap();
        std::fs::write(wiki.join("clean.md"), CLEAN).unwrap();
        let hits = scan_unparsable_notes(tmp.path());
        assert_eq!(hits.len(), 1, "got: {hits:?}");
        assert!(hits[0].path.ends_with("broken.md"));
        assert_eq!(hits[0].reason, "existing_unparsable:no_token");
    }

    #[test]
    fn scan_classifies_no_fence_and_duplicate_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let wiki = tmp.path().join("wiki");
        std::fs::create_dir_all(&wiki).unwrap();
        // no fence at all
        std::fs::write(wiki.join("nofence.md"), "just some text\n").unwrap();
        // duplicate `updated_at:` lines → fallback refuses to pick → parse
        std::fs::write(
            wiki.join("dup.md"),
            "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Dup\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\nupdated_at: 2026-09-25T02:00:00Z\n---\n",
        )
        .unwrap();
        let hits = scan_unparsable_notes(tmp.path());
        assert_eq!(hits.len(), 2, "got: {hits:?}");
        assert!(hits[0].path.ends_with("dup.md"));
        assert_eq!(hits[0].reason, "existing_unparsable:parse");
        assert!(hits[1].path.ends_with("nofence.md"));
        assert_eq!(hits[1].reason, "existing_unparsable:no_fence");
    }

    #[test]
    fn scan_leaves_file_bytes_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let wiki = tmp.path().join("wiki");
        std::fs::create_dir_all(&wiki).unwrap();
        let broken_path = wiki.join("broken.md");
        // duplicate `updated_at:` lines → strict parse fails, fallback
        // refuses duplicates → the scan's worst-case input, and the file
        // must come through the scan byte-identical.
        let bytes: &[u8] = b"---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Broken\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\nupdated_at: 2026-09-25T02:00:00Z\n---\nbody\n";
        std::fs::write(&broken_path, bytes).unwrap();
        let before = std::fs::read(&broken_path).unwrap();
        let hits = scan_unparsable_notes(tmp.path());
        let after = std::fs::read(&broken_path).unwrap();
        assert_eq!(before, after, "scan must NEVER modify files");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].reason, "existing_unparsable:parse");
    }

    #[test]
    fn scan_covers_agents_dir_and_ignores_other_dirs_and_non_md() {
        let tmp = tempfile::tempdir().unwrap();
        let agents = tmp.path().join("immutable-source-files").join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let outside = tmp.path().join("documents");
        std::fs::create_dir_all(&outside).unwrap();
        let broken = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Broken\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n---\n";
        // NOTE (controller, after initially "correcting" this): a colon-title
        // fixture would NOT be reported here — it fails strict parse but the
        // tolerant fallback recovers its token, i.e. it heals on the next
        // edit. This scan reports only notes that fail BOTH paths. That is
        // the designed semantics (see module doc), verified with a serde_yaml
        // probe before restoring the child's original fixture.
        std::fs::write(agents.join("broken-agent.md"), broken).unwrap();
        // outside the scan roots: must NOT be reported
        std::fs::write(outside.join("broken-outside.md"), broken).unwrap();
        // non-.md inside a scan root: must NOT be reported
        std::fs::write(agents.join("notes.txt"), "garbage\n").unwrap();
        let hits = scan_unparsable_notes(tmp.path());
        assert_eq!(hits.len(), 1, "got: {hits:?}");
        assert!(hits[0].path.ends_with("broken-agent.md"));
        assert_eq!(hits[0].reason, "existing_unparsable:no_token");
    }

    #[test]
    fn scan_on_missing_roots_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan_unparsable_notes(tmp.path()).is_empty());
    }
}
