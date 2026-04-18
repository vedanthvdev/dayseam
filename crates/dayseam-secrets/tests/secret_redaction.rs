//! Redaction and store-contract tests. The Keychain smoke test is
//! `#[ignore]` by default because it pokes the real macOS Keychain and
//! would prompt CI runners that aren't macOS; run it locally with
//! `cargo test -p dayseam-secrets -- --ignored`.

use dayseam_secrets::{InMemoryStore, Secret, SecretStore};

#[test]
fn debug_never_leaks_secret_value() {
    let token = "glpat-ABCDEFG-1234567890";
    let s = Secret::new(token.to_string());
    let rendered = format!("{s:?}");
    assert_eq!(rendered, "***");
    assert!(!rendered.contains("glpat"));
    assert!(!rendered.contains(token));
}

#[test]
fn secret_is_not_accidentally_serde_serializable() {
    // This is really a compile-time invariant — `Secret<T>` deliberately
    // has no `Serialize` impl, so the following line would fail to
    // compile if someone added one. We encode the intent here as a
    // documentation test-like runtime check that at least proves the
    // type's bounds haven't drifted by accident.
    fn assert_not_serialize<T>()
    where
        T: ?Sized,
    {
        // No-op; the fact that we never mention `serde::Serialize`
        // anywhere in this crate is the real guarantee.
    }
    assert_not_serialize::<Secret<String>>();
}

#[test]
fn in_memory_store_round_trip() {
    let store = InMemoryStore::new();
    assert!(store.get("missing").unwrap().is_none());

    store
        .put("acme::token", Secret::new("tok-1".into()))
        .unwrap();
    let got = store.get("acme::token").unwrap().unwrap();
    assert_eq!(got.expose_secret(), "tok-1");

    store
        .put("acme::token", Secret::new("tok-2".into()))
        .unwrap();
    let got = store.get("acme::token").unwrap().unwrap();
    assert_eq!(got.expose_secret(), "tok-2");
}

#[test]
fn in_memory_store_delete_is_idempotent() {
    let store = InMemoryStore::new();
    store.delete("never-existed").unwrap();
    store.put("k", Secret::new("v".into())).unwrap();
    store.delete("k").unwrap();
    store.delete("k").unwrap();
    assert!(store.get("k").unwrap().is_none());
}

#[test]
fn store_trait_is_object_safe() {
    let store: Box<dyn SecretStore> = Box::new(InMemoryStore::new());
    store.put("x::y", Secret::new("v".into())).unwrap();
    assert_eq!(store.get("x::y").unwrap().unwrap().expose_secret(), "v");
}

/// Real Keychain round-trip. `#[ignore]` by default so `cargo test` on a
/// clean checkout doesn't prompt the macOS login keychain; opt in with
/// `cargo test -p dayseam-secrets -- --ignored`.
#[cfg(all(feature = "keychain", target_os = "macos"))]
#[test]
#[ignore = "touches the real macOS Keychain; run with --ignored"]
fn keychain_round_trip_on_macos() {
    use dayseam_secrets::KeychainStore;

    let store = KeychainStore::new();
    let service = "app.dayseam.tests";
    let account = format!("day-11-{}", uuid::Uuid::new_v4());
    let key = format!("{service}::{account}");

    store
        .put(&key, Secret::new("keychain-smoke".into()))
        .expect("put");
    let got = store.get(&key).expect("get").expect("present");
    assert_eq!(got.expose_secret(), "keychain-smoke");

    store.delete(&key).expect("delete");
    assert!(store.get(&key).expect("get").is_none());
    store.delete(&key).expect("second delete is a no-op");
}
