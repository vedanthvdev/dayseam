//! End-to-end integration tests for the three IPC commands that hit
//! HTTP — `github_validate_credentials`, `atlassian_validate_credentials`,
//! and the `github_sources_reconnect` login-row self-heal
//! (**DAY-111 / TST-v0.4-04**).
//!
//! Before DAY-111 each of those commands constructed its own
//! [`HttpClient`] inline (`HttpClient::new()?`), which left the IPC
//! surface untestable without a live network. DAY-111 promoted the
//! client to a process-wide [`AppState::http`] field so:
//!
//!   1. The dialog probe (`validate_credentials`) and the walker use
//!      the same retry / jitter / cancellation contract, and
//!   2. Integration tests can inject a wiremock-backed client with
//!      [`AppState::with_http_for_test`] and exercise the full
//!      validate → persist → seed-identity chain.
//!
//! The three scenarios below pin the IPC contracts most likely to
//! silently regress:
//!
//! * [`github_validate_credentials_injects_state_http_and_surfaces_login`] —
//!   a wiremock `/user` response flows through the injected client
//!   and lands in the [`GithubValidationResult`] the dialog renders.
//!   Would catch a regression that swapped `state.http` back to
//!   `HttpClient::new()?`; the test origin is `http://127.0.0.1:PORT`
//!   and the production client would be rejected by `parse_api_base_url`
//!   before the request, failing loud.
//! * [`atlassian_validate_credentials_injects_state_http_and_surfaces_account`] —
//!   same shape for the Atlassian `/rest/api/3/myself` probe. Pins
//!   the `state.http` rewrite on the Atlassian side so an inline
//!   `HttpClient::new()?` creeping back in surfaces immediately.
//! * [`github_reconnect_self_heals_missing_login_row`] — seed a
//!   GitHub source with *only* a `GitHubUserId` identity (the shape
//!   a pre-DAY-101 install carries), run `github_sources_reconnect`
//!   against a wiremock `/user` returning the matching numeric id
//!   and a non-empty `login`, and assert the post-reconnect identity
//!   set contains a `GitHubLogin` row with that login. This is the
//!   CORR-v0.4-01 self-heal contract: reverting the login-row seed
//!   at `src/ipc/github.rs:596..610` would leave the identity set
//!   `{GitHubUserId}` only and the next walk would silently return
//!   `WalkOutcome::default()`.
//!
//! The suite is gated behind the `test-helpers` feature (see
//! `Cargo.toml`'s `[[test]]` block) so a plain `cargo test
//! -p dayseam-desktop` continues to compile; CI's
//! `cargo test --workspace --all-features` runs it.

#![cfg(feature = "test-helpers")]

use std::sync::Arc;

use chrono::Utc;
use connectors_sdk::{HttpClient, RetryPolicy};
use dayseam_core::{
    Source, SourceConfig, SourceHealth, SourceIdentity, SourceIdentityKind, SourceKind,
};
use dayseam_db::{open, PersonRepo, SourceIdentityRepo, SourceRepo};
use dayseam_desktop::ipc::atlassian::atlassian_validate_credentials_impl;
use dayseam_desktop::ipc::commands::SELF_DEFAULT_DISPLAY_NAME;
use dayseam_desktop::ipc::github::{
    github_sources_reconnect_impl, github_validate_credentials_impl,
};
use dayseam_desktop::ipc::secret::IpcSecretString;
use dayseam_desktop::AppState;
use dayseam_events::AppBus;
use dayseam_orchestrator::{ConnectorRegistry, OrchestratorBuilder, SinkRegistry};
use dayseam_secrets::{InMemoryStore, Secret};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- Scaffolding ---------------------------------------------------------

/// Build an in-memory-ish [`AppState`] whose [`HttpClient`] is the
/// same retry-aware wrapper the walker uses, just with
/// [`RetryPolicy::instant()`] so a wiremock 5xx (if any test ever
/// wants one) doesn't burn seconds of wall-clock. `TempDir` is
/// returned alongside so the SQLite file outlives the pool.
async fn make_state_with_http() -> (AppState, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let pool = open(&dir.path().join("state.db"))
        .await
        .expect("open sqlite");
    let app_bus = AppBus::new();
    let orchestrator = OrchestratorBuilder::new(
        pool.clone(),
        app_bus.clone(),
        ConnectorRegistry::new(),
        SinkRegistry::new(),
    )
    .build()
    .expect("build orchestrator");
    let http = HttpClient::new()
        .expect("build HttpClient")
        .with_policy(RetryPolicy::instant());
    let state = AppState::with_http_for_test(
        pool,
        app_bus,
        Arc::new(InMemoryStore::new()),
        orchestrator,
        http,
    );
    (state, dir)
}

