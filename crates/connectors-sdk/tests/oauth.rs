//! End-to-end behaviour of DAY-201's OAuth 2.0 wire work against a
//! real `wiremock` HTTP server.
//!
//! The unit tests in `src/auth.rs::tests` and `src/oauth.rs::tests`
//! prove the types shape right in isolation — PKCE alphabet, token
//! redaction, descriptor validation, scope-downgrade arithmetic.
//! This file drives the composed thing: a real `reqwest::Client`
//! posting form-encoded token-endpoint requests against a
//! mock-backed server and observing the effect on a live
//! [`OAuthAuth`]'s state and its persister's durable copy. A
//! regression in any of the wire-format layering (form encoding,
//! response parsing, error mapping, mutex single-flighting, clock
//! skew arithmetic, refresh-token rotation) surfaces here before it
//! silently breaks the DAY-202 Outlook connector.
//!
//! Each test stands up its own `MockServer` so they can run in
//! parallel without stepping on each other's expectations. All
//! tests thread an injectable [`TestClock`] through `OAuthAuth` so
//! expiry arithmetic is deterministic and independent of the host's
//! wall clock.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use connectors_sdk::oauth::{exchange_code, generate_pkce_pair, TokenPair, TokenPersister};
use connectors_sdk::{AuthDescriptor, AuthStrategy, Clock, OAuthAuth, SharedPersister};
use dayseam_core::{error_codes, DayseamError};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Injectable clock whose `now()` is set once at construction and
/// never advances. Good enough for these tests: each test either
/// builds one clock at a fixed instant (so expiry is deterministic)
/// or rebuilds the `OAuthAuth` with a later clock between calls to
/// simulate "time has passed".
#[derive(Debug, Clone)]
struct FixedClock(DateTime<Utc>);

#[async_trait]
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }

    async fn sleep(&self, _: StdDuration) {}
}

/// Persister that records every `persist_pair` call so tests can
/// assert the keychain write actually happened after a successful
/// refresh. Holds a `Vec<TokenPair>` under a std mutex (not tokio
/// — the only mutation points are from inside async fns that yield
/// immediately).
#[derive(Debug, Default)]
struct RecordingPersister(StdMutex<Vec<TokenPair>>);

impl RecordingPersister {
    fn calls(&self) -> Vec<TokenPair> {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl TokenPersister for RecordingPersister {
    async fn persist_pair(&self, pair: &TokenPair) -> Result<(), DayseamError> {
        self.0.lock().unwrap().push(pair.clone());
        Ok(())
    }
}

fn sample_descriptor() -> AuthDescriptor {
    AuthDescriptor::OAuth {
        issuer: "https://login.microsoftonline.com/organizations/v2.0".into(),
        client_id: "00000000-0000-0000-0000-000000000000".into(),
        scopes: vec![
            "offline_access".into(),
            "Calendars.Read".into(),
            "User.Read".into(),
        ],
        keychain_service: "dayseam.outlook".into(),
        access_keychain_account: "user@contoso.com.oauth.access".into(),
        refresh_keychain_account: "user@contoso.com.oauth.refresh".into(),
    }
}

fn build_oauth_auth(
    server: &MockServer,
    clock: FixedClock,
    persister: SharedPersister,
    access_expires_at: DateTime<Utc>,
) -> OAuthAuth {
    OAuthAuth::new(
        "initial-access-token",
        "initial-refresh-token",
        access_expires_at,
        sample_descriptor(),
        format!("{}/token", server.uri()),
        reqwest::Client::new(),
        persister,
        Arc::new(clock),
    )
    .expect("valid descriptor")
}

/// PR #1's headline scenario: `authenticate()` on an `OAuthAuth`
/// whose access token has just expired fires a single
/// `refresh_token` grant against the IdP, rewrites its state, hands
/// the new access token back as a `Bearer` header, and writes both
/// tokens through to the persister. The assertion set covers the
/// full round-trip: bearer value, persisted pair, and state
/// freshness after the call.
#[tokio::test]
async fn authenticate_refreshes_and_attaches_new_bearer_on_expired_token() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let persister = Arc::new(RecordingPersister::default());

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=initial-refresh-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "refresh_token": "rotated-refresh-token",
            "expires_in": 3600,
            "scope": "offline_access Calendars.Read User.Read",
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Access token is expired by construction so the refresh path
    // must fire.
    let strat = build_oauth_auth(
        &server,
        clock.clone(),
        persister.clone() as SharedPersister,
        now - Duration::seconds(1),
    );

    let client = reqwest::Client::new();
    let out = strat
        .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
        .await
        .expect("refresh-then-bearer happy path");
    let built = out.build().expect("request builds");
    assert_eq!(
        built
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer fresh-access-token"),
        "authenticate must hand back the *new* access token, not the stale one"
    );

    let persisted = persister.calls();
    assert_eq!(persisted.len(), 1, "persister must be called exactly once");
    let p = &persisted[0];
    assert_eq!(p.access_token, "fresh-access-token");
    assert_eq!(p.refresh_token, "rotated-refresh-token");
    assert_eq!(p.granted_scopes.len(), 3);
    assert_eq!(p.access_expires_at, now + Duration::seconds(3600));
}

/// A second `authenticate()` call against a still-fresh token must
/// **not** trigger a refresh — proves the `expires_at` guard short-
/// circuits the HTTP path. Mount no mocks: any network call would
/// panic with "no matching mock", so a `.expect(0)` assertion is
/// implicit.
#[tokio::test]
async fn authenticate_is_noop_on_non_expired_token() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let persister = Arc::new(RecordingPersister::default());

