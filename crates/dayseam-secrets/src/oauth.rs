//! OAuth 2.0 token-pair helpers.
//!
//! `AuthDescriptor::OAuth` (in `connectors-sdk`) stores two tokens per
//! source: the access token and the refresh token, each under its own
//! Keychain row. The `(keychain_service, access_keychain_account)` and
//! `(keychain_service, refresh_keychain_account)` pairs the descriptor
//! carries are independently valid `SecretStore` handles, but they
//! share a naming convention that's easy to get wrong by hand —
//! swapping access for refresh on a read would silently hand a
//! long-lived refresh token to an endpoint expecting a short-lived
//! access token, which is both a security smell and a bug that only
//! surfaces on the next refresh cycle.
//!
//! This module exposes six free-standing helpers (`put_*`, `get_*`,
//! `delete_*` for each of `access_token` and `refresh_token`) that
//! take the `(service, account)` pair as separate args and handle the
//! `"service::account"` composite-key shape that [`KeychainStore`]
//! expects. The functions are identical up to the argument name; the
//! split exists so every call site at the orchestrator / connector
//! boundary reads as `put_access_token` / `put_refresh_token` rather
//! than a generic `put` that has to be guessed-at from context.
//!
//! The full DAY-200 scaffold lives in `connectors-sdk::auth::OAuthAuth`
//! — see its struct-level docs for the store-and-restore flow these
//! helpers plug into.
//!
//! [`KeychainStore`]: crate::KeychainStore

use crate::{Secret, SecretResult, SecretStore};

/// Build the composite key `KeychainStore` splits on.
///
/// `dayseam_secrets::keychain::split_key` rejects keys without the
/// `"::"` separator; this helper is the single writer side of that
/// contract so callers never spell the separator out in-line.
fn compose_key(service: &str, account: &str) -> String {
    format!("{service}::{account}")
}

/// Store the OAuth access token at `(service, account)`.
///
/// Overwrites any existing access-token row under the same key — the
/// refresh cycle is expected to rewrite this row on every successful
/// token swap.
pub fn put_access_token(
    store: &dyn SecretStore,
    service: &str,
    account: &str,
    token: Secret<String>,
) -> SecretResult<()> {
    store.put(&compose_key(service, account), token)
}

/// Read the OAuth access token from `(service, account)`, returning
/// `Ok(None)` if the row is absent (never been written, or deleted on
/// a disconnect).
pub fn get_access_token(
    store: &dyn SecretStore,
    service: &str,
    account: &str,
) -> SecretResult<Option<Secret<String>>> {
    store.get(&compose_key(service, account))
}

/// Remove the OAuth access-token row. Idempotent — deleting an absent
/// row returns `Ok(())`.
pub fn delete_access_token(
    store: &dyn SecretStore,
    service: &str,
    account: &str,
) -> SecretResult<()> {
    store.delete(&compose_key(service, account))
}

/// Store the OAuth refresh token at `(service, account)`.
///
/// Conventionally `account` here is a *different* string from the
/// access-token account — same `service`, distinct `account` — so the
/// two tokens sit in separate Keychain rows and a compromise of one
/// does not unlock the other. The SDK does not enforce the
/// separation; [`AuthDescriptor::OAuth`] is the durable record of
/// which account strings the orchestrator chose.
///
/// [`AuthDescriptor::OAuth`]:
///     https://docs.rs/connectors-sdk/latest/connectors_sdk/auth/enum.AuthDescriptor.html#variant.OAuth
pub fn put_refresh_token(
    store: &dyn SecretStore,
    service: &str,
    account: &str,
    token: Secret<String>,
) -> SecretResult<()> {
    store.put(&compose_key(service, account), token)
}

/// Read the OAuth refresh token from `(service, account)`. `Ok(None)`
/// here typically means the user disconnected the source and the
/// orchestrator already cleaned up — callers should treat it as a
/// re-auth signal, not an error.
pub fn get_refresh_token(
    store: &dyn SecretStore,
    service: &str,
    account: &str,
) -> SecretResult<Option<Secret<String>>> {
    store.get(&compose_key(service, account))
}

