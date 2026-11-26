//! Switch-level pin tests (#376, spec `engine9x-followups` §2 S5): the
//! post-sweep reopen must resolve the vault root from a config that already
//! names the NEW vault. Drives the same seam (`switch_reopen_paths`) the
//! `switch_vault` command uses, against real temp vaults — no Tauri State.

use crate::db::{stated_ledger, AppDb};
use crate::vault::config::VaultConfig;
use std::path::{Path, PathBuf};

const STAMP_SQL: &str = "SELECT value FROM llm_wiki_meta WHERE key='stated_ledger_rederived'";

/// A vault dir with the stated/ ledgers and one attributed deposit note whose
/// file name carries `name`, so derived rows are attributable to a vault.
fn make_vault(root: &Path, name: &str) -> PathBuf {
    let stated = root.join("stated");
    std::fs::create_dir_all(&stated).unwrap();
    std::fs::write(
        stated.join(".confirmations.jsonl"),
        format!("{{\"fact_id\":\"f_1\",\"content_hash\":\"h_{name}\",\"by\":\"kurt\",\"at\":\"t\"}}\n"),
    )
    .unwrap();
    std::fs::write(stated.join(".attestations.jsonl"), "").unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes").join(format!("deposit-{name}.md")),
        "---\nattributed_speaker: kurt\nattribution_kind: verbatim\nattribution_source: discord:1\n---\n\n# T\n\nbody\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".brain")).unwrap();
    root.to_path_buf()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    b: PathBuf,
    db_path: PathBuf,
    config_path: PathBuf,
}

/// Vaults A and B; config names A; the brain at A's `.brain/brain.db` has
/// been opened (stamp set, A's rows derived) and closed.
fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let a = make_vault(&tmp.path().join("A"), "a");
    let b = make_vault(&tmp.path().join("B"), "b");
    let config_path = tmp.path().join("config.json");
    VaultConfig::new(config_path.clone())
        .set_vault_path(a.to_str().unwrap())
        .unwrap();
    let db_path = a.join(".brain").join("brain.db");
    let db = crate::switch_reopen_paths(&db_path, &config_path).unwrap();
    stated_ledger::rebuild_rederive(&db.0, &a.canonicalize().unwrap()).unwrap();
    drop(db);
    Fixture { _tmp: tmp, b, db_path, config_path }
}

fn stamp(db: &AppDb) -> Option<String> {
    db.0.query_row(STAMP_SQL, [], |r| r.get(0)).ok()
}