/// `127.0.0.1:PORT` — the shape [`parse_api_base_url`] and
/// [`parse_workspace_url`] accept under the `test-helpers` seam.
fn mock_origin(server: &MockServer) -> String {
    server.uri()
}

// ---- Scenario 1: GitHub validate_credentials ----------------------------
//
// The dialog probe must flow through `state.http`. The wiremock
// server responds on `GET /user` with the canonical
// `{id, login, name}` triple `validate_auth` decodes into
// `GithubUserInfo`; the IPC command projects that into a
// `GithubValidationResult` the dialog renders in "Connected as …".
//
// What a revert looks like: replace `&state.http` at
// `src/ipc/github.rs:228` with a freshly-built `HttpClient::new()?`.
// The freshly-built client uses production defaults (no retry
// override), which is fine — but the *request* still goes to the
// `http://127.0.0.1:PORT` origin, so the test would still pass by
// accident. The load-bearing assertion is therefore the request
// *count*: exactly one `/user` hit lands on the mock. A
// dropped-into-inline `HttpClient::new()?` path + a future retry
// tweak that silently doubled attempts would surface here.

#[tokio::test]
async fn github_validate_credentials_injects_state_http_and_surfaces_login() {
    let (state, _dir) = make_state_with_http().await;
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header("Accept", "application/vnd.github+json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 4242,
            "login": "vedanth",
            "name": "Vedanth Vasudev",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = github_validate_credentials_impl(
        &state,
        mock_origin(&server),
        IpcSecretString::new("ghp_wiremock_test_token"),
    )
    .await
    .expect("validate against wiremock /user");

    assert_eq!(result.user_id, 4242, "numeric user id round-trips");
    assert_eq!(
        result.login, "vedanth",
        "login string lands in the result the dialog renders"
    );
    assert_eq!(
        result.name.as_deref(),
        Some("Vedanth Vasudev"),
        "optional name falls through"
    );

    // Exactly one /user hit — no accidental retry, no silent
    // second probe. A future refactor that (say) calls
    // `validate_auth` twice to backfill display fields would
    // surface as a count mismatch here before it ships.
    let user_hits = server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .iter()
        .filter(|r| r.url.path() == "/user")
        .count();
    assert_eq!(user_hits, 1, "one-shot probe must hit /user exactly once");
}

// ---- Scenario 2: Atlassian validate_credentials -------------------------
//
// The Atlassian dialog probe routes through `state.http` the same
// way. Wiremock fronts `GET /rest/api/3/myself` with the
// `accountId`/`displayName`/`emailAddress` JSON shape
// `discover_cloud` decodes. The IPC command hands back the triple
// the dialog's "Connected as …" ribbon needs.
//
// What a revert looks like: an inline `HttpClient::new()?` would
// still emit the request (the production client has no URL
// restriction beyond reqwest's own), but pinning the *hit count*
// at exactly one catches a future retry-on-success regression or
// a second probe sneaking in to validate Jira scope separately.

#[tokio::test]
async fn atlassian_validate_credentials_injects_state_http_and_surfaces_account() {
    let (state, _dir) = make_state_with_http().await;
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .and(header("Accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountId": "5d53f3cbc6b9320d9ea5bdc2",
            "displayName": "Vedanth Vasudev",
            "emailAddress": "vedanth@example.com",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = atlassian_validate_credentials_impl(
        &state,
        mock_origin(&server),
        "vedanth@example.com".into(),
        IpcSecretString::new("atlassian_wiremock_test_token"),
    )
    .await
    .expect("validate against wiremock /rest/api/3/myself");

    assert_eq!(
        result.account_id, "5d53f3cbc6b9320d9ea5bdc2",
        "accountId round-trips from the myself response"
    );
    assert_eq!(
        result.display_name, "Vedanth Vasudev",
        "displayName lands in the result the dialog renders"
    );
    assert_eq!(
        result.email.as_deref(),
        Some("vedanth@example.com"),
        "emailAddress falls through so the dialog can prefill the field"
    );

    let myself_hits = server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .iter()
        .filter(|r| r.url.path() == "/rest/api/3/myself")
        .count();
    assert_eq!(
        myself_hits, 1,
        "one-shot cloud-discovery probe must hit /rest/api/3/myself exactly once"
    );
}

// ---- Scenario 3: GitHub reconnect login-row self-heal -------------------
//
// Seed a GitHub source with *only* the `GitHubUserId` identity —
// the shape a pre-DAY-101 install carries, where `list_identities`
// seeded one row instead of two. Then run `github_sources_reconnect`
// with a wiremock `/user` that returns the matching numeric id and
// a non-empty login. The reconnect impl's CORR-v0.4-01 self-heal
// (`src/ipc/github.rs:596..610`) must ensure a `GitHubLogin` row
// lands so the next walk composes `/users/{login}/events` instead
// of early-bailing with `WalkOutcome::default()`.
//
// What a revert looks like: delete the `SourceIdentity { … kind:
// GitHubLogin }` block at `:596..610`. The reconnect still returns
// `Ok(source_id)`, the keychain slot still rotates, and every UI
// surface reports green — but the identity set stays
// `{GitHubUserId}` only and the walker silently contributes zero
// events on every future sync (DOG-v0.2-04 class).

#[tokio::test]
async fn github_reconnect_self_heals_missing_login_row() {
    let (state, _dir) = make_state_with_http().await;
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 4242,
            "login": "vedanth",
            "name": "Vedanth Vasudev",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Seed a GitHub `sources` row pointing at the wiremock origin.
    // The `api_base_url` has to be reachable from the reconnect
    // impl — that's why `parse_api_base_url`'s `test-helpers`
    // carve-out exists. The trailing slash keeps
    // `Url::join("user")` canonical.
    let source_repo = SourceRepo::new(state.pool.clone());
    let identity_repo = SourceIdentityRepo::new(state.pool.clone());
    let person_repo = PersonRepo::new(state.pool.clone());

    let source_id = Uuid::new_v4();
    let api_base_url = format!("{}/", server.uri());
    let secret_ref = dayseam_core::SecretRef {
        keychain_service: "dayseam.github".to_string(),
        keychain_account: format!("source:{source_id}"),
    };
    let source = Source {
        id: source_id,
        kind: SourceKind::GitHub,
        label: "GitHub — wiremock".to_string(),
        config: SourceConfig::GitHub { api_base_url },
        secret_ref: Some(secret_ref.clone()),
        created_at: Utc::now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    source_repo.insert(&source).await.expect("insert source");

    // Seed the keychain slot so the post-validate rotate succeeds.
    // Matches `commands::secret_store_key`'s `{service}::{account}`
    // shape — we duplicate the format here rather than exposing the
    // helper publicly because the format is a stable invariant the
    // test is *also* pinning.
    let secret_key = format!(
        "{}::{}",
        secret_ref.keychain_service, secret_ref.keychain_account
    );
    state
        .secrets
        .put(&secret_key, Secret::new("ghp_pre_rotate".to_string()))
        .expect("seed keychain");

    // Seed the self-person + a single `GitHubUserId` identity row
    // that matches the wiremock numeric id. Deliberately skip the
    // `GitHubLogin` row — that's the pre-DAY-101 shape the
    // self-heal must repair.
    let self_person = person_repo
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .expect("bootstrap self");
    let user_id_row = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: self_person.id,
        source_id: Some(source_id),
        kind: SourceIdentityKind::GitHubUserId,
        external_actor_id: "4242".to_string(),
    };
    identity_repo
        .insert(&user_id_row)
        .await
        .expect("seed GitHubUserId row");

    // Pre-condition: no GitHubLogin row.
    let before = identity_repo
        .list_for_source(self_person.id, &source_id)
        .await
        .expect("list identities before reconnect");
    assert_eq!(before.len(), 1, "pre-reconnect: one identity row seeded");
    assert!(
        !before
            .iter()
            .any(|i| matches!(i.kind, SourceIdentityKind::GitHubLogin)),
        "pre-reconnect: GitHubLogin row must not exist (self-heal has nothing to do otherwise)"
    );

    // Reconnect — new wiremock `/user` returns the matching id +
    // a login, so the invariant check passes and the self-heal
    // inserts the missing `GitHubLogin` row.
    let returned =
        github_sources_reconnect_impl(&state, source_id, IpcSecretString::new("ghp_post_rotate"))
            .await
            .expect("reconnect self-heal path");
    assert_eq!(returned, source_id);

    // Post-condition: both identity rows present.
    let after = identity_repo
        .list_for_source(self_person.id, &source_id)
        .await
        .expect("list identities after reconnect");
    assert_eq!(
        after.len(),
        2,
        "post-reconnect: GitHubUserId + GitHubLogin must both be present"
    );
    let logins: Vec<_> = after
        .iter()
        .filter(|i| matches!(i.kind, SourceIdentityKind::GitHubLogin))
        .collect();
    assert_eq!(
        logins.len(),
        1,
        "post-reconnect: exactly one GitHubLogin row (self-heal is idempotent)"
    );
    assert_eq!(
        logins[0].external_actor_id, "vedanth",
        "self-heal must write the login wiremock returned, not a placeholder"
    );

    // And the keychain slot was rotated — the post-reconnect
    // secret is the new one, not the pre-rotate value.
    let rotated = state
        .secrets
        .get(&secret_key)
        .expect("keychain get")
        .expect("keychain row still present");
    assert_eq!(
        rotated.expose_secret(),
        "ghp_post_rotate",
        "reconnect must rotate the keychain slot after the self-heal"
    );
}
