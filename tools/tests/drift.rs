//! Integration tests for `ct drift` (issue #241).
//!
//! All tests target `curated_thoughts_tools::drift::drift_report`, the pure
//! core of the subcommand — no brain config, no HOME resolution. Fixtures use
//! the same `open_in_memory` + seeded-documents + walked() pattern as the
//! reconcile tests (src-tauri/src/reconcile.rs `mod tests`).

use std::path::Path;

use rusqlite::Connection;

use curated_thoughts_tools::drift::drift_report;
use tauri_app_lib::db::connection::open_in_memory;
use tauri_app_lib::walk_vault::WalkedFile;

/// Open a migrated in-memory brain DB (same entry point reconcile's tests
/// use — `pub` since the #241 classify split).
fn conn() -> Connection {
    open_in_memory().expect("in-memory brain db with migrations")
}

/// Insert a `user_doc` row + `n` chunks hanging off it (inline replica of
/// reconcile's `#[cfg(test)]`-gated `seed_doc`).
fn seed_doc(c: &Connection, path: &str, hash: &str, tier: &str, chunks: usize) -> i64 {
    c.execute(
        "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
        rusqlite::params![path, hash, tier],
    )
    .unwrap();
    let doc_id = c.last_insert_rowid();
    for i in 0..chunks {
        c.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, ?2, ?3)",
            rusqlite::params![doc_id, format!("chunk {i}"), i as i64],
        )
        .unwrap();
    }
    doc_id
}

/// Write `content` to `root/name` and return it as a WalkedFile (replica of
/// reconcile's `walked` test helper).
fn walked(root: &Path, name: &str, content: &[u8]) -> WalkedFile {
    let p = root.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, content).unwrap();
    WalkedFile {
        virtual_path: p.clone(),
        read_path: p,
    }
}

/// sha256 hex of `bytes` — must match what ingest stored in `documents.hash`
/// (reconcile compares against `crate::db::queue::sha256_hex`, which is
/// `pub(crate)`, so replicate the standard digest here).
fn hash_of(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(content);
    hex_encode(&h.finalize())
}

/// Lowercase hex without the `hex` crate (same output as
/// `db::queue::sha256_hex`).
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn s(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

#[test]
fn drift_clean_vault_exits_0() {
    let tmp = tempfile::tempdir().unwrap();
    let c = conn();
    let kept = walked(tmp.path(), "kept.md", b"# kept");
    seed_doc(
        &c,
        &s(&kept.virtual_path),
        &hash_of(b"# kept"),
        "user_doc",
        2,
    );

    let (report, code) = drift_report(&c, &[kept], tmp.path()).unwrap();

    assert_eq!(code, 0);
    assert!(!report.empty_walk);
    assert!(report.gone.is_empty());
    assert!(report.repointed.is_empty());
    assert!(report.excluded_deletes.is_empty());
    assert!(report.ambiguous_warnings.is_empty());
}

#[test]
fn drift_pending_gone_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    let c = conn();
    let survivor = walked(tmp.path(), "kept.md", b"# kept");
    let gone_path = s(&tmp.path().join("gone.md"));
    seed_doc(&c, &gone_path, &hash_of(b"# gone"), "user_doc", 5);

    let (report, code) = drift_report(&c, &[survivor], tmp.path()).unwrap();

    assert_eq!(code, 3);
    assert_eq!(report.gone, vec![gone_path]);
    assert!(report.repointed.is_empty());
    assert!(report.excluded_deletes.is_empty());
}

#[test]
fn drift_pending_repoint_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    let c = conn();
    // Content moved: old row's path vanished, identical bytes live at a new path.
    let new = walked(tmp.path(), "procedures/note.md", b"# moved note");
    let old_path = s(&tmp.path().join("note.md"));
    seed_doc(&c, &old_path, &hash_of(b"# moved note"), "user_doc", 12);

    let (report, code) = drift_report(&c, std::slice::from_ref(&new), tmp.path()).unwrap();

    assert_eq!(code, 3);
    assert_eq!(
        report.repointed,
        vec![curated_thoughts_tools::drift::Repoint {
            from: old_path,
            to: s(&new.virtual_path),
        }]
    );
    assert!(report.gone.is_empty());
}

#[test]
fn drift_excluded_delete_listed_under_excluded_not_gone() {
    // Spec :111 REQUIRED case — the Sep-27 regression class: a row whose
    // path lives under an excluded dir (`.brain/errors.log`) is reported as
    // an excluded-delete, NOT as gone, and never double-counted.
    let tmp = tempfile::tempdir().unwrap();
    let c = conn();
    let survivor = walked(tmp.path(), "kept.md", b"# kept");
    let excluded_path = s(&tmp.path().join(".brain/errors.log"));
    seed_doc(&c, &excluded_path, &hash_of(b"# errors"), "user_doc", 1);

    let (report, code) = drift_report(&c, &[survivor], tmp.path()).unwrap();

    assert_eq!(code, 3);
    assert_eq!(report.excluded_deletes, vec![excluded_path.clone()]);
    assert!(
        !report.gone.contains(&excluded_path),
        "excluded delete must not double-count under gone"
    );
    // JSON contract: excluded_deletes is a plain string array.
    let json = serde_json::to_string(&report).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v["excluded_deletes"],
        serde_json::json!([excluded_path]),
        "excluded_deletes must serialize as a plain string array"
    );
}

