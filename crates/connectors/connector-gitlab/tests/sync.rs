//! End-to-end wiremock-driven tests for the GitLab connector.
//!
//! The plan's Task 1 test matrix is:
//!
//! 1. `validate_pat` — 200 / 401 / 403 / transport error.
//! 2. Events walker — single-page happy path produces deterministic
//!    output.
//! 3. Events walker — day-window filter drops rows outside the UTC
//!    bounds of the local day.
//! 4. Events walker — identity filter drops rows whose `author.id`
//!    is not in the provided [`dayseam_core::SourceIdentity`] list.
//! 5. Events walker — an unknown `target_type` does not crash the
//!    walk; the row is silently dropped and its neighbours are
//!    processed.
//!
//! These exercise the full authn → HTTP → pagination → normalise →
//! identity-filter path so any regression touching the connector's
//! contract with GitLab trips at least one of them.

use std::sync::Arc;

use chrono::{FixedOffset, NaiveDate};
use connector_gitlab::{auth::validate_pat, walk::walk_day};
use connectors_sdk::{AuthStrategy, HttpClient, PatAuth, RetryPolicy};
use dayseam_core::{
    error_codes, ActivityKind, DayseamError, SourceId, SourceIdentity, SourceIdentityKind,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- validate_pat (4 auth cases) ------------------------------------------

#[tokio::test]
async fn validate_pat_returns_user_on_200() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .and(header("PRIVATE-TOKEN", "good-pat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": 17,
            "username": "vedanth",
            "name": "Vedanth",
            "avatar_url": "https://…",
            "state": "active"
        })))
        .mount(&server)
        .await;

    let info = validate_pat(&server.uri(), "good-pat")
        .await
        .expect("200 should return user info");
    assert_eq!(info.id, 17);
    assert_eq!(info.username, "vedanth");
}

#[tokio::test]
async fn validate_pat_maps_401_to_invalid_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = validate_pat(&server.uri(), "bad-pat")
        .await
        .expect_err("401 should surface as invalid token");
    assert_eq!(err.code(), error_codes::GITLAB_AUTH_INVALID_TOKEN);
    assert_eq!(err.variant(), "Auth");
}

#[tokio::test]
async fn validate_pat_maps_403_to_missing_scope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let err = validate_pat(&server.uri(), "scoped-pat")
        .await
        .expect_err("403 should surface as missing scope");
    assert_eq!(err.code(), error_codes::GITLAB_AUTH_MISSING_SCOPE);
    assert_eq!(err.variant(), "Auth");
}

#[tokio::test]
async fn validate_pat_maps_transport_error_to_http_transport_subcode() {
    // Port 1 is reliably unbound on Unix; the connect attempt fails
    // with ECONNREFUSED. DAY-129: the PAT-validation lane is now
    // routed through `HttpClient::send` rather than its own
    // `reqwest::Client` + legacy `map_transport_error`, so the
    // surfaced code is one of the SDK's `http.transport.*` sub-codes
    // instead of the dropped `gitlab.url.dns` / `gitlab.url.tls`
    // codes. Connect-refused on a bound localhost port lands on
    // `http.transport.connect`; we accept the full transport family
    // to keep the test stable if the SDK classifier sharpens its
    // fragment list.
    let err = validate_pat("http://127.0.0.1:1", "any-pat")
        .await
        .expect_err("connection refused should surface as a network error");
    let code = err.code();
    assert!(
        code.starts_with("http.transport"),
        "unexpected transport code: {code}",
    );
    assert_eq!(err.variant(), "Network");
    // The message now names the host (thanks to
    // `format_transport_error`) so the dialog's generic error
    // surface has something actionable even without bespoke copy.
    match err {
        DayseamError::Network { ref message, .. } => {
            assert!(
                message.contains("127.0.0.1"),
                "expected host in transport error message, got `{message}`",
            );
        }
        other => panic!("expected Network variant, got {other:?}"),
    }
}

