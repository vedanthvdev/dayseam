//! End-to-end tests for [`connector_jira::validate_auth`] +
//! [`connector_jira::list_identities`].
//!
//! The common crate's own tests prove `discover_cloud` classifies each
//! HTTP status correctly; these tests prove the Jira-crate wrapper
//! preserves that classification end-to-end. If a future refactor
//! widens [`connector_jira::validate_auth`] to do extra work (e.g. a
//! /serverInfo probe for Jira DC support), the tests here are the
//! first to flip red.
//!
//! Covers the invariants listed in DAY-76 §Task 4:
//! * `validate_auth_200` — happy path, account triple round-trips.
//! * `validate_auth_401` — `atlassian.auth.invalid_credentials`.
//! * `validate_auth_403` — `atlassian.auth.missing_scope`.
//! * `list_identities_seeds_one_row` — integration with the common
//!   seed helper.

use connector_jira::{list_identities, validate_auth};
use connectors_sdk::{BasicAuth, HttpClient, RetryPolicy};
use dayseam_core::{error_codes, DayseamError, SourceIdentityKind};
use dayseam_events::{RunId, RunStreams};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http() -> HttpClient {
    HttpClient::new()
        .expect("build http client")
        .with_policy(RetryPolicy::instant())
}

fn auth() -> BasicAuth {
    BasicAuth::atlassian(
        "vedanth@acme.com",
        "api-token-xyz",
        "dayseam.atlassian",
        "vedanth@acme.com",
    )
}

fn workspace(server: &MockServer) -> Url {
    // `Url::join` on a bare server URI drops the last path segment —
    // match the real production flow where `JiraConfig::from_raw`
    // pads the trailing slash.
    Url::parse(&format!("{}/", server.uri())).expect("mock uri parses")
}

#[tokio::test]
async fn validate_auth_200_returns_account_info() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .and(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accountId": "5d53f3cbc6b9320d9ea5bdc2",
            "displayName": "Vedanth Vasudev",
            "emailAddress": "vedanth@acme.com",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((_ptx, ltx), (_, _)) = streams.split();
    let cancel = CancellationToken::new();

    let cloud = validate_auth(&http(), &auth(), &workspace(&server), &cancel, Some(&ltx))
        .await
        .expect("200 must yield AtlassianCloud");

    assert_eq!(cloud.account.account_id, "5d53f3cbc6b9320d9ea5bdc2");
    assert_eq!(cloud.account.display_name, "Vedanth Vasudev");
    assert_eq!(cloud.account.email.as_deref(), Some("vedanth@acme.com"));
    // Basic-auth flow: `cloud_id` is deliberately absent — the
    // OAuth-era opaque UUID lives at `/_edge/tenant_info` which
    // Basic credentials cannot reach. The identity-seed layer does
    // not care about the UUID; it keys off `accountId`.
    assert!(cloud.account.cloud_id.is_none());
}

#[tokio::test]
async fn validate_auth_401_maps_to_atlassian_auth_invalid_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthenticated"))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let err = validate_auth(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect_err("401 must surface as DayseamError");

    assert_eq!(err.code(), error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS);
    assert!(matches!(err, DayseamError::Auth { .. }));
}

#[tokio::test]
async fn validate_auth_403_maps_to_atlassian_auth_missing_scope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let err = validate_auth(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect_err("403 must surface as DayseamError");

    assert_eq!(err.code(), error_codes::ATLASSIAN_AUTH_MISSING_SCOPE);
    assert!(matches!(err, DayseamError::Auth { .. }));
}

#[tokio::test]
async fn validate_auth_404_maps_to_atlassian_cloud_resource_not_found() {
    // Classic "user typed `foo.atlassian.net` when they meant
    // `bar.atlassian.net`" case. The dialog renders a "workspace URL
    // is likely mistyped" toast based on this code; regressing the
    // mapping would silently break that UX.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let err = validate_auth(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect_err("404 must surface as DayseamError");

    assert_eq!(err.code(), error_codes::ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND);
    assert!(matches!(err, DayseamError::Network { .. }));
}

#[tokio::test]
async fn list_identities_seeds_one_row_per_successful_probe() {
    // Round-trip the happy path of the IPC add-source flow: probe
    // the workspace, then seed the identity row the report walker
    // will filter by. The DAY-71 post-mortem was exactly this
    // sequence failing silently on GitLab; keep the end-to-end
    // assertion here so the Atlassian side can't regress in the
    // same way.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accountId": "5d53f3cbc6b9320d9ea5bdc2",
            "displayName": "Vedanth Vasudev",
            "emailAddress": "vedanth@acme.com",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let cloud = validate_auth(&http(), &auth(), &workspace(&server), &cancel, None)
        .await
        .expect("validation succeeds");

    let source_id = Uuid::new_v4();
    let person_id = Uuid::new_v4();
    let identities =
        list_identities(&cloud.account, source_id, person_id, None).expect("identity seed is pure");

    assert_eq!(identities.len(), 1);
    let row = &identities[0];
    assert_eq!(row.person_id, person_id);
    assert_eq!(row.source_id, Some(source_id));
    assert_eq!(row.kind, SourceIdentityKind::AtlassianAccountId);
    assert_eq!(row.external_actor_id, "5d53f3cbc6b9320d9ea5bdc2");
}
