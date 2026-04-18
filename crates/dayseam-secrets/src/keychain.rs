//! macOS Keychain backend, powered by the `keyring` crate. The Keychain
//! addresses entries by a `(service, account)` pair; callers of this
//! crate give us a single composite key in the form `"service::account"`
//! and we split it on the first `"::"`. Keeping the separator explicit
//! means we can't silently swap service and account on callers that use
//! colons in either half.
//!
//! The backend is feature-gated (`--features keychain`) so CI runners
//! and Linux dev machines can build and test the rest of the crate
//! without pulling in the platform-specific dependency tree.

use keyring::Entry;

use crate::{Secret, SecretError, SecretResult, SecretStore};

/// Live Keychain-backed [`SecretStore`]. One instance per process is
/// plenty — `Entry` values are cheap to construct on demand.
pub struct KeychainStore;

impl KeychainStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(key: &str) -> SecretResult<Entry> {
        let (service, account) = split_key(key)?;
        Entry::new(service, account).map_err(SecretError::from)
    }
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for KeychainStore {
    fn put(&self, key: &str, value: Secret<String>) -> SecretResult<()> {
        let entry = Self::entry(key)?;
        let raw = value.into_inner();
        let res = entry.set_password(&raw);
        // Zero the raw copy we handed to `keyring`; the Keychain itself
        // now holds an independent copy.
        let mut raw = raw;
        use zeroize::Zeroize;
        raw.zeroize();
        res.map_err(SecretError::from)
    }

    fn get(&self, key: &str) -> SecretResult<Option<Secret<String>>> {
        let entry = Self::entry(key)?;
        match entry.get_password() {
            Ok(pw) => Ok(Some(Secret::new(pw))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::from(e)),
        }
    }

    fn delete(&self, key: &str) -> SecretResult<()> {
        let entry = Self::entry(key)?;
        match entry.delete_password() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::from(e)),
        }
    }
}

/// Split `"service::account"` into its parts. Rejects keys without a
/// separator so callers can't accidentally store everything under one
/// blob that's painful to audit in Keychain Access later.
fn split_key(key: &str) -> SecretResult<(&str, &str)> {
    match key.split_once("::") {
        Some((service, account)) if !service.is_empty() && !account.is_empty() => {
            Ok((service, account))
        }
        _ => Err(SecretError::Backend {
            message: format!(
                "keychain key must be `service::account` with non-empty halves, got `{key}`"
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_key_happy_path() {
        assert_eq!(
            split_key("app.dayseam.desktop::gitlab:42").unwrap(),
            ("app.dayseam.desktop", "gitlab:42"),
        );
    }

    #[test]
    fn split_key_rejects_missing_separator() {
        assert!(split_key("lonely-key").is_err());
    }

    #[test]
    fn split_key_rejects_empty_halves() {
        assert!(split_key("::account").is_err());
        assert!(split_key("service::").is_err());
    }
}