// ---- walk_day -------------------------------------------------------------

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 4, 19).unwrap()
}

fn source_id() -> SourceId {
    Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
}

fn http_for_tests() -> HttpClient {
    HttpClient::new()
        .expect("HttpClient::new")
        .with_policy(RetryPolicy::instant())
}

fn auth_for_tests() -> Arc<dyn AuthStrategy> {
    Arc::new(PatAuth::gitlab("test-pat", "dayseam.gitlab", "acme"))
}

fn identity_for_user(user_id: i64) -> Vec<SourceIdentity> {
    vec![SourceIdentity {
        id: Uuid::new_v4(),
        person_id: Uuid::new_v4(),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: user_id.to_string(),
        source_id: Some(source_id()),
    }]
}

#[tokio::test]
async fn walk_day_returns_normalised_events_for_happy_path() {
    let server = MockServer::start().await;

    let events_json = serde_json::json!([
        {
            "id": 1001,
            "action_name": "pushed to",
            "target_type": null,
            "target_iid": null,
            "target_id": null,
            "target_title": null,
            "project_id": 42,
            "created_at": "2026-04-19T12:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" },
            "push_data": { "ref": "refs/heads/main", "commit_count": 3, "commit_to": "abc123" }
        },
        {
            "id": 1002,
            "action_name": "opened",
            "target_type": "MergeRequest",
            "target_iid": 11,
            "target_id": 2001,
            "target_title": "Add payments slice",
            "project_id": 42,
            "created_at": "2026-04-19T12:05:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        }
    ]);

    Mock::given(method("GET"))
        .and(path("/api/v4/users/17/events"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(events_json))
        .mount(&server)
        .await;

    let http = http_for_tests();
    let outcome = walk_day(
        &http,
        auth_for_tests(),
        &server.uri(),
        17,
        source_id(),
        &identity_for_user(17),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("happy path should succeed");

    assert_eq!(outcome.events.len(), 2, "two rows in, two events out");
    assert_eq!(outcome.fetched_count, 2);
    assert_eq!(outcome.filtered_by_identity, 0);
    // Events should be oldest-first after the walker's sort.
    assert_eq!(outcome.events[0].kind, ActivityKind::CommitAuthored);
    assert_eq!(outcome.events[1].kind, ActivityKind::MrOpened);
}

#[tokio::test]
async fn walk_day_filters_events_outside_local_day() {
    let server = MockServer::start().await;
    // One event on 2026-04-18 (outside window), one on 2026-04-19
    // (inside window).
    let events_json = serde_json::json!([
        {
            "id": 2001,
            "action_name": "opened",
            "target_type": "MergeRequest",
            "target_iid": 11,
            "target_id": 2001,
            "target_title": "yesterday's MR",
            "project_id": 42,
            "created_at": "2026-04-18T23:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        },
        {
            "id": 2002,
            "action_name": "opened",
            "target_type": "MergeRequest",
            "target_iid": 12,
            "target_id": 2002,
            "target_title": "today's MR",
            "project_id": 42,
            "created_at": "2026-04-19T12:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        }
    ]);
    Mock::given(method("GET"))
        .and(path("/api/v4/users/17/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(events_json))
        .mount(&server)
        .await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &server.uri(),
        17,
        source_id(),
        &identity_for_user(17),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("day-window filter should not error");

    assert_eq!(outcome.events.len(), 1, "only the in-window event survives");
    assert_eq!(outcome.filtered_by_date, 1);
    assert_eq!(outcome.events[0].title, "Opened MR: today's MR");
}

#[tokio::test]
async fn walk_day_filters_events_by_author_id() {
    let server = MockServer::start().await;
    // Two events on the same day — one from user 17 (us), one from
    // user 99 (a teammate). The v0.1 identity filter keeps only the
    // first because source_identities is `[user_id: 17]`.
    let events_json = serde_json::json!([
        {
            "id": 3001,
            "action_name": "opened",
            "target_type": "MergeRequest",
            "target_iid": 11,
            "target_id": 3001,
            "target_title": "mine",
            "project_id": 42,
            "created_at": "2026-04-19T10:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        },
        {
            "id": 3002,
            "action_name": "opened",
            "target_type": "MergeRequest",
            "target_iid": 12,
            "target_id": 3002,
            "target_title": "teammate",
            "project_id": 42,
            "created_at": "2026-04-19T10:30:00.000Z",
            "author_id": 99,
            "author": { "id": 99, "username": "teammate" }
        }
    ]);
    Mock::given(method("GET"))
        .and(path("/api/v4/users/17/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(events_json))
        .mount(&server)
        .await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &server.uri(),
        17,
        source_id(),
        &identity_for_user(17),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("identity filter should not error");

    assert_eq!(outcome.events.len(), 1);
    assert_eq!(outcome.filtered_by_identity, 1);
    assert_eq!(outcome.events[0].title, "Opened MR: mine");
}

#[tokio::test]
async fn walk_day_drops_unknown_target_type_without_error() {
    let server = MockServer::start().await;
    // A future GitLab release might add `"WikiPage"` as a target
    // type; the walker must keep walking and just drop that row.
    let events_json = serde_json::json!([
        {
            "id": 4001,
            "action_name": "edited",
            "target_type": "WikiPage",
            "target_iid": null,
            "target_id": 9001,
            "target_title": "Onboarding",
            "project_id": 42,
            "created_at": "2026-04-19T10:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        },
        {
            "id": 4002,
            "action_name": "opened",
            "target_type": "MergeRequest",
            "target_iid": 11,
            "target_id": 2001,
            "target_title": "real MR",
            "project_id": 42,
            "created_at": "2026-04-19T10:30:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        }
    ]);
    Mock::given(method("GET"))
        .and(path("/api/v4/users/17/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(events_json))
        .mount(&server)
        .await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &server.uri(),
        17,
        source_id(),
        &identity_for_user(17),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("schema-drift row must not abort the walk");

    assert_eq!(outcome.events.len(), 1);
    assert_eq!(outcome.dropped_by_shape, 1);
    assert_eq!(outcome.events[0].title, "Opened MR: real MR");
}

#[tokio::test]
async fn walk_day_surfaces_401_as_invalid_token_from_walker_path() {
    // CORR-01: before the Phase 3 fix, the SDK collapsed 401 into
    // `http.transport`, breaking the Reconnect error card. With the fix,
    // `HttpClient::send` hands the walker the raw response and the
    // walker's `map_status` routes 401 → `gitlab.auth.invalid_token`.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/17/events"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &server.uri(),
        17,
        source_id(),
        &identity_for_user(17),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect_err("401 during the walk must surface as an auth error");

    assert_eq!(
        err.code(),
        error_codes::GITLAB_AUTH_INVALID_TOKEN,
        "401 must surface as gitlab.auth.invalid_token so the Reconnect card keys on it; got {err:?}"
    );
    assert!(matches!(err, DayseamError::Auth { .. }));
}

#[tokio::test]
async fn walk_day_surfaces_403_as_missing_scope_from_walker_path() {
    // CORR-01 sibling of the 401 case. 403 = PAT authenticates but the
    // `read_api` scope is absent; the UI surfaces a dedicated copy for
    // this via `gitlab.auth.missing_scope`.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/17/events"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let err = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &server.uri(),
        17,
        source_id(),
        &identity_for_user(17),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect_err("403 during the walk must surface as an auth error");

    assert_eq!(
        err.code(),
        error_codes::GITLAB_AUTH_MISSING_SCOPE,
        "403 must surface as gitlab.auth.missing_scope so the scope-hint copy renders; got {err:?}"
    );
    assert!(matches!(err, DayseamError::Auth { .. }));
}