    let strat = build_oauth_auth(
        &server,
        clock,
        persister.clone() as SharedPersister,
        now + Duration::hours(1),
    );

    let client = reqwest::Client::new();
    let out = strat
        .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
        .await
        .expect("non-expired ⇒ no-op refresh");
    let built = out.build().expect("request builds");
    assert_eq!(
        built
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer initial-access-token"),
    );
    assert!(
        persister.calls().is_empty(),
        "no refresh must have fired, so no persister call"
    );
}

/// Terminal 400 `invalid_grant` surfaces as
/// [`error_codes::OAUTH_REFRESH_REJECTED`] with `retryable: false`.
/// Mirrors what happens when the user has revoked consent in their
/// tenant admin portal or the refresh token has aged past its
/// absolute lifetime.
#[tokio::test]
async fn refresh_rejected_invalid_grant_surfaces_stable_code() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let persister = Arc::new(RecordingPersister::default());

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "AADSTS70008: The refresh token has expired due to inactivity.",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let strat = build_oauth_auth(
        &server,
        clock,
        persister.clone() as SharedPersister,
        now - Duration::seconds(1),
    );

    let client = reqwest::Client::new();
    let err = strat
        .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
        .await
        .expect_err("invalid_grant must surface a terminal error");
    match err {
        DayseamError::Auth {
            code,
            retryable,
            action_hint,
            message,
        } => {
            assert_eq!(code, error_codes::OAUTH_REFRESH_REJECTED);
            assert!(
                !retryable,
                "invalid_grant is terminal; the UI must not silently retry"
            );
            assert!(
                action_hint.is_some_and(|h| h.to_lowercase().contains("reconnect")),
                "action_hint must point at the Reconnect flow",
            );
            assert!(
                message.contains("invalid_grant"),
                "message should carry the IdP-supplied error code for debugging: {message}",
            );
        }
        other => panic!("expected DayseamError::Auth, got {other:?}"),
    }
    assert!(
        persister.calls().is_empty(),
        "failed refresh must not persist anything"
    );
}

/// Clock-skew scenario: the stored `access_expires_at` is in the
/// past because the host clock jumped forward between the last
/// sync and now, but the refresh token is still valid. The refresh
/// succeeds and the state is repaired. Proves that the SDK does
/// not treat a stale `expires_at` as a terminal condition when the
/// IdP is ready to mint a new token.
#[tokio::test]
async fn clock_skew_triggers_refresh_and_state_is_repaired() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let persister = Arc::new(RecordingPersister::default());

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "repair-access-token",
            "refresh_token": "repair-refresh-token",
            "expires_in": 3600,
            "scope": "offline_access Calendars.Read User.Read",
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Simulate the host clock having jumped forward: stored
    // `expires_at` is 10 minutes behind `clock.now()` even though
    // the refresh token itself is fine.
    let strat = build_oauth_auth(
        &server,
        clock,
        persister.clone() as SharedPersister,
        now - Duration::minutes(10),
    );

    let client = reqwest::Client::new();
    let _ = strat
        .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
        .await
        .expect("clock-skewed expiry must refresh, not terminate");

    // A second call on the same strategy must now see a live token
    // and not re-trigger the refresh (the mock is `.expect(1)`, so
    // a second network call would fail the assertion at test-drop
    // time).
    let _ = strat
        .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
        .await
        .expect("post-refresh state must be live");

    assert_eq!(
        persister.calls().len(),
        1,
        "exactly one persister call — the skew repair, not the second authenticate"
    );
}

