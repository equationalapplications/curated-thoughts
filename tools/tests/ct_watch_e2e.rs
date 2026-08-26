//! End-to-end test for `ct watch` (spec §11 test plan item 4).
//!
//! Setup: temp vault with one file, seed brain.db via `ct ingest --yes`,
//! launch `ct watch --once --json --once-timeout 10s`, modify the file,
//! assert: (a) stdout contains the JSON event line, (b) DB row flipped
//! to status='pending', (c) hash matches the new file content.
//!
//! Fixture notes (deviations from the plan body):
//! - `ct ingest` reads the vault root from `config.json`'s `vault_path`
//!   field (NOT `CURATED_VAULT_ROOT`, which is the `ct watch` knob). We
//!   write a temp `config.json` like `ct_write_cmds.rs::with_ingest_fixture`.
//! - `ct ingest` calls `embed_batch` for any non-empty chunk set, which
//!   would hit Ollama. We set `CURATED_EMBED_STUB=constant8` so the
//!   in-process stub returns deterministic vectors.
//! - The DB stores the canonicalized path (`fs::canonicalize(...).to_string_lossy()`)
//!   per `src-tauri/src/db/queue.rs::enqueue_vault_event`. The temp dir is
//!   under `/tmp` with no symlinks, so `note_path.canonicalize()` matches
//!   the DB-stored string.
//! - The 500ms sleep before modifying the file gives the watcher time to
//!   finish its inotify setup + initial scan. 500ms is enough on the
//!   test host; if it becomes flaky on slower CI, the next step is a
//!   poll-based readiness loop (read brain.db until a 'indexed' row
//!   appears) — but we keep the simple sleep for now per the plan.

use std::process::{Command as StdCommand, Stdio};

use temp_env::with_vars;
use tempfile::TempDir;

#[test]
fn ct_watch_e2e_emits_event_and_flips_db_row_to_pending() {
    let brain = TempDir::new().unwrap();
    let vault = TempDir::new().unwrap();
    let brain_path = brain.path().to_path_buf();
    let vault_path = vault.path().to_path_buf();
    let brain_str = brain_path.to_str().unwrap().to_string();
    let vault_str = vault_path.to_str().unwrap().to_string();

    // Vault with one markdown file. `.md` is indexed via the prose
    // chunker; with such a tiny body it may produce zero chunks, which
    // short-circuits `mark_document_indexed` without hitting the
    // embedder. Either way, we set CURATED_EMBED_STUB=constant8 below
    // to guarantee no Ollama call.
    let note_path = vault_path.join("note.md");
    std::fs::write(&note_path, "initial content").unwrap();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_str.as_str())),
            ("CURATED_VAULT_ROOT", Some(vault_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            // Seed config.json so ct ingest discovers the temp vault.
            // JSON-serialize the vault path so Windows backslashes
            // (and any other JSON-significant chars in the temp
            // directory name — e.g. quotes, backslashes, control
            // chars) round-trip correctly. Direct interpolation
            // produced a malformed config on Windows hosts
            // (CodeRabbit review on PR #96).
            let config_json = serde_json::json!({
                "vault_path": vault_path.to_string_lossy().as_ref()
            })
            .to_string();
            std::fs::write(
                brain_path.join("config.json"),
                config_json,
            )
            .unwrap();

            // Step 1: seed brain.db via `ct ingest --yes`.
            let ingest = StdCommand::new(env!("CARGO_BIN_EXE_ct"))
                .args(["ingest", "--yes"])
                .output()
                .expect("failed to run ct ingest");
            assert!(
                ingest.status.success(),
                "ct ingest failed: status={:?} stderr={}",
                ingest.status,
                String::from_utf8_lossy(&ingest.stderr),
            );

            // Step 2: launch `ct watch --once --json` with a generous
            // timeout so the test isn't flaky on slow CI. The watcher
            // exits as soon as it receives one event.
            let watch = StdCommand::new(env!("CARGO_BIN_EXE_ct"))
                .args(["watch", "--once", "--json", "--once-timeout", "10s"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("failed to spawn ct watch");

            // Step 3: give the watcher time to install its inotify watch
            // and run its initial scan, then modify the file. Empirically
            // 500ms is enough on the test host (inotify setup is ~ms);
            // bump if it becomes flaky.
            std::thread::sleep(std::time::Duration::from_millis(500));
            std::fs::write(&note_path, "modified content with more bytes").unwrap();

            // Step 4: wait for watch to exit (timeout or events).
            let output = watch.wait_with_output().expect("ct watch failed");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            // (a) stdout contains the JSON event emitted by `ct watch`.
            assert!(
                stdout.contains(r#""kind":"modified""#),
                "expected modified event in stdout, got: stdout={stdout} stderr={stderr}",
            );
            assert!(
                stdout.contains(r#""path":"#),
                "expected path field in stdout, got: stdout={stdout} stderr={stderr}",
            );
            assert!(
                stdout.contains(r#""ts_ms":"#),
                "expected ts_ms field in stdout, got: stdout={stdout} stderr={stderr}",
            );
            assert!(
                output.status.success(),
                "ct watch exited non-zero: {:?} stderr={}",
                output.status,
                stderr,
            );

            // (b) DB row flipped to pending. The path stored in the DB
            // is canonicalized by `enqueue_vault_event` (see
            // `src-tauri/src/db/queue.rs`), so query with the same
            // canonical form.
            let conn =
                rusqlite::Connection::open(brain_path.join("brain.db")).expect("open brain.db");
            let canonical = note_path
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let (status, hash): (String, String) = conn
                .query_row(
                    "SELECT status, hash FROM documents WHERE path = ?1",
                    rusqlite::params![canonical],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|e| {
                    panic!("query_row failed for path={canonical}: {e}; stderr was: {stderr}")
                });
            assert_eq!(
                status, "pending",
                "expected status='pending', got '{status}'; stderr={stderr}"
            );

            // (c) hash matches the new content (lowercase hex sha256).
            let expected_hash = {
                use sha2::{Digest, Sha256};
                let bytes = std::fs::read(&note_path).unwrap();
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                let digest = hasher.finalize();
                let mut s = String::with_capacity(digest.len() * 2);
                for byte in digest {
                    s.push_str(&format!("{:02x}", byte));
                }
                s
            };
            assert_eq!(
                hash, expected_hash,
                "DB hash doesn't match file content; stderr={stderr}"
            );
        },
    );
}
