//! `dayseam-secrets` — everywhere we hold an authentication token, we hold
//! it through this crate. The job of the crate is to make three things
//! hard to get wrong:
//!
//!   1. A secret must never end up in `Debug` output, panics, or a
//!      serialized form. `Secret<T>` enforces that at the type level.
//!   2. There is exactly one way to read a secret back out —
//!      [`Secret::expose_secret`] — which is deliberately verbose and
//!      easy to `rg` for during audits.
//!   3. Secrets live in the OS keychain in production and a process-local
//!      store in tests, behind a common [`SecretStore`] trait so
//!      connectors never care which is in play.
//!
//! `dayseam-core::SecretRef` stays a plain, serialisable handle — this
//! crate is what you hand a `SecretRef` to when you actually need the
//! bytes.

pub mod error;
pub mod memory;
pub mod oauth;
pub mod secret;
pub mod store;

#[cfg(feature = "keychain")]
pub mod keychain;

pub use error::{SecretError, SecretResult};
pub use memory::InMemoryStore;
pub use secret::Secret;
pub use store::SecretStore;

#[cfg(feature = "keychain")]
pub use keychain::KeychainStore;