/// Scope-downgrade: the IdP returns 200 with a strict subset of the
/// requested scopes. The refresh still succeeds (we don't want to
/// tank an in-flight sync over a narrower-but-valid token), the
/// state is rewritten, and the persister sees the narrower scope
/// set so the orchestrator can decide whether to raise a reconnect
/// nudge on the next sync boundary.
#[tokio::test]
async fn scope_downgrade_is_recorded_but_not_fatal() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let persister = Arc::new(RecordingPersister::default());

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "narrower-access-token",
            "refresh_token": "narrower-refresh-token",
            "expires_in": 1800,
            // Tenant admin removed `Calendars.Read` overnight.
            "scope": "offline_access User.Read",
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let strat = build_oauth_auth(
        &server,
        clock,
        persister.clone() as SharedPersister,
        now - Duration::seconds(1),
    );

    let client = reqwest::Client::new();
    let _ = strat
        .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
        .await
        .expect("narrower-but-valid scope must not fail the refresh");

    let persisted = persister.calls();
    assert_eq!(persisted.len(), 1);
    let got = &persisted[0].granted_scopes;
    assert!(
        got.contains(&"offline_access".to_string()) && got.contains(&"User.Read".to_string()),
        "granted scopes must survive the persist round trip: {got:?}"
    );
    assert!(
        !got.contains(&"Calendars.Read".to_string()),
        "the downgrade must be observable to the orchestrator"
    );

    // Helper function verifies the downgrade is classified as such —
    // exercises the `is_scope_downgrade` canonical comparator.
    let requested: Vec<String> = match strat.descriptor() {
        AuthDescriptor::OAuth { scopes, .. } => scopes,
        _ => unreachable!(),
    };
    assert!(
        connectors_sdk::is_scope_downgrade(&requested, got),
        "orchestrator-facing comparator must agree that Calendars.Read was dropped"
    );
}

/// A refresh response that omits the `refresh_token` field (some
/// IdPs do this to mean "keep the old one") preserves the previous
/// refresh token in the persisted pair. Microsoft rotates on every
/// grant so this path matters mostly for defensive portability, but
/// it is load-bearing for the "no silent drift" property: the
/// keychain's refresh row must never end up as the empty string
/// because a parse branch assumed `refresh_token` was always
/// populated.
#[tokio::test]
async fn refresh_without_rotated_refresh_token_preserves_previous() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let persister = Arc::new(RecordingPersister::default());

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "only-access-rotated",
            "expires_in": 3600,
            "scope": "offline_access Calendars.Read User.Read",
            "token_type": "Bearer",
            // Deliberately no `refresh_token` field.
        })))
        .expect(1)
        .mount(&server)
        .await;

    let strat = build_oauth_auth(
        &server,
        clock,
        persister.clone() as SharedPersister,
        now - Duration::seconds(1),
    );

    let client = reqwest::Client::new();
    let _ = strat
        .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
        .await
        .expect("missing-refresh response must still succeed");

    let persisted = persister.calls();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].access_token, "only-access-rotated");
    assert_eq!(
        persisted[0].refresh_token, "initial-refresh-token",
        "previous refresh token must be preserved when the IdP omits rotation"
    );
}

