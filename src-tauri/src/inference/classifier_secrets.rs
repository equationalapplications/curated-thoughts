//! OS-keychain storage for the classifier API key (spec CT-REQ-CLASS-01 §5.2).
//!
//! The classifier's API token is a credential, not a setting: it must never be
//! persisted in `config.json` (CodeRabbit review for the 7.7.4 adoption PR).
//! Reads and writes go through [`ClassifierSecretStore`] so tests can substitute
//! an in-memory store; production wiring uses [`KeyringClassifierSecretStore`]
//! against `keyring::Entry`.

use anyhow::Result;

pub trait ClassifierSecretStore: Send + Sync + 'static {
    fn get(&self) -> Result<Option<String>>;
    fn set(&self, secret: &str) -> Result<()>;
    fn delete(&self) -> Result<()>;
}

const SERVICE: &str = "curated-thoughts-classifier";
const ACCOUNT: &str = "api-key";

pub struct KeyringClassifierSecretStore;

impl ClassifierSecretStore for KeyringClassifierSecretStore {
    fn get(&self) -> Result<Option<String>> {
        let entry = keyring::Entry::new(SERVICE, ACCOUNT)?;
        match entry.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, secret: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, ACCOUNT)?;
        entry.set_password(secret)?;
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
pub struct InMemoryClassifierSecretStore(pub std::sync::Mutex<Option<String>>);

#[cfg(test)]
impl ClassifierSecretStore for InMemoryClassifierSecretStore {
    fn get(&self) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn set(&self, secret: &str) -> Result<()> {
        *self.0.lock().unwrap() = Some(secret.to_string());
        Ok(())
    }

    fn delete(&self) -> Result<()> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}
