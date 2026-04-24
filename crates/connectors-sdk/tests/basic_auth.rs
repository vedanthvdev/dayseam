//! End-to-end behaviour of [`connectors_sdk::BasicAuth`] under a real
//! wiremock HTTP server.
//!
//! The unit tests in `src/auth.rs::tests` prove the header is shaped
//! right in isolation. This file proves the same thing through the
//! full `HttpClient::send_with_retry` pipeline — the same pipeline the
//! Jira and Confluence walkers in DAY-77 / DAY-80 will use — so a
//! regression in either layer (request-builder hooking, header copy)
//! surfaces here before it silently corrupts the live walkers.
//!
//! Per CORR-01 (Phase 3 review), 401 / 403 classification is each
//! connector's responsibility — not `HttpClient`'s. This file therefore
//! only asserts that:
//!
//! 1. `BasicAuth::atlassian` attaches an `Authorization: Basic …`
//!    header the wiremock server can echo back.
//! 2. 401 / 403 responses flow through `send_with_retry` as raw
//!    [`reqwest::Response`] objects (not pre-classified
//!    `DayseamError::Network` blobs). `connector-atlassian-common` in
//!    DAY-75 owns the mapping from those responses to
//!    `DayseamError::Auth { code: atlassian.auth.* }`.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;

use connectors_sdk::{AuthStrategy, BasicAuth, HttpClient, RetryPolicy};
use dayseam_events::{RunId, RunStreams};
use reqwest::StatusCode;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> HttpClient {
    HttpClient::new()
        .expect("build http client")
        .with_policy(RetryPolicy::instant())
}

#[tokio::test]
async fn basic_auth_attaches_authorization_header_on_live_request() {
    let server = MockServer::start().await;

    let email = "user@company.com";
    let token = "api-token-123";
    let expected = format!(
        "Basic {}",
        BASE64_STANDARD.encode(format!("{email}:{token}"))
    );

    // The wiremock matcher is the assertion: if `BasicAuth` fails to
    // attach the header, `expect(1)` below trips.
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .and(header("Authorization", expected.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accountId": "abc-123",
            "emailAddress": email,
            "displayName": "Test User",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (_prx, _lrx)) = streams.split();
    let cancel = CancellationToken::new();

    let auth = BasicAuth::atlassian(email, token, "dayseam.atlassian", "acme");
    let http = client();

    let url = format!("{}/rest/api/3/myself", server.uri());
    let req = http.reqwest().get(url);
    let req = auth.authenticate(req).await.expect("authenticate ok");

    let res = http
        .send(req, &cancel, Some(&progress_tx), Some(&log_tx))
        .await
        .expect("send ok");
    assert_eq!(res.status(), StatusCode::OK);
}

/// Per CORR-01: `HttpClient::send_with_retry` must **not** auto-classify
/// 401. It returns the raw [`reqwest::Response`] and lets the Atlassian
/// classifier in DAY-75 decide the error code. A regression that
/// promotes the response into `DayseamError::Network` here would, at
/// runtime, hide the "reconnect" UI path the way the Phase-3 GitLab
/// CORR-01 bug did.
#[tokio::test]
async fn basic_auth_401_surfaces_as_raw_response_not_pre_classified_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthenticated"))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (_prx, _lrx)) = streams.split();
    let cancel = CancellationToken::new();

    let auth = BasicAuth::atlassian("u@e.com", "bad-token", "svc", "acct");
    let http = client();

    let url = format!("{}/rest/api/3/myself", server.uri());
    let req = http.reqwest().get(url);
    let req = auth.authenticate(req).await.expect("authenticate ok");

    let res = http
        .send(req, &cancel, Some(&progress_tx), Some(&log_tx))
        .await
        .expect("401 is not an HttpClient-level error");
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "401 must reach the caller as a response, not be swallowed into an error"
    );
}

/// 403 is the other half of the Atlassian auth-failure pair (the token
/// authenticates but the tenant denied the scope). Same CORR-01
/// invariant as 401: the response flows through, the connector classifies.
#[tokio::test]
async fn basic_auth_403_surfaces_as_raw_response_not_pre_classified_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (_prx, _lrx)) = streams.split();
    let cancel = CancellationToken::new();

    let auth = BasicAuth::atlassian("u@e.com", "scoped-down-token", "svc", "acct");
    let http = client();

    let url = format!("{}/rest/api/3/myself", server.uri());
    let req = http.reqwest().get(url);
    let req = auth.authenticate(req).await.expect("authenticate ok");

    let res = http
        .send(req, &cancel, Some(&progress_tx), Some(&log_tx))
        .await
        .expect("403 is not an HttpClient-level error");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// Two `BasicAuth` instances pointing at the **same** keychain row
