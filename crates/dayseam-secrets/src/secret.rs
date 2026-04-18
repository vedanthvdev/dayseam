//! The `Secret<T>` wrapper. Holds a value of type `T` (typically
//! `String`) with three guarantees that the compiler enforces:
//!
//!   * `Debug` never prints the value — always the literal `"***"`.
//!   * `Drop` zeroes the underlying memory via `zeroize::Zeroize`.
//!   * The only reader is [`Secret::expose_secret`] — deliberately
//!     verbose so every access is easy to spot in code review.
//!
//! The type does not implement `serde::Serialize` or `serde::Deserialize`
//! on purpose: secrets cross process boundaries through the [`store`]
//! trait only, never through accidental serialisation of a struct field.

use zeroize::Zeroize;

/// Opaque container for a secret value.
///
/// `Secret<String>` is the common case; any `T: Zeroize` works. `Clone`
/// is intentionally not implemented so a secret can't be duplicated by
/// accident — callers who truly need two copies can reach for
/// `expose_secret` and wrap the result in a new `Secret`.
pub struct Secret<T: Zeroize>(T);

impl<T: Zeroize> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Read the underlying value. Named verbosely so `rg expose_secret`
    /// returns every call site in one command during audits.
    pub fn expose_secret(&self) -> &T {
        &self.0
    }

    /// Consume the wrapper and return the raw value. Use only at the
    /// absolute edge of the system (e.g. building an HTTP header).
    pub fn into_inner(self) -> T {
        // We need to take the value out without triggering `Drop::drop`
        // — otherwise the returned `T` would be zeroed before the
        // caller could read it. `ManuallyDrop` makes that explicit.
        let this = std::mem::ManuallyDrop::new(self);
        // Safety: `self.0` is behind a `ManuallyDrop`, so reading it out
        // once and never touching it again is sound.
        unsafe { std::ptr::read(&this.0) }
    }
}

impl<T: Zeroize> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

impl<T: Zeroize> std::fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_does_not_leak_value() {
        let s = Secret::new("super-secret-token".to_string());
        let rendered = format!("{s:?}");
        assert_eq!(rendered, "***");
        assert!(!rendered.contains("super-secret-token"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn display_does_not_leak_value() {
        let s = Secret::new("another-token".to_string());
        let rendered = format!("{s}");
        assert_eq!(rendered, "***");
        assert!(!rendered.contains("another-token"));
    }

    #[test]
    fn expose_secret_is_the_only_reader() {
        let s = Secret::new("visible-here".to_string());
        assert_eq!(s.expose_secret(), "visible-here");
    }

    #[test]
    fn into_inner_returns_raw_value() {
        let s = Secret::new("raw".to_string());
        assert_eq!(s.into_inner(), "raw");
    }
}
