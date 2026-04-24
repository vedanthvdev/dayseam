//! End-to-end retry behaviour of [`connectors_sdk::HttpClient`].
//!
//! The wiremock server is the only "upstream" that matters here. Each
//! test asserts three things:
//!
//! 1. The client actually retried the configured number of times.
//! 2. The retry loop emitted a progress event per backoff (the "never
//!    fail silently" rule).
//! 3. The final response is either a success or a well-typed
//!    `DayseamError` — no silent swallowing.

use std::time::Duration;

use connectors_sdk::{HttpClient, RetryPolicy};
use dayseam_core::{DayseamError, ProgressPhase};
use dayseam_events::{RunId, RunStreams};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn drain_progress(
    mut rx: dayseam_events::ProgressReceiver,
) -> Vec<dayseam_events::ProgressEvent> {
    let mut out = Vec::new();
    while let Some(evt) = rx.recv().await {
        out.push(evt);
    }
    out
}

#[tokio::test]
async fn client_retries_until_success_after_429s() {
    let server = MockServer::start().await;

    // First two requests get 429, third gets 200.
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let client = HttpClient::new()
        .expect("build")
        .with_policy(RetryPolicy::instant());
    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (progress_rx, _log_rx)) = streams.split();
    let cancel = CancellationToken::new();

    let res = client
        .send(
            client.reqwest().get(server.uri() + "/flaky"),
            &cancel,
            Some(&progress_tx),
            Some(&log_tx),
        )
        .await
        .expect("eventually succeeds");
    assert_eq!(res.status(), 200);

    // Close senders so `drain_progress` can terminate.
    drop(progress_tx);
    drop(log_tx);
    let events = drain_progress(progress_rx).await;
    assert_eq!(
        events.len(),
        2,
        "expected one InProgress event per retry, got {events:#?}"
    );
    assert!(matches!(events[0].phase, ProgressPhase::InProgress { .. }));
    assert!(matches!(events[1].phase, ProgressPhase::InProgress { .. }));
}

#[tokio::test]
async fn client_gives_up_with_rate_limited_error_after_budget_exhausted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/always429"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .mount(&server)
        .await;

    let client = HttpClient::new().expect("build").with_policy(RetryPolicy {
        max_attempts: 3,
        base_backoff: Duration::from_millis(0),
        max_backoff: Duration::from_millis(0),
        jitter_frac: 0.0,
    });
    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), _rx) = streams.split();

    let err = client
        .send(
            client.reqwest().get(server.uri() + "/always429"),
            &CancellationToken::new(),
            Some(&progress_tx),
            Some(&log_tx),
        )
        .await
        .expect_err("should give up");
    match err {
        DayseamError::RateLimited {
            retry_after_secs, ..
        } => assert_eq!(retry_after_secs, 1),
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn client_retries_5xx_and_eventually_returns_network_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/always500"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let client = HttpClient::new()
        .expect("build")
        .with_policy(RetryPolicy::instant());
    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), _rx) = streams.split();

    let err = client
        .send(
            client.reqwest().get(server.uri() + "/always500"),
            &CancellationToken::new(),
            Some(&progress_tx),
            Some(&log_tx),
        )
        .await
        .expect_err("should give up");
    assert!(matches!(err, DayseamError::Network { .. }));
    assert_eq!(
        err.code(),
        dayseam_core::error_codes::HTTP_RETRY_BUDGET_EXHAUSTED
    );
}

#[tokio::test]
async fn non_retriable_status_returns_response_without_retries() {
    // Contract (Phase 3 CORR-01 fix): non-retriable non-success statuses
    // (4xx except 429) return `Ok(res)` so the caller can classify the
    // status with resource-specific knowledge. The generic SDK does not
    // own "what does a 401 mean for this resource" — the connector does.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let client = HttpClient::new()
        .expect("build")
        .with_policy(RetryPolicy::instant());
    let streams = RunStreams::new(RunId::new());
    let ((progress_tx, log_tx), (progress_rx, _log_rx)) = streams.split();

    let res = client
        .send(
            client.reqwest().get(server.uri() + "/gone"),
            &CancellationToken::new(),
            Some(&progress_tx),
            Some(&log_tx),
        )
        .await
        .expect("404 is returned as Ok(res); callers map the status themselves");
    assert_eq!(res.status(), 404);

    drop(progress_tx);
    drop(log_tx);
    let events = drain_progress(progress_rx).await;
    assert!(
        events.is_empty(),
        "non-retriable non-success must not emit InProgress events"
    );
}

