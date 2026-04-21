//! Wiremock-driven integration tests for
//! [`connector_atlassian_common::discover_cloud`].
//!
//! The unit tests in `src/errors.rs::tests` prove the classifier
//! routes each status to the right `AtlassianError` variant. This
//! file proves the end-to-end funnel: a real `HttpClient` hitting a
//! live wiremock endpoint, with a real [`BasicAuth`] attached,
//! surfaces the expected `DayseamError` codes — and, critically, the
//! `atlassian.auth.invalid_credentials` code DAY-74 deferred here per
//! CORR-01.

use connector_atlassian_common::discover_cloud;
use connectors_sdk::{BasicAuth, HttpClient, RetryPolicy};
use dayseam_core::{error_codes, DayseamError};
use dayseam_events::{RunId, RunStreams};
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http() -> HttpClient {
    HttpClient::new()
        .expect("build http client")
        .with_policy(RetryPolicy::instant())
}

fn auth() -> BasicAuth {
    BasicAuth::atlassian(
        "vedanth@modulrfinance.com",
        "api-token-xyz",
        "dayseam.atlassian",
        "vedanth@modulrfinance.com",
    )
}

fn workspace(server: &MockServer) -> Url {
    // `Url::join` needs a trailing slash or it strips the last path
    // segment.
    Url::parse(&format!("{}/", server.uri())).expect("mock uri parses")
}

#[tokio::test]
async fn discover_cloud_returns_account_triple_on_200() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .and(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accountId": "5d53f3cbc6b9320d9ea5bdc2",
            "displayName": "Vedanth Vasudev",
            "emailAddress": "vedanth@modulrfinance.com",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((_ptx, ltx), (_, _lrx)) = streams.split();
    let cancel = CancellationToken::new();

    let cloud = discover_cloud(&http(), &auth(), &workspace(&server), &cancel, Some(&ltx))
        .await
        .expect("discovery should succeed");

    assert_eq!(cloud.account.account_id, "5d53f3cbc6b9320d9ea5bdc2");
    assert_eq!(cloud.account.display_name, "Vedanth Vasudev");
    assert_eq!(
        cloud.account.email.as_deref(),
        Some("vedanth@modulrfinance.com")
    );
    // Under Basic auth, `cloud_id` is intentionally `None` — this is
    // the OAuth-era placeholder the plan reserved in DAY-73.
    assert!(cloud.account.cloud_id.is_none());
}

#[tokio::test]
async fn discover_cloud_maps_401_to_atlassian_auth_invalid_credentials() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthenticated"))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let err = discover_cloud(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect_err("401 should surface as error");

    assert_eq!(err.code(), error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS);
    assert!(matches!(err, DayseamError::Auth { .. }));
}

#[tokio::test]
async fn discover_cloud_maps_403_to_atlassian_auth_missing_scope() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let err = discover_cloud(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect_err("403 should surface as error");

    assert_eq!(err.code(), error_codes::ATLASSIAN_AUTH_MISSING_SCOPE);
    assert!(matches!(err, DayseamError::Auth { .. }));
}

#[tokio::test]
async fn discover_cloud_maps_404_to_atlassian_cloud_resource_not_found() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let err = discover_cloud(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect_err("404 should surface as error");

    assert_eq!(err.code(), error_codes::ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND);
    assert!(matches!(err, DayseamError::Network { .. }));
}

#[tokio::test]
async fn discover_cloud_rejects_malformed_account_id_with_atlassian_identity_code() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accountId": "",
            "displayName": "Ghost",
            "emailAddress": null,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let err = discover_cloud(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect_err("empty accountId should be rejected");

    assert_eq!(
        err.code(),
        error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID
    );
    assert!(matches!(err, DayseamError::UpstreamChanged { .. }));
}
