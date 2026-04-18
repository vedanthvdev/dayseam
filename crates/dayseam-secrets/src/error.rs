//! Error taxonomy for secret stores. Kept separate from
//! `dayseam-core::DayseamError` so backends can surface native error
//! shapes; callers map into `DayseamError::Auth` / `Internal` at the
//! public boundary.

use thiserror::Error;

pub type SecretResult<T> = Result<T, SecretError>;

#[derive(Debug, Error)]
pub enum SecretError {
    /// The underlying platform store refused the operation (permission
    /// denied, locked keychain, disk full, …). Carries a human-readable
    /// message; we deliberately keep the shape opaque so we don't leak
    /// platform-specific detail into `DayseamError`.
    #[error("secret store backend failed: {message}")]
    Backend { message: String },

    /// The value stored under the key wasn't valid UTF-8. In practice
    /// this only happens when something other than Dayseam wrote to the
    /// same keychain entry; still worth surfacing distinctly so we don't
    /// blame ourselves on a corruption.
    #[error("secret is not valid UTF-8")]
    NotUtf8,
}

#[cfg(feature = "keychain")]
impl From<keyring::Error> for SecretError {
    fn from(err: keyring::Error) -> Self {
        SecretError::Backend {
            message: err.to_string(),
        }
    }
}
