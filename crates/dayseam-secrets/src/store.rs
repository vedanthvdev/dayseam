//! The one trait every secret backend implements. Deliberately narrow:
//! three methods, all keyed by a single string, each returning a typed
//! `Secret<String>`.
//!
//! Splitting `service` from `account` (as macOS Keychain does) is the
//! *backend's* concern — the trait's string key is whatever composite
//! shape the backend prefers. `KeychainStore` parses `"service::account"`;
//! `InMemoryStore` stores the raw string.

use crate::{Secret, SecretResult};

/// A process-level secret store. `Send + Sync` so connectors can own a
/// shared reference across tasks without wrapping in yet another mutex.
pub trait SecretStore: Send + Sync {
    /// Write `value` under `key`, overwriting any existing entry.
    fn put(&self, key: &str, value: Secret<String>) -> SecretResult<()>;

    /// Read the value stored under `key`. Returns `Ok(None)` if the key
    /// is absent — that's the single "not present" signal callers need.
    fn get(&self, key: &str) -> SecretResult<Option<Secret<String>>>;

    /// Remove `key`. Idempotent: deleting an absent key returns `Ok(())`.
    fn delete(&self, key: &str) -> SecretResult<()>;
}