/// Single-flight invariant: N concurrent `authenticate()` calls on
/// the same expired `OAuthAuth` collapse into exactly one network
/// round-trip. Mounted with `.expect(1)` — a second request would
/// tank the test at drop time. Each caller still observes a
/// freshly bearer-authenticated request on return.
///
/// Uses `tokio::task::JoinSet` to fan out 16 parallel authenticate
/// calls. The mutex on `OAuthAuth`'s state plus the post-lock
/// double-check of `expires_at` produces this behaviour without
/// any caller-side coordination.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_authenticate_collapses_to_one_refresh() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let persister = Arc::new(RecordingPersister::default());

    // A small in-test counter lets us also assert from the
    // persister side that exactly one write happened, even if
    // wiremock's own `.expect(1)` were to be too forgiving.
    let exchange_hits = Arc::new(AtomicUsize::new(0));

    // Wrap wiremock's response in a custom responder so we can
    // count hits ourselves and detect any real-world races.
    let hits_for_mock = exchange_hits.clone();
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |_: &wiremock::Request| {
            hits_for_mock.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "concurrent-refreshed",
                "refresh_token": "concurrent-rotated",
                "expires_in": 3600,
                "scope": "offline_access Calendars.Read User.Read",
                "token_type": "Bearer",
            }))
        })
        .expect(1)
        .mount(&server)
        .await;

    let strat = Arc::new(build_oauth_auth(
        &server,
        clock,
        persister.clone() as SharedPersister,
        now - Duration::seconds(1),
    ));

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let strat = strat.clone();
        set.spawn(async move {
            let client = reqwest::Client::new();
            let out = strat
                .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
                .await
                .expect("each caller must observe a refreshed bearer");
            let built = out.build().expect("request builds");
            built
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        });
    }

    let mut observed = Vec::with_capacity(16);
    while let Some(res) = set.join_next().await {
        observed.push(res.expect("task panic").expect("header present"));
    }

    assert_eq!(
        exchange_hits.load(Ordering::SeqCst),
        1,
        "single-flight must collapse 16 concurrent calls into one refresh"
    );
    assert_eq!(
        persister.calls().len(),
        1,
        "persister should only see the single successful refresh",
    );
    assert_eq!(observed.len(), 16);
    for header in observed {
        assert_eq!(header, "Bearer concurrent-refreshed");
    }
}

/// `exchange_code` (the first-time consent path) turns an auth
/// code + PKCE verifier into a [`TokenPair`] when the IdP responds
/// 200. The outgoing form body carries every RFC 6749 §4.1 param
/// the endpoint expects: `grant_type=authorization_code`, `code`,
/// `code_verifier`, `client_id`, `redirect_uri`.
#[tokio::test]
async fn exchange_code_happy_path_parses_token_pair() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let clock = FixedClock(now);
    let http = reqwest::Client::new();

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=one-shot-auth-code"))
        .and(body_string_contains("code_verifier="))
        .and(body_string_contains(
            "redirect_uri=http%3A%2F%2F127.0.0.1%2Foauth%2Fcallback",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "initial-access",
            "refresh_token": "initial-refresh",
            "expires_in": 3600,
            "scope": "offline_access Calendars.Read User.Read",
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut rng = rand::thread_rng();
    let (verifier, _challenge) = generate_pkce_pair(&mut rng);

    let pair = exchange_code(
        &http,
        &format!("{}/token", server.uri()),
        "00000000-0000-0000-0000-000000000000",
        "one-shot-auth-code",
        &verifier,
        "http://127.0.0.1/oauth/callback",
        &clock,
    )
    .await
    .expect("exchange must succeed");

    assert_eq!(pair.access_token, "initial-access");
    assert_eq!(pair.refresh_token, "initial-refresh");
    assert_eq!(pair.access_expires_at, now + Duration::seconds(3600));
    assert_eq!(pair.granted_scopes.len(), 3);
}

/// `exchange_code` failures surface as the same stable
/// [`error_codes::OAUTH_REFRESH_REJECTED`] code the refresh path
/// uses — keeps the UI's "reconnect" reaction uniform across both
/// entry points without forcing callers to branch on which step
/// failed.
#[tokio::test]
async fn exchange_code_rejected_maps_to_stable_code() {
    let server = MockServer::start().await;
    let clock = FixedClock(Utc::now());
    let http = reqwest::Client::new();

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "AADSTS9002313: Invalid request.",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut rng = rand::thread_rng();
    let (verifier, _challenge) = generate_pkce_pair(&mut rng);

    let err = exchange_code(
        &http,
        &format!("{}/token", server.uri()),
        "00000000-0000-0000-0000-000000000000",
        "one-shot-auth-code",
        &verifier,
        "http://127.0.0.1/oauth/callback",
        &clock,
    )
    .await
    .expect_err("invalid_grant must surface a terminal error");
    match err {
        DayseamError::Auth { code, .. } => {
            assert_eq!(code, error_codes::OAUTH_REFRESH_REJECTED);
        }
        other => panic!("expected Auth, got {other:?}"),
    }
}
