//! Drift-fixups Task 2: process-level exit-code coverage for `ct watch`.
//!
//! The unit tests in `tools/src/cmds.rs::tests` already cover
//! `watch_run`'s `Result<i32>` mapping. This file pins the
//! *process-level* translation at the `main()` boundary — i.e. that
//! `Ok(code)` actually becomes a process exit with that exact status
//! — so any future refactor that accidentally drops a `?` between
//! `watch_run` and `exit()` is caught.
//!
//! Spec §5 exit-code contract:
//!   0 ok / clean shutdown
//!   1 config error
//!   2 lock conflict     (covered at unit level; hard to fork at
//!                        integration level reliably — see comment
//!                        in `ct_watch_smoke_exits_zero`).
//!   3 DB/schema error
//!   4 notify init failure
//!
//! We do not re-implement the lock-holder cross-process dance here;
//! the unit test in `cmds.rs` owns that contract because it can
//! hold `VaultLock` in the same process and call `watch_run` directly.

use std::process::Command;
use tempfile::TempDir;

mod common;

fn ct() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ct"))
}

/// Set `CURATED_BRAIN_DIR` for the parent process so `init_brain_db`'s
/// `resolve_brain_paths` env-var lookup resolves into the test's
/// TempDir, then spawn the binary with the same env. We avoid
/// `temp_env::with_var` here because that scope-wraps closures and we
/// want `init_brain_db` to run before `ct()` to capture the
/// desired schema state.
fn seed_brain_and_run(brain_tmp: &TempDir, vault_root: &str, args: &[&str]) -> std::process::Output {
    let brain_dir = brain_tmp.path().to_str().unwrap().to_string();
    temp_env::with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_dir.as_str())),
            ("CURATED_BRAIN_DB", None::<&str>),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
        ],
        || {
            common::init_brain_db(brain_tmp.path());
            ct()
                .env("CURATED_BRAIN_DIR", &brain_dir)
                .env("CURATED_VAULT_ROOT", vault_root)
                .env_remove("CURATED_BRAIN_DB")
                .env_remove("CURATED_BRAIN_CONFIG")
                .args(args)
                .output()
                .expect("failed to spawn ct watch")
        },
    )
}

/// Exit 0: clean shutdown in `--once` mode with a valid brain.db +
/// valid vault directory. Sanity check that the existing happy path
/// still exits 0 after the refactor.
#[test]
fn ct_watch_once_timeout_exits_0_on_clean_shutdown() {
    let brain_tmp = TempDir::new().unwrap();
    let vault_tmp = TempDir::new().unwrap();
    let out = seed_brain_and_run(
        &brain_tmp,
        vault_tmp.path().to_str().unwrap(),
        &["watch", "--once", "--once-timeout", "200ms"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "--once-mode clean shutdown must exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Exit 1: missing `CURATED_VAULT_ROOT` env var. `watch_run` returns
/// `Err` from the env-var path; `main()` translates that to exit 1.
#[test]
fn ct_watch_missing_vault_root_exits_1() {
    let brain_tmp = TempDir::new().unwrap();
    let brain_dir = brain_tmp.path().to_str().unwrap().to_string();
    let out = ct()
        .env("CURATED_BRAIN_DIR", &brain_dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .env_remove("CURATED_VAULT_ROOT")
        .args(["watch", "--once", "--once-timeout", "200ms"])
        .output()
        .expect("failed to spawn ct watch");
    assert_eq!(
        out.status.code(),
        Some(1),
        "missing CURATED_VAULT_ROOT must exit 1; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Exit 3: `brain.db` is a directory (not a regular file). SQLite
/// refuses to open a directory as a DB file (returns
/// `SQLITE_CANTOPEN` / "unable to open database file" / "is a
/// directory"). The refactor adds a fail-fast `open_rw` probe at
/// watcher startup that surfaces this as exit 3 instead of letting
/// the binary silently loop with no DB connectivity.
#[test]
fn ct_watch_brain_db_is_directory_exits_3() {
    // `parent_tmp` is what we point CURATED_BRAIN_DIR at. Inside it,
    // `brain.db` exists as a *directory* rather than a regular file,
    // so `open_rw` will fail on open with a deterministic "is a
    // directory" error.
    let parent_tmp = TempDir::new().unwrap();
    let vault_tmp = TempDir::new().unwrap();
    std::fs::create_dir(parent_tmp.path().join("brain.db"))
        .expect("create brain.db as directory");

    let env_brain_dir = parent_tmp.path().to_str().unwrap().to_string();
    let vault_root = vault_tmp.path().to_str().unwrap().to_string();

    let out = temp_env::with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(env_brain_dir.as_str())),
            ("CURATED_BRAIN_DB", None::<&str>),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
            ("CURATED_VAULT_ROOT", Some(vault_root.as_str())),
        ],
        || {
            ct()
                .env("CURATED_BRAIN_DIR", &env_brain_dir)
                .env("CURATED_VAULT_ROOT", &vault_root)
                .env_remove("CURATED_BRAIN_DB")
                .env_remove("CURATED_BRAIN_CONFIG")
                .args(["watch", "--once", "--once-timeout", "200ms"])
                .output()
                .expect("failed to spawn ct watch")
        },
    );

    assert_eq!(
        out.status.code(),
        Some(3),
        "brain.db = directory must exit 3 (DB/schema); stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Exit 4: vault root path is a regular file, not a directory. notify
/// backends differ on whether they refuse a non-directory upfront
/// (kqueue/Win32 do, inotify happens to accept single files but
/// cannot recursively enumerate), so `watch_run` adds an explicit
/// `is_dir()` check that maps to `WatchError::NotifyInit` → exit 4.
#[test]
fn ct_watch_non_directory_vault_root_exits_4() {
    // CURATED_VAULT_ROOT points at a regular file (not a directory).
    let brain_tmp = TempDir::new().unwrap();
    let fake_root_tmp = TempDir::new().unwrap();
    let fake_root = fake_root_tmp.path().join("not_a_dir.txt");
    std::fs::write(&fake_root, b"x").unwrap();
    let out = seed_brain_and_run(
        &brain_tmp,
        fake_root.to_str().unwrap(),
        &["watch", "--once", "--once-timeout", "200ms"],
    );

    assert_eq!(
        out.status.code(),
        Some(4),
        "non-directory vault root must exit 4 (notify init); stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}