/// Remove the OAuth refresh-token row. Idempotent.
pub fn delete_refresh_token(
    store: &dyn SecretStore,
    service: &str,
    account: &str,
) -> SecretResult<()> {
    store.delete(&compose_key(service, account))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryStore;

    /// The canonical round trip: write an access token, read it back,
    /// get exactly what we stored. If this regresses the rest of
    /// DAY-200's OAuth scaffold collapses — everything downstream
    /// assumes `put_access_token` + `get_access_token` on the same key
    /// pair is a no-op identity.
    #[test]
    fn access_token_put_get_round_trip() {
        let store = InMemoryStore::new();
        let token = Secret::new("access-abc-123".to_string());
        put_access_token(&store, "dayseam.outlook", "user.oauth.access", token)
            .expect("put succeeds");

        let out = get_access_token(&store, "dayseam.outlook", "user.oauth.access")
            .expect("get succeeds")
            .expect("row present");
        assert_eq!(out.expose_secret(), "access-abc-123");
    }

    /// The refresh-token path is implementation-identical to access,
    /// but the named helper is the whole point of this module: the
    /// symmetry is asserted separately so a future refactor that
    /// accidentally collapses `put_refresh_token` into
    /// `put_access_token` has to rewrite this test too, rather than
    /// silently inheriting coverage from the access test.
    #[test]
    fn refresh_token_put_get_round_trip() {
        let store = InMemoryStore::new();
        let token = Secret::new("refresh-xyz-789".to_string());
        put_refresh_token(&store, "dayseam.outlook", "user.oauth.refresh", token)
            .expect("put succeeds");

        let out = get_refresh_token(&store, "dayseam.outlook", "user.oauth.refresh")
            .expect("get succeeds")
            .expect("row present");
        assert_eq!(out.expose_secret(), "refresh-xyz-789");
    }

    /// Access and refresh tokens stored under **different** account
    /// strings land in distinct Keychain rows and read back
    /// independently. This is the isolation guarantee `AuthDescriptor::OAuth`
    /// relies on: its two `*_keychain_account` fields must point at
    /// different rows for the "compromise one row, not both" property
    /// to hold. The helper functions themselves are thin labels over
    /// `store.put/get` — the row separation comes from the caller
    /// passing two different `account` strings, which is exactly what
    /// `AuthDescriptor::OAuth` records.
    #[test]
    fn access_and_refresh_accounts_produce_distinct_rows() {
        let store = InMemoryStore::new();
        put_access_token(
            &store,
            "dayseam.outlook",
            "user.oauth.access",
            Secret::new("ACCESS".into()),
        )
        .expect("put access");
        put_refresh_token(
            &store,
            "dayseam.outlook",
            "user.oauth.refresh",
            Secret::new("REFRESH".into()),
        )
        .expect("put refresh");

        let access = get_access_token(&store, "dayseam.outlook", "user.oauth.access")
            .expect("get")
            .expect("present");
        let refresh = get_refresh_token(&store, "dayseam.outlook", "user.oauth.refresh")
            .expect("get")
            .expect("present");
        assert_eq!(access.expose_secret(), "ACCESS");
        assert_eq!(refresh.expose_secret(), "REFRESH");

        // Deleting the access row must not disturb the refresh row
        // (the actual isolation property we care about: one disconnect
        // path revokes one token, leaving the other recoverable
        // short-term for diagnostics or migration).
        delete_access_token(&store, "dayseam.outlook", "user.oauth.access").expect("delete");
        assert!(
            get_access_token(&store, "dayseam.outlook", "user.oauth.access")
                .expect("get")
                .is_none()
        );
        let refresh_still_there =
            get_refresh_token(&store, "dayseam.outlook", "user.oauth.refresh")
                .expect("get")
                .expect("refresh row untouched");
        assert_eq!(refresh_still_there.expose_secret(), "REFRESH");
    }

    /// The helper function names (`put_access_token` vs
    /// `put_refresh_token`) are documentation, not identity — they
    /// compose the same `"service::account"` key and delegate to
    /// [`SecretStore::put`]. If a caller passes the same `account`
    /// string to both, they end up writing to the same row. This test
    /// pins that behaviour so a future refactor that tries to bake
    /// `.oauth.access` / `.oauth.refresh` suffixes *inside* the
    /// helpers has to update this test — and think hard about why
    /// that duplicates the naming convention already recorded in
    /// `AuthDescriptor::OAuth`'s `*_keychain_account` fields.
    #[test]
    fn helpers_are_labels_over_a_shared_composite_key() {
        let store = InMemoryStore::new();
        // Same (service, account) for both calls — the second put
        // overwrites the first regardless of the function name.
        put_access_token(
            &store,
            "dayseam.outlook",
            "shared.account",
            Secret::new("ACCESS".into()),
        )
        .expect("put access");
        put_refresh_token(
            &store,
            "dayseam.outlook",
            "shared.account",
            Secret::new("REFRESH".into()),
        )
        .expect("put refresh (overwrites)");

        let out = get_access_token(&store, "dayseam.outlook", "shared.account")
            .expect("get")
            .expect("present");
        assert_eq!(
            out.expose_secret(),
            "REFRESH",
            "helpers share a composite key; the callers' distinct account strings are the isolation boundary"
        );
    }

    /// Deletion is idempotent — a source that was never fully
    /// connected (e.g. the user cancelled the consent dialog) leaves
    /// no row to delete, and the orchestrator's disconnect path
    /// should not treat that as an error.
    #[test]
    fn delete_is_idempotent_on_absent_rows() {
        let store = InMemoryStore::new();
        delete_access_token(&store, "dayseam.outlook", "user.oauth.access")
            .expect("delete on absent row is ok");
        delete_refresh_token(&store, "dayseam.outlook", "user.oauth.refresh")
            .expect("delete on absent row is ok");
    }

    /// Overwriting an existing access-token row is expected on every
    /// successful refresh cycle — DAY-201's `refresh_if_expired` will
    /// write a fresh token on top of the old one. The row count at
    /// the store level must therefore stay at 1, not append.
    #[test]
    fn access_token_put_overwrites() {
        let store = InMemoryStore::new();
        put_access_token(
            &store,
            "dayseam.outlook",
            "user.oauth.access",
            Secret::new("OLD".into()),
        )
        .expect("first put");
        put_access_token(
            &store,
            "dayseam.outlook",
            "user.oauth.access",
            Secret::new("NEW".into()),
        )
        .expect("second put overwrites");

        let out = get_access_token(&store, "dayseam.outlook", "user.oauth.access")
            .expect("get")
            .expect("present");
        assert_eq!(out.expose_secret(), "NEW");
    }

    /// After a `delete_access_token`, `get_access_token` returns
    /// `Ok(None)` — not an error — so the orchestrator's "reconnect
    /// needed" branch is reached without having to pattern-match on a
    /// backend-specific error variant.
    #[test]
    fn delete_then_get_returns_none() {
        let store = InMemoryStore::new();
        put_access_token(
            &store,
            "dayseam.outlook",
            "user.oauth.access",
            Secret::new("token".into()),
        )
        .expect("put");
        delete_access_token(&store, "dayseam.outlook", "user.oauth.access").expect("delete");
        assert!(
            get_access_token(&store, "dayseam.outlook", "user.oauth.access")
                .expect("get")
                .is_none()
        );
    }
}