fn deposit_paths(db: &AppDb) -> Vec<String> {
    let mut stmt = db.0.prepare("SELECT document_path FROM deposit_attribution").unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn sweep(db_path: &Path) {
    let mut conn = rusqlite::Connection::open(db_path).unwrap();
    crate::db::clear_vault_tables(&mut conn, 1).unwrap();
}

/// Sweep, then drop the rederived stamp so the OPEN itself re-derives from the
/// root it resolves out of the config (the stamp otherwise survives a sweep and
/// the open would skip the walk). No root is passed by hand: whatever the
/// reopen's derived rows say is what the open resolved from the config.
fn sweep_and_unstamp(db_path: &Path) {
    sweep(db_path);
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute("DELETE FROM llm_wiki_meta WHERE key='stated_ledger_rederived'", [])
        .unwrap();
}

#[test]
fn old_order_leaks_outgoing_vault_rows() {
    // OLD order: config still names A at reopen, no rewrite before it.
    let f = fixture();
    sweep_and_unstamp(&f.db_path);
    let db = crate::switch_reopen_paths(&f.db_path, &f.config_path).unwrap();
    // The open resolved A from the config, so it derived A's rows.
    let paths = deposit_paths(&db);
    assert!(
        paths.iter().any(|p| p.contains("deposit-a.md")),
        "old order leaks A's rows into the switched brain: {paths:?}"
    );
    assert!(!paths.iter().any(|p| p.contains("deposit-b.md")));
}

#[test]
fn inplace_switch_rederives_from_new_vault_via_open_rederive() {
    let f = fixture();
    let before = stamp(&crate::switch_reopen_paths(&f.db_path, &f.config_path).unwrap());
    sweep_and_unstamp(&f.db_path);
    // NEW order: config rewritten BEFORE the reopen.
    VaultConfig::new(f.config_path.clone())
        .set_vault_path(f.b.to_str().unwrap())
        .unwrap();
    // The OPEN resolves B from the config and re-derives from it; no root is
    // passed by hand, so swapping the rewrite and the reopen fails this test.
    let db = crate::switch_reopen_paths(&f.db_path, &f.config_path).unwrap();

    let paths = deposit_paths(&db);
    assert!(paths.iter().any(|p| p.contains("deposit-b.md")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("deposit-a.md")), "{paths:?}");
    for table in ["stated_attestations", "queue_denials"] {
        let n: i64 = db
            .0
            .query_row(&format!("SELECT COUNT(*) FROM {table} WHERE document_path LIKE '%deposit-a%'"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table} holds an A-derived row");
    }
    // Stamp survival of the sweep is proven by
    // `clear_vault_tables_empties_stated_ledger_and_kick_state_but_keeps_stamp`
    // in queries.rs; here the stamp was cleared, so only the reopen's
    // re-derive is under test.
    assert!(before.is_some());
}

/// Moved-root restore: the installed backup is A's brain (stamped, A-rooted
/// keys) placed under B, config names B. The open skips the walk (stamp set).
fn installed_backup_at_b(f: &Fixture) -> AppDb {
    let b_db = f.b.join(".brain").join("brain.db");
    std::fs::copy(&f.db_path, &b_db).unwrap();
    VaultConfig::new(f.config_path.clone())
        .set_vault_path(f.b.to_str().unwrap())
        .unwrap();
    crate::switch_reopen_paths(&b_db, &f.config_path).unwrap()
}

#[test]
fn moved_root_restore_rederives_keys_from_new_root() {
    let f = fixture();
    {
        let conn = rusqlite::Connection::open(&f.db_path).unwrap();
        let p = f.b.parent().unwrap().join("A").join("notes").join("deposit-a.md");
        conn.execute(
            "INSERT OR REPLACE INTO stated_attestations VALUES (?1,'h','kurt','t')",
            [p.to_str().unwrap()],
        )
        .unwrap();
    }
    let db = installed_backup_at_b(&f);
    assert!(stamp(&db).is_some(), "backup carries the stamp; open skips the walk");
    crate::restore_rederive_with(&db.0, &f.b.canonicalize().unwrap(), |c, r| {
        stated_ledger::rebuild_rederive(c, r)
    })
    .unwrap();
    let paths = deposit_paths(&db);
    assert!(paths.iter().any(|p| p.contains("deposit-b.md")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("deposit-a.md")), "{paths:?}");
    let n: i64 = db
        .0
        .query_row("SELECT COUNT(*) FROM stated_attestations WHERE document_path LIKE '%/A/%'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "stated_attestations holds an A-rooted key");
}

#[test]
fn restore_rederive_failure_unstamps_and_open_gate_retries() {
    let f = fixture();
    let b_db = f.b.join(".brain").join("brain.db");
    let db = installed_backup_at_b(&f);
    assert!(stamp(&db).is_some());
    let r = crate::restore_rederive_with(&db.0, &f.b.canonicalize().unwrap(), |_, _| {
        anyhow::bail!("injected")
    });
    assert!(r.is_err());
    assert!(stamp(&db).is_none(), "failure must delete the stamp");
    drop(db);
    let db = crate::switch_reopen_paths(&b_db, &f.config_path).unwrap();
    let paths = deposit_paths(&db);
    assert!(paths.iter().any(|p| p.contains("deposit-b.md")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("deposit-a.md")), "{paths:?}");
}

#[test]
fn restore_branch_config_before_reopen_rederives_from_new_vault() {
    // Scope: the restore branch's ordering essence — production does NOT sweep
    // or run a hand Run 1 here (`swept` is false); it only rewrites the config
    // before the reopen, so the open resolves B. This is therefore the in-place
    // pin minus the sweep: the installed file is a brain whose stamp is absent
    // (a restored backup), and the open must derive from B alone. The
    // staged-backup install itself is covered by restore_sync tests.
    let f = fixture();
    {
        let conn = rusqlite::Connection::open(&f.db_path).unwrap();
        conn.execute("DELETE FROM llm_wiki_meta WHERE key='stated_ledger_rederived'", [])
            .unwrap();
        conn.execute("DELETE FROM deposit_attribution", []).unwrap();
    }
    VaultConfig::new(f.config_path.clone())
        .set_vault_path(f.b.to_str().unwrap())
        .unwrap();
    let db = crate::switch_reopen_paths(&f.db_path, &f.config_path).unwrap();
    let paths = deposit_paths(&db);
    assert!(!paths.iter().any(|p| p.contains("deposit-a.md")), "{paths:?}");
    assert!(paths.iter().any(|p| p.contains("deposit-b.md")), "{paths:?}");
}