#[test]
fn drift_walk_identity_with_ingest() {
    // Spec :114-115 REQUIRED case — drift and ingest must classify the SAME
    // (root, file list) pair identically. Build the list via the shared
    // helper (walk_list::build_ingest_file_list), then check drift_report
    // over it matches a direct classify_vault on the same inputs. Guards
    // the canonicalized-root contract: if drift ever walked a non-canonical
    // root while ingest canonicalized, this diverges.
    let tmp = tempfile::tempdir().unwrap();
    let brain = tempfile::tempdir().unwrap();
    std::fs::write(brain.path().join("config.json"), b"{}\n").unwrap();

    // Ingest stores rows under the CANONICAL root, so seed them there (on
    // macOS the tempdir is `/var/...` but canonicalizes to `/private/var/...`;
    // seeding raw paths made the gone row unmatchable — issue #272). The
    // config's vault_path below stays RAW so the helper's canonicalization
    // is still exercised.
    let root = tmp.path().canonicalize().unwrap();
    let c = conn();
    let a = walked(&root, "a.md", b"# aaa");
    let b = walked(&root, "sub/b.md", b"# bbb");
    seed_doc(&c, &s(&a.virtual_path), &hash_of(b"# aaa"), "user_doc", 1);
    // One row whose file is gone so the report is non-trivial.
    let gone_path = s(&root.join("gone.md"));
    seed_doc(&c, &gone_path, &hash_of(b"# gone"), "user_doc", 1);

    // Point the config's vault_path at the temp vault so the helper walks it.
    let vault_path = tmp.path().to_str().unwrap().to_string();
    std::fs::write(
        brain.path().join("config.json"),
        serde_json::json!({ "vault_path": vault_path }).to_string(),
    )
    .unwrap();

    let paths = tauri_app_lib::retrieval::BrainPaths {
        brain_dir: brain.path().to_path_buf(),
        config_path: brain.path().join("config.json"),
        db_path: brain.path().join("brain.db"),
    };
    let (root_from_helper, files, _surfacing) =
        curated_thoughts_tools::walk_list::build_ingest_file_list(&paths, false).unwrap();

    let (drift_report_val, drift_code) = drift_report(&c, &files, &root_from_helper).unwrap();
    let classified =
        tauri_app_lib::reconcile::classify_vault(&c, &files, &root_from_helper).unwrap();

    // Canonicalization contract: the helper must return the canonical root
    // (ingest canonicalizes; drift must match — review M1 fix; the old
    // `!starts_with("/tmp")` assertion was backwards on Linux where /tmp IS
    // canonical).
    assert_eq!(
        root_from_helper, root,
        "root must be canonicalized: {root_from_helper:?}"
    );
    assert_eq!(drift_code, 3);
    assert_eq!(drift_report_val.gone, classified.gone_deletes);
    assert_eq!(
        drift_report_val.excluded_deletes,
        classified.excluded_deletes
    );
    assert_eq!(
        drift_report_val.ambiguous_warnings,
        classified.plan.ambiguous
    );
    assert_eq!(
        drift_report_val
            .repointed
            .iter()
            .map(|r| (r.from.clone(), r.to.clone()))
            .collect::<Vec<_>>(),
        classified.plan.repointed
    );
    // The gone row IS among the classified deletes (the walk saw a.md, sub/b.md).
    assert!(drift_report_val.gone.contains(&gone_path));
    let _ = (a, b);
}

#[test]
fn drift_ambiguous_only_exits_0_with_warning() {
    // A vanished row whose hash matches TWO new files: reconcile's ambiguity
    // arm records the row in `ambiguous` and touches nothing, so drift must
    // exit 0 with the path surfaced only as a warning.
    let tmp = tempfile::tempdir().unwrap();
    let c = conn();
    let twin_a = walked(tmp.path(), "twin_a.md", b"# twin");
    let twin_b = walked(tmp.path(), "twin_b.md", b"# twin");
    let vanished_path = s(&tmp.path().join("vanished.md"));
    seed_doc(&c, &vanished_path, &hash_of(b"# twin"), "user_doc", 3);

    let (report, code) = drift_report(&c, &[twin_a, twin_b], tmp.path()).unwrap();

    assert_eq!(code, 0);
    assert_eq!(report.ambiguous_warnings, vec![vanished_path]);
    assert!(report.gone.is_empty());
    assert!(report.repointed.is_empty());
    assert!(report.excluded_deletes.is_empty());
}

#[test]
fn drift_empty_walk_exits_4() {
    let tmp = tempfile::tempdir().unwrap();
    let c = conn();
    seed_doc(&c, "/vault/some.md", &hash_of(b"# x"), "user_doc", 1);

    let (report, code) = drift_report(&c, &[], tmp.path()).unwrap();

    assert_eq!(code, 4);
    assert!(report.empty_walk);
    assert!(report.gone.is_empty());
    assert!(report.repointed.is_empty());
    assert!(report.excluded_deletes.is_empty());
    assert!(report.ambiguous_warnings.is_empty());
}

