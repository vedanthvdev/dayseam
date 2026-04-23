//! IPC-boundary wrapper for secret strings.
//!
//! The PAT the user pastes into `AddGitlabSourceDialog` has to cross
//! the Tauri IPC boundary exactly once — from the renderer into
//! [`super::commands::gitlab_validate_pat`] — and then never again.
//! Once it's on the Rust side it moves straight into
//! [`connector_gitlab::auth::validate_pat`], which sends it as the
//! `PRIVATE-TOKEN` header and drops it.
//!
//! To make that one-hop journey safe we need a wrapper that:
//!
//! 1. [`serde::Deserialize`]s from a plain JSON string (Tauri's
//!    `invoke` payload is JSON) so the command signature can accept it
//!    directly from the IPC layer.
//! 2. Does **not** implement [`serde::Serialize`] — no command
//!    returning a `IpcSecretString` can ever exist, so a PAT cannot
//!    round-trip back to the renderer even by mistake.
//! 3. Redacts its value in [`std::fmt::Debug`] so any `tracing`
//!    breadcrumb that captures the command's args via `#[instrument]`
//!    still prints `IpcSecretString(***)` rather than the token.
//! 4. Zeroes the backing `String`'s bytes on drop so a cancelled
//!    dialog — or a command that returns `Err(...)` before handing the
//!    secret to the validator — does not leave the PAT sitting in the
//!    freed allocation until whatever writes over it next.
//!
//! The type is intentionally narrow: its only reader is
//! [`IpcSecretString::expose`], which returns a borrowed `&str` that
//! lives only as long as the call site holds it. Callers should pass
//! that `&str` directly into the first HTTP client method that needs
//! it, never copy it into another `String`.
//!
//! This is the *only* place in the desktop crate a PAT-shaped value
//! crosses the IPC boundary; keeping the wrapper here — rather than in
//! `dayseam-core` or `dayseam-secrets` — is deliberate. `dayseam-core`
//! types derive `TS` + `Serialize`, both of which would be a mistake
//! here; `dayseam-secrets`'s [`dayseam_secrets::Secret`] does the
//! right thing post-IPC (long-lived Keychain round-trip) but does not
//! implement `Deserialize` for exactly the symmetric reason.

use std::fmt;

use serde::Deserialize;
use zeroize::Zeroize;

/// Opaque wrapper around a `String` received over Tauri IPC. See the
/// module-level docstring for the four invariants this type preserves.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct IpcSecretString(String);

impl IpcSecretString {
    /// Construct directly. Used only in tests and in call sites that
    /// have already deserialised the payload by some other means; the
    /// production path is [`serde::Deserialize`] via Tauri's `invoke`
    /// reactor.
    ///
    /// DAY-111 widens the gate to `cfg(any(test, feature =
    /// "test-helpers"))` so the `tests/reconnect_rebind.rs`
    /// integration suite (which lives *outside* `#[cfg(test)]` from
    /// this crate's perspective — integration tests compile as
    /// separate crates) can build PATs without round-tripping
    /// through `serde_json`. The constructor remains invisible to
    /// release binaries because the `test-helpers` feature is never
    /// enabled in a release build.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn new(inner: impl Into<String>) -> Self {
        Self(inner.into())
    }

    /// The single reader. Returns a borrowed `&str` that lives only as
    /// long as `self`; callers should forward it straight into the
    /// HTTP client and never copy it into a fresh `String`.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

// Manual `Debug` so `tracing` spans / `{:?}` formatting never spill
// the PAT. The `Deserialize` derive above does not generate a `Debug`
// impl on its own; the manual impl here is the belt-and-suspenders.
impl fmt::Debug for IpcSecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IpcSecretString").field(&"***").finish()
    }
}

// Zero the backing bytes on drop so the PAT does not linger in the
// freed allocation. `zeroize::Zeroize for String` is provided by the
// `alloc` default feature and is the exact semantic we want here.
impl Drop for IpcSecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_is_redacted() {
        let pat = IpcSecretString::new("glpat-super-secret-token-123");
        let rendered = format!("{pat:?}");
        assert!(
            rendered.contains("***"),
            "debug output must show redacted marker; got: {rendered}"
        );
        assert!(
            !rendered.contains("super-secret-token"),
            "debug output must not contain raw token bytes; got: {rendered}"
        );
    }

    #[test]
    fn deserializes_from_plain_json_string() {
        let pat: IpcSecretString =
            serde_json::from_str(r#""glpat-xyz""#).expect("deserialize plain string");
        assert_eq!(pat.expose(), "glpat-xyz");
    }

    /// Drop zeroes the inner String. We cannot observe the bytes of a
    /// freed allocation from safe Rust, but we *can* exercise
    /// `Zeroize::zeroize` directly on the inner `String` and confirm
    /// it empties it — which is the operation `Drop` delegates to.
    #[test]
    fn zeroize_empties_inner() {
        let mut s = String::from("glpat-secret");
        s.zeroize();
        assert!(s.is_empty(), "zeroize should empty the inner String");
    }
}
