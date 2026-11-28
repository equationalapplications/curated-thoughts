//! Best-effort delivery audit rows in `curated_agent_log` (spec
//! 2026-11-28-wisdom-brief 3.7). Fail-open: nothing here changes a command's
//! stdout bytes or exit code. The log connection never creates the database,
//! waits at most 100 ms for a lock, and a failure prints at most one short
//! stderr line with a class from a closed vocabulary (never the SQLite
//! message, which can hold the db path). No `unwrap`/`expect` in this module.

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

/// Ids kept in one row's summary (OR-5).
pub const MAX_IDS: usize = 64;
/// A call that has already used this long skips logging (CTI timeout).
pub const SKIP_AFTER: Duration = Duration::from_millis(1500);
/// The closed error-class vocabulary.
pub const ERROR_CLASSES: [&str; 5] = ["open", "no_table", "busy", "readonly", "other"];

const CLIENT: &str = "ct-cli";

/// `{"v":1,"surface":..,"n":..,"ids":[..]}` plus `extra` keys. `n` is the full
/// count; `ids` is capped at [`MAX_IDS`]. Ids only, never query text or paths.
pub fn summary(surface: &str, ids: &[String], extra: Vec<(&str, Value)>) -> Value {
    let mut v = json!({
        "v": 1,
        "surface": surface,
        "n": ids.len(),
        "ids": ids.iter().take(MAX_IDS).collect::<Vec<_>>(),
    });
    for (k, val) in extra {
        v[k] = val;
    }
    v
}

/// True when the summary names at least one delivered id (`ids` or
/// `corrections`): no row is written when nothing was delivered.
fn delivered_something(summary: &Value) -> bool {
    ["ids", "corrections"].iter().any(|k| {
        summary
            .get(k)
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
    })
}

fn classify_sqlite(e: &rusqlite::Error) -> &'static str {
    use rusqlite::ErrorCode;
    match e {
        rusqlite::Error::SqliteFailure(f, msg) => match f.code {
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => "busy",
            ErrorCode::ReadOnly => "readonly",
            ErrorCode::CannotOpen => "open",
            _ => match msg.as_deref() {
                Some(m) if m.contains("no such table") || m.contains("no such column") => {
                    "no_table"
                }
                _ => "other",
            },
        },
        _ => "other",
    }
}

/// Class of a flattened error message (the commit helper wraps the SQLite
/// error in `anyhow`). Used only to pick a class, never printed.
fn classify_message(m: &str) -> &'static str {
    let m = m.to_ascii_lowercase();
    if m.contains("no such table") || m.contains("no such column") || m.contains("has no column") {
        "no_table"
    } else if m.contains("locked") || m.contains("busy") {
        "busy"
    } else if m.contains("readonly") || m.contains("read-only") {
        "readonly"
    } else {
        "other"
    }
}

fn skipped(class: &str) {
    let class = if ERROR_CLASSES.contains(&class) { class } else { "other" };
    eprintln!("wisdom_delivery_log_skipped\t{class}");
}

fn enabled() -> bool {
    std::env::var("CURATED_DELIVERY_LOG").map_or(true, |v| v != "0")
}

/// The fallible core: `Ok(())` when a row was written or nothing needed
/// writing, `Err(class)` with a class from [`ERROR_CLASSES`] otherwise.
fn try_record(
    db_path: &Path,
    tool: &str,
    entity_id: Option<&str>,
    summary: &Value,
) -> Result<(), &'static str> {
    if !enabled() || !delivered_something(summary) {
        return Ok(());
    }
    // READ_WRITE without CREATE: a missing db is an error, not a new file.
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| classify_sqlite(&e))?;
    conn.busy_timeout(Duration::from_millis(100))
        .map_err(|e| classify_sqlite(&e))?;
    // Pragmas are best-effort tuning; a failure here is not a delivery error.
    let _ = conn.pragma_update(None, "wal_autocheckpoint", 0);
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let text = summary.to_string();
    tauri_app_lib::db::commit::log_agent_access_checked_with_summary(
        &conn,
        CLIENT,
        tool,
        entity_id,
        "read",
        Some(&text),
    )
    .map_err(|e| classify_message(&format!("{e:#}")))
}

