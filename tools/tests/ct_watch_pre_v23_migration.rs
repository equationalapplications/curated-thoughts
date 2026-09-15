//! Issue #211 final-review finding: `ct watch`'s per-event raw connection
//! never runs `migrate()`, but its Remove/NotFound branches now call
//! `queries::delete_document`, whose first statement inserts into the V23
//! `curated_proposal_deleted_sources` table. On a brain.db last touched by a
//! pre-V23 build, the first Remove/vanished event after upgrading failed
//! with "no such table" and the delete was dropped (loud eprintln, exit 0)
//! until some migrating open touched the file.
//!
//! The fix runs the rootless migrate (`migrate_open_db`) on the watch
//! startup probe connection, so this test builds the current schema, strips
//! it back to a pre-V23 shape (drop the V23 table + unstamp 23), runs a
//! real `ct watch --once` over a file removal, and asserts the delete lands
//! and the deleted-source provenance row is recorded.

use std::process::{Command as StdCommand, Stdio};

use temp_env::with_vars;
use tempfile::TempDir;

#[test]
fn ct_watch_remove_event_deletes_on_a_pre_v23_brain() {
    let brain = TempDir::new().unwrap();
    let vault = TempDir::new().unwrap();
    let brain_path = brain.path().to_path_buf();
    let vault_path = vault.path().to_path_buf();
    let brain_str = brain_path.to_str().unwrap().to_string();
    let vault_str = vault_path.to_str().unwrap().to_string();

    let note_path = vault_path.join("doomed.md");
    std::fs::write(&note_path, "doomed content").unwrap();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_str.as_str())),
            ("CURATED_VAULT_ROOT", Some(vault_str.as_str())),
        ],
        || {
            // Build the CURRENT schema via the real migration path (same
            // pattern as tests/common/mod.rs::init_brain_db).
            std::fs::write(brain_path.join("config.json"), b"{}\n").unwrap();
            let paths = tauri_app_lib::retrieval::resolve_brain_paths();
            let db = tauri_app_lib::retrieval::AppDb::open_with_config(
                &paths.db_path,
                &paths.config_path,
            )
            .expect("writable brain db open");
            drop(db);

            // Downgrade to a pre-V23 shape: a V22-stamped brain without the
            // V23 table — what a db last touched by a pre-#211 build looks
            // like.
            {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                // Dropping the table takes its index with it.
                conn.execute_batch(
                    "DROP TABLE curated_proposal_deleted_sources;
                     DELETE FROM schema_version WHERE version = 23;",
                )
                .unwrap();
            }

            // Seed a document the watcher will see removed, cited by a
            // pending proposal. The path must match what
            // `enqueue_vault_event` stores and deletes by: on macOS FSEvents
            // delivers already-canonical event paths (`/private/var/...`, the
            // same form `fs::canonicalize` yields for `/var/...` temp dirs),
            // so `abs` and `canonical` coincide with the canonical form.
            let canonical_note = note_path.canonicalize().unwrap();
            let virtual_note_str = canonical_note.to_string_lossy().into_owned();
            {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                conn.execute(
                    "INSERT INTO documents (path, hash, tier, status) \
                     VALUES (?1, 'h_pre_v23', 'user_doc', 'indexed')",
                    rusqlite::params![virtual_note_str],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO curated_proposals \
                         (id, kind, entity_id, proposed_name, proposed_type, reasoning, model, \
                          status, created_at) \
                     VALUES ('prop-pre-v23', 'new_entity', NULL, 'Entity pre v23', NULL, NULL, \
                             'fixture-model', 'pending', 1000)",
                    [],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role) \
                     SELECT 'prop-pre-v23', id, 'evidence' FROM documents WHERE path = ?1",
                    rusqlite::params![virtual_note_str],
                )
                .unwrap();
            }

            // Launch a real watch and remove the file inside the window.
            let watch = StdCommand::new(env!("CARGO_BIN_EXE_ct"))
                .args(["watch", "--once", "--json", "--once-timeout", "15s"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("failed to spawn ct watch");
            // Remove at 2s (not 500ms): the FSEvents stream on macOS takes
            // a moment to start delivering; a removal issued too early is
            // silently missed (empirically verified — 500ms dropped the
            // event, 2s fires it, with FSEvents emitting duplicates).
            std::thread::sleep(std::time::Duration::from_millis(2000));
            std::fs::remove_file(&note_path).unwrap();
            let output = watch.wait_with_output().expect("ct watch failed");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            assert!(
                output.status.success(),
                "ct watch exited non-zero: {:?} stderr={stderr}",
                output.status
            );
            assert!(
                stdout.contains(r#""kind":"removed""#),
                "expected removed event in stdout, got: stdout={stdout} stderr={stderr}"
            );
            assert!(
                !stderr.contains("enqueue failed"),
                "watch must not drop the Remove event's delete; stderr={stderr}"
            );

            let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
            let remaining: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE path = ?1",
                    rusqlite::params![virtual_note_str],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                remaining, 0,
                "document row must be deleted by the Remove event; stderr={stderr}"
            );
            let (proposal_id, role): (String, String) = conn
                .query_row(
                    "SELECT proposal_id, role FROM curated_proposal_deleted_sources \
                     WHERE doc_path = ?1",
                    rusqlite::params![virtual_note_str],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap_or_else(|e| {
                    panic!(
                        "deleted-source provenance row missing for {virtual_note_str}: {e}; \
                         stderr={stderr}"
                    )
                });
            assert_eq!(proposal_id, "prop-pre-v23");
            assert_eq!(role, "evidence");
        },
    );
}
