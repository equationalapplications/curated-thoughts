use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{path::PathBuf, sync::mpsc, thread};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "path")]
pub enum VaultEvent {
    Added(String),
    Modified(String),
    Deleted(String),
}

pub fn start_watcher<F>(vault_path: PathBuf, callback: F) -> Result<()>
where
    F: Fn(VaultEvent) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&vault_path, RecursiveMode::Recursive)?;

    thread::spawn(move || {
        let _keep = watcher;
        for result in rx {
            let Ok(event) = result else { continue };
            for path in event.paths {
                let path_str = path.to_string_lossy().to_string();
                let vault_event = match event.kind {
                    EventKind::Create(_) => VaultEvent::Added(path_str),
                    EventKind::Modify(_) => VaultEvent::Modified(path_str),
                    EventKind::Remove(_) => VaultEvent::Deleted(path_str),
                    _ => continue,
                };
                callback(vault_event);
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::mpsc, time::Duration};
    use tempfile::TempDir;

    #[test]
    fn test_watcher_detects_new_file() {
        let tmp = TempDir::new().unwrap();
        let (tx, rx) = mpsc::channel::<VaultEvent>();
        start_watcher(tmp.path().to_path_buf(), move |e| {
            tx.send(e).ok();
        })
        .unwrap();

        fs::write(tmp.path().join("note.md"), "hello").unwrap();

        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no event received");
        assert!(matches!(event, VaultEvent::Added(_)));
    }

    #[test]
    fn test_watcher_detects_deleted_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("note.md");
        fs::write(&path, "hello").unwrap();

        let (tx, rx) = mpsc::channel::<VaultEvent>();
        start_watcher(tmp.path().to_path_buf(), move |e| {
            tx.send(e).ok();
        })
        .unwrap();

        std::thread::sleep(Duration::from_millis(200));
        fs::remove_file(&path).unwrap();

        // Drain events until Deleted is found — macOS FSEvents may emit spurious
        // Modify events before the Remove event arrives.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.duration_since(std::time::Instant::now());
            if let Ok(event) = rx.recv_timeout(remaining) {
                if matches!(event, VaultEvent::Deleted(_)) {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "no Deleted event received within timeout");
    }
}
