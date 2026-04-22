//! End-to-end behaviour of [`connectors_sdk::PatAuth::github`] under a
//! real wiremock HTTP server.
//!
//! The unit tests in `src/auth.rs::tests` prove the header is shaped
//! right in isolation. This file proves the same thing through the
//! full [`HttpClient::send`] pipeline — the same pipeline the GitHub
//! walker in DAY-96 will use — so a regression in either layer
//! (request-builder hooking, header copy, retry-loop body consumption)
//! surfaces here before it silently corrupts the live walker.
//!
//! Per CORR-01 (Phase 3 review, restated in the DAY-94 plan), 401 / 403
//! classification is each connector's responsibility — not
//! [`HttpClient`]'s. This file therefore only asserts that:
//!
//! 1. [`PatAuth::github`] attaches an `Authorization: Bearer <token>`
//!    header the wiremock server can match on.
//! 2. 401 / 403 responses flow through [`HttpClient::send`] as raw
//!    [`reqwest::Response`] objects (not pre-classified
//!    `DayseamError::Auth` / `DayseamError::Network` blobs).
//!    `connector-github` in DAY-95 owns the mapping from those
//!    responses to `DayseamError::Auth { code: github.auth.* }`.
//! 3. Two instances pointing at the same keychain row produce
//!    byte-equal headers for the same live token (mirrors the
//!    Atlassian `basic_auth_shared_handle_produces_equal_headers_live`
//!    test — the GitHub analogue matters when a user connects the
//!    same service-account PAT to both github.com and a GitHub
//!    Enterprise Server host).

use connectors_sdk::{AuthStrategy, HttpClient, PatAuth, RetryPolicy};
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
async fn github_pat_attaches_authorization_header_on_live_request() {
    let server = MockServer::start().await;

    let token = "ghp_live_token_abc123";
    let expected = format!("Bearer {token}");

    // The wiremock matcher is the assertion: if `PatAuth::github` fails
    // to attach the header, `expect(1)` below trips.
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header("Authorization", expected.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": 12345_u64,
            "login": "octocat",
            "name": "The Octocat",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (_prx, _lrx)) = streams.split();
    let cancel = CancellationToken::new();

    let auth = PatAuth::github(token, "dayseam.github", "acme");
    let http = client();

    let url = format!("{}/user", server.uri());
    let req = http.reqwest().get(url);
    let req = auth.authenticate(req).await.expect("authenticate ok");

    let res = http
        .send(req, &cancel, Some(&progress_tx), Some(&log_tx))
        .await
        .expect("send ok");
    assert_eq!(res.status(), StatusCode::OK);
}

/// Per CORR-01: [`HttpClient::send`] must **not** auto-classify 401.
/// It returns the raw [`reqwest::Response`] and lets the GitHub
/// classifier in DAY-95 decide the error code. A regression that
/// promoted the response into `DayseamError::Network` here would, at
/// runtime, hide the "reconnect" UI path the way the Phase-3 GitLab
/// CORR-01 bug did — a class of failure the DAY-75 Atlassian shape
/// already pinned in [`basic_auth_401_surfaces_as_raw_response_…`](
/// crate::tests) and DAY-94 now re-pins for GitHub.
#[tokio::test]
async fn github_pat_401_surfaces_as_raw_response_not_pre_classified_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "message": "Bad credentials",
            "documentation_url": "https://docs.github.com/rest",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (_prx, _lrx)) = streams.split();
    let cancel = CancellationToken::new();

    let auth = PatAuth::github("ghp_bad_token", "dayseam.github", "acme");
    let http = client();

    let url = format!("{}/user", server.uri());
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

/// 403 is the other half of GitHub's auth-failure pair. Unlike Jira's
/// 403 (tenant-denied-scope) or GitLab's 403 (project-visibility),
/// GitHub uses 403 for **both** scope-miss (classic PAT missing
/// `repo` scope) and secondary rate-limits (too-fast unauthenticated
/// traffic); the body's `message` disambiguates. DAY-95's
/// `map_status` reads the body to pick between
/// `github.auth.missing_scope` and `github.rate_limited`. The
/// [`HttpClient`] layer stays out of that decision — which is what
/// this test pins.
#[tokio::test]
async fn github_pat_403_surfaces_as_raw_response_not_pre_classified_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "message": "Resource not accessible by integration",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (_prx, _lrx)) = streams.split();
    let cancel = CancellationToken::new();

    let auth = PatAuth::github("ghp_scoped_down_token", "dayseam.github", "acme");
    let http = client();

    let url = format!("{}/user", server.uri());
    let req = http.reqwest().get(url);
    let req = auth.authenticate(req).await.expect("authenticate ok");

    let res = http
        .send(req, &cancel, Some(&progress_tx), Some(&log_tx))
        .await
        .expect("403 is not an HttpClient-level error");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// Two `PatAuth::github` instances pointing at the **same** keychain
/// row (shared-PAT mode — the same service-account token connected to
/// both github.com and a GitHub Enterprise host) must produce
/// byte-equal `Authorization` headers for the same live token. This
/// is the on-the-wire half of the shared-handle invariant the unit
/// test `github_pat_same_keychain_handle_produces_equal_descriptors`
/// asserts at the descriptor level. The DAY-81 refcount guard hangs
/// off this invariant; a regression would let the same keychain row
/// be written twice.
#[tokio::test]
async fn github_pat_shared_handle_produces_equal_headers_live() {
    let server = MockServer::start().await;

    let token = "ghp_shared_service_account";
    let expected = format!("Bearer {token}");

    // Two different endpoints (one each for github.com + a GitHub
    // Enterprise host) both see the same Authorization header.
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header("Authorization", expected.as_str()))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .and(header("Authorization", expected.as_str()))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let dotcom = PatAuth::github(token, "dayseam.github", "acme");
    let enterprise = PatAuth::github(token, "dayseam.github", "acme");
    let http = client();

    let dotcom_req = dotcom
        .authenticate(http.reqwest().get(format!("{}/user", server.uri())))
        .await
        .expect("dotcom auth ok");
    let enterprise_req = enterprise
        .authenticate(http.reqwest().get(format!("{}/api/v3/user", server.uri())))
        .await
        .expect("enterprise auth ok");

    let streams = RunStreams::new(RunId::new());
    let ((ptx, ltx), _) = streams.split();
    let cancel = CancellationToken::new();

    assert_eq!(
        http.send(dotcom_req, &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("dotcom ok")
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        http.send(enterprise_req, &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("enterprise ok")
            .status(),
        StatusCode::OK
    );
}
