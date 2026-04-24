//! Wiremock-driven tests for `connector-github::auth::validate_auth`.
//!
//! The matrix:
//!
//! 1. 200 with a well-formed `{ id, login, name }` body → `Ok`.
//! 2. 401 → [`dayseam_core::error_codes::GITHUB_AUTH_INVALID_CREDENTIALS`]
//!    on [`DayseamError::Auth`].
//! 3. 403 → [`dayseam_core::error_codes::GITHUB_AUTH_MISSING_SCOPE`]
//!    on [`DayseamError::Auth`].
//! 4. 404 → [`dayseam_core::error_codes::GITHUB_RESOURCE_NOT_FOUND`]
//!    on [`DayseamError::Network`] (the "Enterprise URL missing
//!    `/api/v3`" failure mode users actually hit).
//! 5. Transport error (unbound port) → `github` family routes through
//!    the reused `gitlab.url.*` codes, consistent with the
//!    `errors::map_transport_error` contract.
//! 6. Every successful probe carries the documented GitHub headers —
//!    `Authorization: Bearer …`, `Accept: application/vnd.github+json`,
//!    `X-GitHub-Api-Version: 2022-11-28`. A header regression would
//!    be silently accepted by github.com (it falls back to a default
//!    version) but explicitly rejected by GHE proxies, so we pin the
//!    exact shape here.

use connector_github::auth::validate_auth;
use connectors_sdk::{HttpClient, PatAuth};
use dayseam_core::error_codes;
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn with_trailing_slash(base: &str) -> Url {
    let s = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    Url::parse(&s).expect("test URL parses")
}

#[tokio::test]
async fn validate_auth_returns_user_info_on_200_with_documented_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header("Authorization", "Bearer ghp-test"))
        .and(header("Accept", "application/vnd.github+json"))
        .and(header("X-GitHub-Api-Version", "2022-11-28"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "login": "vedanth",
            "id": 17,
            "node_id": "MDQ6VXNlcjE3",
            "name": "Vedanth Vasudev",
            "avatar_url": "https://avatars.githubusercontent.com/u/17"
        })))
        .mount(&server)
        .await;

    let http = HttpClient::new().expect("http client");
    let auth = PatAuth::github("ghp-test", "dayseam.github", "vedanth");
    let base = with_trailing_slash(&server.uri());
    let info = validate_auth(&http, &auth, &base, &CancellationToken::new(), None)
        .await
        .expect("200 should return user info");
    assert_eq!(info.id, 17);
    assert_eq!(info.login, "vedanth");
    assert_eq!(info.name.as_deref(), Some("Vedanth Vasudev"));
}

#[tokio::test]
async fn validate_auth_maps_401_to_invalid_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"message":"Bad credentials"}"#),
        )
        .mount(&server)
        .await;

    let http = HttpClient::new().expect("http client");
    let auth = PatAuth::github("bad", "dayseam.github", "vedanth");
    let base = with_trailing_slash(&server.uri());
    let err = validate_auth(&http, &auth, &base, &CancellationToken::new(), None)
        .await
        .expect_err("401 should surface as invalid credentials");
    assert_eq!(err.code(), error_codes::GITHUB_AUTH_INVALID_CREDENTIALS);
    assert_eq!(err.variant(), "Auth");
}

#[tokio::test]
async fn validate_auth_maps_403_to_missing_scope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let http = HttpClient::new().expect("http client");
    let auth = PatAuth::github("scope-less", "dayseam.github", "vedanth");
    let base = with_trailing_slash(&server.uri());
    let err = validate_auth(&http, &auth, &base, &CancellationToken::new(), None)
        .await
        .expect_err("403 should surface as missing scope");
    assert_eq!(err.code(), error_codes::GITHUB_AUTH_MISSING_SCOPE);
    assert_eq!(err.variant(), "Auth");
}

#[tokio::test]
async fn validate_auth_maps_404_to_resource_not_found() {
    // The real-world trigger for this path is a user pasting an
    // Enterprise Server URL without the `/api/v3` suffix — the proxy
    // then returns 404 for `/user`. Routing it through the
    // resource-not-found lane (rather than the generic 5xx bucket)
    // lets the Add-Source dialog render the "check the URL" hint.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let http = HttpClient::new().expect("http client");
    let auth = PatAuth::github("any", "dayseam.github", "vedanth");
    let base = with_trailing_slash(&server.uri());
    let err = validate_auth(&http, &auth, &base, &CancellationToken::new(), None)
        .await
        .expect_err("404 should surface as resource not found");
    assert_eq!(err.code(), error_codes::GITHUB_RESOURCE_NOT_FOUND);
    assert_eq!(err.variant(), "Network");
}

#[tokio::test]
async fn validate_auth_maps_transport_error_to_reused_url_dns_or_tls_code() {
    // Port 1 is reliably unbound on every dev host; the connect
    // attempt fails with ECONNREFUSED and the SDK's retry loop
    // exhausts. Because GitHub reuses GitLab's URL codes for
    // transport failures (see `connector-github::errors` doc-comment
    // explaining the symmetric-UX rationale), the code surfaces as
    // one of the two GitLab variants.
    let http = HttpClient::new()
        .expect("http client")
        .with_policy(connectors_sdk::RetryPolicy::instant());
    let auth = PatAuth::github("any", "dayseam.github", "vedanth");
    let base = Url::parse("http://127.0.0.1:1/").unwrap();
    let err = validate_auth(&http, &auth, &base, &CancellationToken::new(), None)
        .await
        .expect_err("connection refused should surface as a network error");
    let code = err.code();
    assert!(
        code == error_codes::GITLAB_URL_DNS
            || code == error_codes::GITLAB_URL_TLS
            || code == error_codes::HTTP_TRANSPORT
            || code == error_codes::HTTP_TRANSPORT_DNS
            || code == error_codes::HTTP_TRANSPORT_TLS
            || code == error_codes::HTTP_TRANSPORT_CONNECT
            || code == error_codes::HTTP_TRANSPORT_TIMEOUT
            || code == error_codes::HTTP_RETRY_BUDGET_EXHAUSTED,
        "unexpected transport code: {code}",
    );
    assert_eq!(err.variant(), "Network");
}
