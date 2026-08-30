//! Privacy mode persistence and migration. See UX spec §6 and
//! `docs/superpowers/plans/2026-07-06-privacy-modes.md`.

mod enforce;

pub use enforce::{allows_cloud_bridge, allows_external_generation};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::cloud_bridge::pairing::PairingTokenStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyMode {
    Strict,
    Ephemeral,
    Connected,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PrivacyConfig {
    #[serde(default)]
    pub mode: Option<PrivacyMode>,
    #[serde(default)]
    pub chosen: bool,
    #[serde(default)]
    pub ephemeral_disclosure_acknowledged: bool,
    #[serde(default)]
    pub migration_disclosure_acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrivacyState {
    pub mode: PrivacyMode,
    pub chosen: bool,
    pub needs_migration_disclosure: bool,
    pub ephemeral_disclosure_acknowledged: bool,
}

pub fn effective_mode(cfg: &PrivacyConfig) -> PrivacyMode {
    cfg.mode.unwrap_or(PrivacyMode::Strict)
}

pub fn read_privacy_config(_brain_dir: &Path) -> Result<PrivacyConfig> {
    let paths = crate::retrieval::resolve_brain_paths();
    let report = crate::config::BrainConfig::load_lenient(&paths);
    Ok(report.config.privacy)
}

pub fn write_privacy_config(_brain_dir: &Path, privacy: &PrivacyConfig) -> Result<()> {
    let paths = crate::retrieval::resolve_brain_paths();
    let mut config = crate::config::BrainConfig::load_lenient(&paths).config;
    config.privacy = privacy.clone();
    config.write(&paths)
}

pub fn write_privacy_mode(brain_dir: &Path, mode: PrivacyMode, chosen: bool) -> Result<()> {
    let mut cfg = read_privacy_config(brain_dir)?;
    cfg.mode = Some(mode);
    cfg.chosen = chosen;
    write_privacy_config(brain_dir, &cfg)
}

pub fn acknowledge_migration_disclosure(brain_dir: &Path) -> Result<()> {
    let mut cfg = read_privacy_config(brain_dir)?;
    cfg.migration_disclosure_acknowledged = true;
    write_privacy_config(brain_dir, &cfg)
}

pub fn acknowledge_ephemeral_disclosure(brain_dir: &Path) -> Result<()> {
    let mut cfg = read_privacy_config(brain_dir)?;
    cfg.ephemeral_disclosure_acknowledged = true;
    write_privacy_config(brain_dir, &cfg)
}

/// Returns whether the cloud bridge is permitted to run (privacy + token + WS URL).
pub fn cloud_bridge_permitted(
    brain_dir: &Path,
    token_store: &dyn PairingTokenStore,
) -> Result<bool> {
    let state = resolve_privacy_state(brain_dir, token_store)?;
    if !allows_cloud_bridge(state.mode) {
        return Ok(false);
    }
    Ok(token_store.get()?.is_some())
}

/// Persists a user-chosen mode. When leaving `Connected`, clears the pairing token.
/// Returns `(new_state, disconnected_bridge)`.
pub fn set_privacy_mode_config(
    brain_dir: &Path,
    mode: PrivacyMode,
    token_store: &dyn PairingTokenStore,
) -> Result<(PrivacyState, bool)> {
    let current = resolve_privacy_state(brain_dir, token_store)?;
    let mut disconnected_bridge = false;
    if current.mode == PrivacyMode::Connected && mode != PrivacyMode::Connected {
        if token_store.get()?.is_some() {
            token_store.delete()?;
            disconnected_bridge = true;
        }
    }
    let mut cfg = read_privacy_config(brain_dir)?;
    cfg.mode = Some(mode);
    cfg.chosen = true;
    write_privacy_config(brain_dir, &cfg)?;
    let state = resolve_privacy_state(brain_dir, token_store)?;
    Ok((state, disconnected_bridge))
}

pub fn resolve_privacy_state(
    brain_dir: &Path,
    token_store: &dyn PairingTokenStore,
) -> Result<PrivacyState> {
    let mut cfg = read_privacy_config(brain_dir)?;
    if !cfg.chosen {
        if token_store.get()?.is_some() {
            cfg.mode = Some(PrivacyMode::Connected);
            cfg.chosen = true;
            write_privacy_config(brain_dir, &cfg)?;
            return Ok(PrivacyState {
                mode: PrivacyMode::Connected,
                chosen: true,
                needs_migration_disclosure: !cfg.migration_disclosure_acknowledged,
                ephemeral_disclosure_acknowledged: cfg.ephemeral_disclosure_acknowledged,
            });
        }
        if cfg.mode.is_none() {
            cfg.mode = Some(PrivacyMode::Strict);
            write_privacy_config(brain_dir, &cfg)?;
        }
    }
    Ok(PrivacyState {
        mode: effective_mode(&cfg),
        chosen: cfg.chosen,
        needs_migration_disclosure: false,
        ephemeral_disclosure_acknowledged: cfg.ephemeral_disclosure_acknowledged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Privacy reads/writes now route through `crate::retrieval::resolve_brain_paths()`,
    // which resolves the config path from `CURATED_BRAIN_CONFIG` / `CURATED_BRAIN_DB` /
    // `CURATED_BRAIN_DIR`. Tests must pin one of these so the operations land in the
    // tempdir, not in the developer's home directory. `temp_env::with_var` restores the
    // previous value on drop, but unit tests run in parallel — serialize them with a
    // static mutex to prevent one test from observing another test's env mutation.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Pin `CURATED_BRAIN_CONFIG` at `<tmp>/config.json` and run `body`. Restores the
    /// prior env var value (or unsets it) on return.
    fn run_with_config<F: FnOnce()>(tmp: &TempDir, body: F) {
        let _guard = ENV_LOCK.lock().unwrap();
        let config_path = tmp.path().join("config.json");
        let config_path_str = config_path.to_str().expect("tempdir path is UTF-8").to_string();
        temp_env::with_var(
            "CURATED_BRAIN_CONFIG",
            Some(config_path_str),
            || {
                body();
            },
        );
    }

    struct NoTokenStore;

    impl PairingTokenStore for NoTokenStore {
        fn get(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn set(&self, _token: &str) -> Result<()> {
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            Ok(())
        }
    }

    struct TokenStore(&'static str);

    impl PairingTokenStore for TokenStore {
        fn get(&self) -> Result<Option<String>> {
            Ok(Some(self.0.to_string()))
        }
        fn set(&self, _token: &str) -> Result<()> {
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn absent_privacy_defaults_strict_without_token() {
        let dir = TempDir::new().unwrap();
        run_with_config(&dir, || {
            let resolved = resolve_privacy_state(dir.path(), &NoTokenStore).unwrap();
            assert_eq!(resolved.mode, PrivacyMode::Strict);
            assert!(!resolved.needs_migration_disclosure);
            assert_eq!(
                read_privacy_config(dir.path()).unwrap().mode,
                Some(PrivacyMode::Strict)
            );
        });
    }

    #[test]
    fn absent_privacy_with_token_migrates_to_connected() {
        let dir = TempDir::new().unwrap();
        run_with_config(&dir, || {
            let resolved = resolve_privacy_state(dir.path(), &TokenStore("tok")).unwrap();
            assert_eq!(resolved.mode, PrivacyMode::Connected);
            assert!(resolved.needs_migration_disclosure);
            assert_eq!(
                read_privacy_config(dir.path()).unwrap().mode,
                Some(PrivacyMode::Connected)
            );
        });
    }

    #[test]
    fn explicit_strict_not_overridden_by_token() {
        let dir = TempDir::new().unwrap();
        run_with_config(&dir, || {
            write_privacy_mode(dir.path(), PrivacyMode::Strict, true).unwrap();
            let resolved = resolve_privacy_state(dir.path(), &TokenStore("tok")).unwrap();
            assert_eq!(resolved.mode, PrivacyMode::Strict);
            assert!(!resolved.needs_migration_disclosure);
        });
    }

    #[test]
    fn write_privacy_config_preserves_other_config_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"vault_path":"/tmp/vault"}"#).unwrap();
        run_with_config(&dir, || {
            write_privacy_mode(dir.path(), PrivacyMode::Ephemeral, true).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            let root: serde_json::Value = serde_json::from_str(&contents).unwrap();
            assert_eq!(root["vault_path"], "/tmp/vault");
            assert_eq!(root["privacy"]["mode"], "ephemeral");
        });
    }

    #[test]
    fn migration_disclosure_suppressed_after_acknowledged() {
        let dir = TempDir::new().unwrap();
        run_with_config(&dir, || {
            let mut cfg = PrivacyConfig::default();
            cfg.migration_disclosure_acknowledged = true;
            write_privacy_config(dir.path(), &cfg).unwrap();
            let resolved = resolve_privacy_state(dir.path(), &TokenStore("tok")).unwrap();
            assert!(!resolved.needs_migration_disclosure);
        });
    }

    #[test]
    fn cloud_bridge_not_permitted_in_strict_even_with_token() {
        let dir = TempDir::new().unwrap();
        run_with_config(&dir, || {
            write_privacy_mode(dir.path(), PrivacyMode::Strict, true).unwrap();
            assert!(!cloud_bridge_permitted(dir.path(), &TokenStore("tok")).unwrap());
        });
    }

    #[test]
    fn cloud_bridge_permitted_in_connected_with_token() {
        let dir = TempDir::new().unwrap();
        run_with_config(&dir, || {
            write_privacy_mode(dir.path(), PrivacyMode::Connected, true).unwrap();
            assert!(cloud_bridge_permitted(dir.path(), &TokenStore("tok")).unwrap());
        });
    }

    #[test]
    fn downgrade_from_connected_clears_token() {
        use std::sync::Mutex;

        let dir = TempDir::new().unwrap();
        struct MutableTokenStore(Mutex<Option<String>>);
        impl PairingTokenStore for MutableTokenStore {
            fn get(&self) -> Result<Option<String>> {
                Ok(self.0.lock().unwrap().clone())
            }
            fn set(&self, token: &str) -> Result<()> {
                *self.0.lock().unwrap() = Some(token.to_string());
                Ok(())
            }
            fn delete(&self) -> Result<()> {
                *self.0.lock().unwrap() = None;
                Ok(())
            }
        }
        let store = MutableTokenStore(Mutex::new(Some("tok".into())));
        run_with_config(&dir, || {
            write_privacy_mode(dir.path(), PrivacyMode::Connected, true).unwrap();
            let (_, disconnected) =
                set_privacy_mode_config(dir.path(), PrivacyMode::Strict, &store).unwrap();
            assert!(disconnected);
            assert!(store.get().unwrap().is_none());
        });
    }
}
