//! Pairing-token storage. The token grants query-only access to the five read-only tools
//! (§6 of the design spec) and must never land in `brain.db` or a plaintext config file —
//! it lives in the OS keychain via the `keyring` crate.

use anyhow::Result;

pub trait PairingTokenStore: Send + Sync + 'static {
    fn get(&self) -> Result<Option<String>>;
    fn set(&self, token: &str) -> Result<()>;
    fn delete(&self) -> Result<()>;
}

const SERVICE: &str = "curated-thoughts-cloud-bridge";
const ACCOUNT: &str = "clanker-pairing-token";

pub struct KeyringPairingTokenStore;

impl PairingTokenStore for KeyringPairingTokenStore {
    fn get(&self) -> Result<Option<String>> {
        let entry = keyring::Entry::new(SERVICE, ACCOUNT)?;
        match entry.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, token: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, ACCOUNT)?;
        entry.set_password(token)?;
        Ok(())
    }

    fn delete(&self) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, ACCOUNT)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct InMemoryPairingTokenStore(Mutex<Option<String>>);

    impl PairingTokenStore for InMemoryPairingTokenStore {
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

    #[test]
    fn get_returns_none_before_any_set() {
        let store = InMemoryPairingTokenStore::default();
        assert_eq!(store.get().unwrap(), None);
    }

    #[test]
    fn set_then_get_round_trips() {
        let store = InMemoryPairingTokenStore::default();
        store.set("token-123").unwrap();
        assert_eq!(store.get().unwrap(), Some("token-123".to_string()));
    }

    #[test]
    fn delete_clears_a_set_token() {
        let store = InMemoryPairingTokenStore::default();
        store.set("token-123").unwrap();
        store.delete().unwrap();
        assert_eq!(store.get().unwrap(), None);
    }

    #[test]
    fn delete_before_any_set_is_a_noop() {
        let store = InMemoryPairingTokenStore::default();
        assert!(store.delete().is_ok());
    }

    #[test]
    fn set_overwrites_previous_token() {
        let store = InMemoryPairingTokenStore::default();
        store.set("first").unwrap();
        store.set("second").unwrap();
        assert_eq!(store.get().unwrap(), Some("second".to_string()));
    }
}