#[tokio::test]
async fn unreachable_host_surfaces_transport_connect_with_hostname_in_message() {
    // DAY-125: users hit `http.transport` when the VPN drops out from
    // under a private GitLab instance, and the generic message gives
    // them nothing actionable. The fix: classify the terminal
    // `reqwest::Error` into `http.transport.connect` (still prefixed
    // `http.transport.*` so existing log parsers keep matching) and
    // splice the host into the message so "couldn't reach
    // `git.modulrfinance.io`" appears in the error card, pointing the
    // user straight at their VPN.
    //
    // Port 1 is reliably unbound on every dev host, so this exercises
    // the connect-refused branch of `reqwest::Error` without needing
    // network access.
    let client = HttpClient::new()
        .expect("build")
        .with_policy(RetryPolicy::instant());
    let cancel = CancellationToken::new();
    let err = client
        .send(
            client.reqwest().get("http://127.0.0.1:1/"),
            &cancel,
            None,
            None,
        )
        .await
        .expect_err("connect refused must surface as a transport error");
    match &err {
        DayseamError::Network { code, message } => {
            assert_eq!(
                code,
                dayseam_core::error_codes::HTTP_TRANSPORT_CONNECT,
                "unexpected code; full error = {err:?}",
            );
            assert!(
                message.contains("127.0.0.1"),
                "expected host in message, got `{message}`",
            );
            assert!(
                message.contains("couldn't reach"),
                "expected 'couldn't reach' prefix, got `{message}`",
            );
        }
        other => panic!("expected Network error, got {other:?}"),
    }
}

#[tokio::test]
async fn unresolvable_host_surfaces_transport_dns_with_hostname_in_message() {
    // The complement to the connect-refused test: a host that cannot
    // resolve (`.invalid` is reserved by RFC 6761 for precisely this
    // use case — it must never be registered) must classify as
    // `http.transport.dns` rather than `.connect`, because the
    // remedies differ (check DNS / VPN vs. check firewall / service).
    //
    // Skipped on CI runners whose resolvers synthesise A records for
    // unknown names (some aggressive corporate DNS does this); the
    // assertion accepts either the DNS sub-code or — if the resolver
    // coughed up *some* address that then refused — the connect
    // sub-code, because what we care about here is "not the generic
    // HTTP_TRANSPORT anymore". The hostname-in-message invariant
    // holds in both branches.
    let client = HttpClient::new()
        .expect("build")
        .with_policy(RetryPolicy::instant());
    let cancel = CancellationToken::new();
    let err = client
        .send(
            client
                .reqwest()
                .get("http://dayseam-nonexistent-host.invalid/"),
            &cancel,
            None,
            None,
        )
        .await
        .expect_err("unresolvable host must surface as a transport error");
    match &err {
        DayseamError::Network { code, message } => {
            let code = code.as_str();
            assert!(
                code == dayseam_core::error_codes::HTTP_TRANSPORT_DNS
                    || code == dayseam_core::error_codes::HTTP_TRANSPORT_CONNECT,
                "expected dns or connect sub-code, got `{code}`",
            );
            assert!(
                message.contains("dayseam-nonexistent-host.invalid"),
                "expected host in message, got `{message}`",
            );
        }
        other => panic!("expected Network error, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_error_message_does_not_leak_url_userinfo() {
    // DAY-129 `test10`: the security review on DAY-125 flagged a
    // theoretical PAT-leak concern — if a user pastes a URL with
    // embedded credentials (`https://user:pat@gitlab.example.com`)
    // and the transport layer fails, does the surfaced error
    // message render those credentials? `reqwest` strips userinfo
    // from `url()` before the error's `Display` runs (via its
    // internal `extract_authority` invariant); this test pins the
    // invariant by forcing a connect-refused on a URL carrying
    // userinfo and asserting neither the host-splice nor the
    // trailing detail leaks the password. A future dep bump that
    // regressed `reqwest` here would fail this test loudly, giving
    // us a chance to add our own scrub before the leaked bytes
    // reach a log.
    let client = HttpClient::new()
        .expect("build")
        .with_policy(RetryPolicy::instant());
    let cancel = CancellationToken::new();
    let err = client
        .send(
            client
                .reqwest()
                .get("http://alice:super-secret-pat@127.0.0.1:1/probe"),
            &cancel,
            None,
            None,
        )
        .await
        .expect_err("connect refused on userinfo URL must still surface as a transport error");
    let DayseamError::Network { message, .. } = &err else {
        panic!("expected Network error, got {err:?}");
    };
    assert!(
        !message.contains("super-secret-pat"),
        "PAT leaked into transport error message: `{message}`",
    );
    assert!(
        !message.contains("alice:"),
        "username:password pair leaked into transport error message: `{message}`",
    );
    assert!(
        message.contains("127.0.0.1"),
        "host must still appear in the message, got `{message}`",
    );
}

#[tokio::test]
async fn status_401_and_403_return_response_so_caller_can_classify() {
    // CORR-01 regression: before the fix, the SDK collapsed 401/403 into
    // `DayseamError::Network { code: "http.transport" }`, which broke the
    // Reconnect error-card contract in `connector-gitlab` because the UI
    // keys on `gitlab.auth.invalid_token` / `gitlab.auth.missing_scope`.
    // The walker's `map_status` is the only caller qualified to say what
    // 401 or 403 means for the GitLab Events API; the SDK must hand it
    // the raw response.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/401"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/403"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;

    let client = HttpClient::new()
        .expect("build")
        .with_policy(RetryPolicy::instant());

    let cancel = CancellationToken::new();
    let res = client
        .send(
            client.reqwest().get(server.uri() + "/401"),
            &cancel,
            None,
            None,
        )
        .await
        .expect("401 is returned as Ok(res)");
    assert_eq!(res.status(), 401);

    let res = client
        .send(
            client.reqwest().get(server.uri() + "/403"),
            &cancel,
            None,
            None,
        )
        .await
        .expect("403 is returned as Ok(res)");
    assert_eq!(res.status(), 403);
}