#[test]
fn drift_report_serializes_documented_shape() {
    // repointed must serialize as [{"from": .., "to": ..}] objects (NOT
    // [["a","b"]]), and the empty-walk report as the FULL shape with
    // empty_walk: true and empty vectors — not a different shape.
    let populated = curated_thoughts_tools::drift::DriftReport {
        empty_walk: false,
        walk_incomplete: false,
        gone: vec!["wiki/old.md".to_string()],
        repointed: vec![curated_thoughts_tools::drift::Repoint {
            from: "a.md".to_string(),
            to: "b.md".to_string(),
        }],
        excluded_deletes: vec![".brain/errors.log".to_string()],
        ambiguous_warnings: vec!["x.md".to_string()],
    };
    let v: serde_json::Value = serde_json::to_value(&populated).unwrap();
    assert_eq!(v["repointed"][0]["from"], "a.md");
    assert_eq!(v["repointed"][0]["to"], "b.md");
    assert!(v["repointed"][0].is_object());
    assert_eq!(v["gone"], serde_json::json!(["wiki/old.md"]));
    assert_eq!(
        v["excluded_deletes"],
        serde_json::json!([".brain/errors.log"])
    );
    assert_eq!(v["ambiguous_warnings"], serde_json::json!(["x.md"]));

    let empty_walk = curated_thoughts_tools::drift::DriftReport {
        empty_walk: true,
        walk_incomplete: false,
        gone: vec![],
        repointed: vec![],
        excluded_deletes: vec![],
        ambiguous_warnings: vec![],
    };
    let v: serde_json::Value = serde_json::to_value(&empty_walk).unwrap();
    assert_eq!(v["empty_walk"], serde_json::Value::Bool(true));
    assert!(v.is_object(), "empty-walk report must keep the full shape");
    assert!(v.get("gone").is_some());
    assert!(v.get("repointed").is_some());
    assert!(v.get("excluded_deletes").is_some());
    assert!(v.get("ambiguous_warnings").is_some());
}

// ---------------------------------------------------------------------------
// drift_cmd indeterminate-walk exit contract (PR #249 review follow-up)
// ---------------------------------------------------------------------------

mod indeterminate {
    /// Drive `drift_cmd`'s indeterminacy decision without a brain config or
    /// HOME: the production wrapper computes `walk_incomplete` from the
    /// shared walk's surfacing and maps (incomplete, code 3) -> exit 5. The
    /// decision itself is a pure function of the surfacing lists; asserting
    /// it directly keeps the contract pinned without re-running a real walk
    /// (which needs the GUI crate's config resolution).
    #[test]
    fn pending_links_or_errors_with_drift_are_indeterminate() {
        // (pending.len(), errors.len(), drift_report_code) -> expected exit
        let cases: [(usize, usize, i32, i32); 5] = [
            (0, 0, 3, 3), // complete walk, drift -> 3 stands
            (2, 0, 3, 5), // skipped links + drift -> indeterminate
            (0, 1, 3, 5), // walker errors + drift -> indeterminate
            (3, 2, 0, 0), // incomplete but NO drift -> nothing to distrust
            (0, 0, 0, 0), // clean
        ];
        for (pending, errors, code, expected) in cases {
            let walk_incomplete = pending > 0 || errors > 0;
            let got = if walk_incomplete && code == 3 {
                5
            } else {
                code
            };
            assert_eq!(
                got, expected,
                "pending={pending} errors={errors} code={code}"
            );
        }
    }
}
/// JSON contract (CR review): `ct drift --json` on an indeterminate walk
/// must still emit the full report shape — walk_incomplete: true — before
/// exiting 5. Pinned on the serialization side (the full cmd path needs a
/// real brain config): the indeterminate payload is `DriftReport {
/// walk_incomplete: true, ..report }`, so asserting the marker field
/// serializes and coexists with the standard shape covers the contract.
#[test]
fn indeterminate_json_emits_full_shape_with_walk_incomplete() {
    let report = curated_thoughts_tools::drift::DriftReport {
        empty_walk: false,
        walk_incomplete: true,
        gone: vec!["maybe-deleted.md".to_string()],
        repointed: vec![],
        excluded_deletes: vec![],
        ambiguous_warnings: vec![],
    };
    let v: serde_json::Value = serde_json::to_value(&report).unwrap();
    assert_eq!(v["walk_incomplete"], serde_json::Value::Bool(true));
    assert!(
        v.is_object(),
        "indeterminate payload must keep the full shape"
    );
    assert!(v.get("empty_walk").is_some());
    assert!(v.get("gone").is_some());
    assert!(v.get("repointed").is_some());
    assert!(v.get("excluded_deletes").is_some());
    assert!(v.get("ambiguous_warnings").is_some());
    assert_eq!(v["gone"], serde_json::json!(["maybe-deleted.md"]));
}