/// (shared-PAT mode: one Atlassian PAT unlocks both Jira and
/// Confluence) must produce byte-equal `Authorization` headers for the
/// same live token. This is the on-the-wire half of the shared-PAT
/// invariant the unit test `basic_auth_same_keychain_handle_produces_equal_descriptors`
/// asserts at the descriptor level.
#[tokio::test]
async fn basic_auth_shared_handle_produces_equal_headers_live() {
    let server = MockServer::start().await;
    let email = "shared@company.com";
    let token = "shared-token";
    let expected = format!(
        "Basic {}",
        BASE64_STANDARD.encode(format!("{email}:{token}"))
    );

    // Two different endpoints (one each for Jira + Confluence) both
    // see the same Authorization header. Matcher = assertion.
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .and(header("Authorization", expected.as_str()))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/users/current"))
        .and(header("Authorization", expected.as_str()))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let jira = BasicAuth::atlassian(email, token, "dayseam.atlassian", "acme");
    let confluence = BasicAuth::atlassian(email, token, "dayseam.atlassian", "acme");
    let http = client();

    let jira_req = jira
        .authenticate(
            http.reqwest()
                .get(format!("{}/rest/api/3/myself", server.uri())),
        )
        .await
        .expect("jira auth ok");
    let conf_req = confluence
        .authenticate(
            http.reqwest()
                .get(format!("{}/wiki/api/v2/users/current", server.uri())),
        )
        .await
        .expect("confluence auth ok");

    let streams = RunStreams::new(RunId::new());
    let ((ptx, ltx), _) = streams.split();
    let cancel = CancellationToken::new();
    assert_eq!(
        http.send(jira_req, &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("jira ok")
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        http.send(conf_req, &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("confluence ok")
            .status(),
        StatusCode::OK
    );
}

/// Separate-PAT mode: two `BasicAuth` instances with different
/// keychain rows (different service accounts, or different tenants)
/// produce different `Authorization` headers. A regression here would
/// let one product's PAT silently leak into the other product's
/// request — the exact failure mode the DAY-73 spike flagged as the
/// "separate-tenant Atlassian" risk.
#[tokio::test]
async fn basic_auth_separate_handles_produce_distinct_headers_live() {
    let server = MockServer::start().await;

    let jira_email = "jira-bot@company.com";
    let jira_token = "jira-token";
    let jira_header = format!(
        "Basic {}",
        BASE64_STANDARD.encode(format!("{jira_email}:{jira_token}"))
    );

    let conf_email = "confluence-bot@company.com";
    let conf_token = "confluence-token";
    let conf_header = format!(
        "Basic {}",
        BASE64_STANDARD.encode(format!("{conf_email}:{conf_token}"))
    );

    assert_ne!(
        jira_header, conf_header,
        "test precondition: separate PATs must differ at the header level"
    );

    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .and(header("Authorization", jira_header.as_str()))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/users/current"))
        .and(header("Authorization", conf_header.as_str()))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let jira = BasicAuth::atlassian(jira_email, jira_token, "dayseam.atlassian", "acme-jira-bot");
    let confluence = BasicAuth::atlassian(
        conf_email,
        conf_token,
        "dayseam.atlassian",
        "acme-confluence-bot",
    );
    let http = client();

    let streams = RunStreams::new(RunId::new());
    let ((ptx, ltx), _) = streams.split();
    let cancel = CancellationToken::new();

    let jira_req = jira
        .authenticate(
            http.reqwest()
                .get(format!("{}/rest/api/3/myself", server.uri())),
        )
        .await
        .expect("jira auth ok");
    let conf_req = confluence
        .authenticate(
            http.reqwest()
                .get(format!("{}/wiki/api/v2/users/current", server.uri())),
        )
        .await
        .expect("confluence auth ok");

    assert_eq!(
        http.send(jira_req, &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("jira ok")
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        http.send(conf_req, &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("confluence ok")
            .status(),
        StatusCode::OK
    );
}