/// Append one delivery row. Never creates `db_path`, never panics, never
/// errors: at most one `wisdom_delivery_log_skipped\t<class>` stderr line.
pub fn record_delivery(db_path: &Path, tool: &str, entity_id: Option<&str>, summary: Value) {
    if let Err(class) = try_record(db_path, tool, entity_id, &summary) {
        skipped(class);
    }
}

/// The 1.5 s rule in one place: skip logging when the call (started at
/// `start`) has already used [`SKIP_AFTER`].
pub fn record_delivery_since(
    start: Instant,
    db_path: &Path,
    tool: &str,
    entity_id: Option<&str>,
    summary: Value,
) {
    if start.elapsed() >= SKIP_AFTER {
        return;
    }
    record_delivery(db_path, tool, entity_id, summary);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn scratch() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("brain.db");
        let c = Connection::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        c.execute_batch(
            "CREATE TABLE curated_agent_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT, client TEXT NOT NULL, tool TEXT NOT NULL,
                operation TEXT NOT NULL CHECK(operation IN ('read','write')),
                entity_id TEXT, summary TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch()));",
        )
        .unwrap();
        (dir, path)
    }

    fn rows(path: &Path) -> Vec<(String, String, String, Option<String>, String)> {
        let c = Connection::open(path).unwrap();
        let mut s = c
            .prepare("SELECT client, tool, operation, entity_id, summary FROM curated_agent_log ORDER BY id")
            .unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("f{i}")).collect()
    }

    #[test]
    fn writes_one_row_with_ids_only() {
        temp_env::with_var("CURATED_DELIVERY_LOG", None::<&str>, || {
            let (_d, p) = scratch();
            record_delivery(&p, "wisdom_brief", Some("hub"), summary("brief", &ids(2), vec![]));
            let r = rows(&p);
            assert_eq!(r.len(), 1);
            assert_eq!((r[0].0.as_str(), r[0].1.as_str(), r[0].2.as_str()), ("ct-cli", "wisdom_brief", "read"));
            assert_eq!(r[0].3.as_deref(), Some("hub"));
            let v: Value = serde_json::from_str(&r[0].4).unwrap();
            assert_eq!(v, json!({"v":1,"surface":"brief","n":2,"ids":["f0","f1"]}));
        });
    }

    #[test]
    fn no_row_when_no_ids() {
        temp_env::with_var("CURATED_DELIVERY_LOG", None::<&str>, || {
            let (_d, p) = scratch();
            record_delivery(&p, "wisdom_brief", None, summary("brief", &[], vec![]));
            assert!(rows(&p).is_empty());
            // Corrections alone count as a delivery.
            record_delivery(
                &p,
                "wisdom_match",
                None,
                summary("match", &[], vec![("corrections", json!(["c1"]))]),
            );
            assert_eq!(rows(&p).len(), 1);
        });
    }

    #[test]
    fn ids_capped_at_64() {
        let v = summary("brief", &ids(100), vec![]);
        assert_eq!(v["ids"].as_array().unwrap().len(), 64);
        assert_eq!(v["n"], 100);
    }

    #[test]
    fn missing_table_is_fail_open() {
        temp_env::with_var("CURATED_DELIVERY_LOG", None::<&str>, || {
            let d = TempDir::new().unwrap();
            let p = d.path().join("brain.db");
            Connection::open(&p).unwrap().execute_batch("CREATE TABLE x (a)").unwrap();
            let r = try_record(&p, "wisdom_brief", None, &summary("brief", &ids(1), vec![]));
            assert_eq!(r, Err("no_table"));
            // The log never creates its table or any other file.
            let c = Connection::open(&p).unwrap();
            let tables: Vec<String> = c
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(tables, vec!["x".to_string()]);
            let files = std::fs::read_dir(d.path()).unwrap().count();
            assert!(files <= 3, "only the db and its journal files: {files}");
        });
    }

    #[test]
    fn missing_db_does_not_create_file() {
        temp_env::with_var("CURATED_DELIVERY_LOG", None::<&str>, || {
            let d = TempDir::new().unwrap();
            let p = d.path().join("absent.db");
            record_delivery(&p, "wisdom_brief", None, summary("brief", &ids(1), vec![]));
            assert!(!p.exists());
        });
    }

    #[test]
    fn locked_db_fails_open_within_1s() {
        temp_env::with_var("CURATED_DELIVERY_LOG", None::<&str>, || {
            let (_d, p) = scratch();
            let holder = Connection::open(&p).unwrap();
            holder.execute_batch("BEGIN IMMEDIATE").unwrap();
            let t = Instant::now();
            record_delivery(&p, "wisdom_brief", None, summary("brief", &ids(1), vec![]));
            assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
            holder.execute_batch("ROLLBACK").unwrap();
            assert!(rows(&p).is_empty());
        });
    }

    #[cfg(unix)]
    #[test]
    fn readonly_file_fails_open() {
        temp_env::with_var("CURATED_DELIVERY_LOG", None::<&str>, || {
            use std::os::unix::fs::PermissionsExt;
            let (_d, p) = scratch();
            // Fold the WAL in so a read-only open needs no -shm write.
            Connection::open(&p).unwrap().pragma_update(None, "journal_mode", "DELETE").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
            // Root ignores file modes: skip when we can still open it for write.
            if std::fs::OpenOptions::new().write(true).open(&p).is_ok() {
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
                return;
            }
            let r = try_record(&p, "wisdom_brief", None, &summary("brief", &ids(1), vec![]));
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(r, Err("readonly"));
            assert!(rows(&p).is_empty());
        });
    }

    #[test]
    fn error_class_vocabulary_closed() {
        for m in ["no such table: t", "database is locked", "attempt to write a readonly database", "boom /some/path/brain.db"] {
            assert!(ERROR_CLASSES.contains(&classify_message(m)), "{m}");
        }
        assert_eq!(classify_message("boom /some/path/brain.db"), "other");
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("/secret/path".into()),
        );
        assert_eq!(classify_sqlite(&busy), "busy");
        // The printed class is clamped to the vocabulary.
        assert!(ERROR_CLASSES.contains(&"other"));
    }

    #[test]
    fn env_off_writes_nothing() {
        temp_env::with_var("CURATED_DELIVERY_LOG", Some("0"), || {
            let (_d, p) = scratch();
            record_delivery(&p, "wisdom_brief", None, summary("brief", &ids(1), vec![]));
            assert!(rows(&p).is_empty());
        });
    }

    #[test]
    fn summary_contains_no_query_or_path() {
        let v = summary(
            "match",
            &ids(1),
            vec![("target", json!("path")), ("corrections", json!(["c1"]))],
        );
        let keys: std::collections::BTreeSet<&str> =
            v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["corrections", "ids", "n", "surface", "target", "v"].into_iter().collect());
    }

    #[test]
    fn skip_after_1500ms_wrapper() {
        temp_env::with_var("CURATED_DELIVERY_LOG", None::<&str>, || {
            let (_d, p) = scratch();
            let s = || summary("brief", &ids(1), vec![]);
            if let Some(old) = Instant::now().checked_sub(Duration::from_secs(2)) {
                record_delivery_since(old, &p, "wisdom_brief", None, s());
                assert!(rows(&p).is_empty());
            }
            record_delivery_since(Instant::now(), &p, "wisdom_brief", None, s());
            assert_eq!(rows(&p).len(), 1);
        });
    }
}
