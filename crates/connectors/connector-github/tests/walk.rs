//! End-to-end wiremock-driven tests for the DAY-96 GitHub walker.
//!
//! The plan's DAY-96 matrix:
//!
//! 1. **401 / invalid credentials** — a `GET /users/:login/events` that
//!    comes back 401 fails the walk with `DayseamError::Auth` carrying
//!    [`dayseam_core::error_codes::GITHUB_AUTH_INVALID_CREDENTIALS`].
//! 2. **410 / account gone** — a 410 surfaces as
//!    [`dayseam_core::error_codes::GITHUB_RESOURCE_GONE`]. (GitHub
//!    returns 410 for fully deleted user accounts; the UI's
//!    Reconnect-card copy keys off this code.)
//! 3. **Self-filter** — an event whose `actor.id` is not the
//!    registered `GitHubUserId` is silently dropped.
//! 4. **Day window** — events with `created_at` outside the local
//!    day's UTC bounds are filtered via `filtered_by_date`.
//! 5. **Search dedup** — a PR that shows up in both the events stream
//!    and `/search/issues` surfaces exactly once; the events-stream
//!    row wins and the dedup counter increments.
//! 6. **Rapid-review collapse** — three review events on the same PR
//!    within a 60s window collapse into one `GitHubPullRequestReviewed`.
//! 7. **No identity — early bail** — no `GitHubUserId` identity in
//!    scope returns an empty outcome without issuing a request.
//!
//! Companion to the unit tests already pinned inline in
//! `events.rs` / `normalise.rs` / `rollup.rs` / `walk.rs`. Those pin
//! per-function contracts; the tests here pin the
//! authn → HTTP → paginate → normalise → rollup round-trip the
//! orchestrator will invoke.

use std::sync::Arc;

use chrono::{FixedOffset, NaiveDate};
use connector_github::walk::{walk_day, WalkOutcome};
use connectors_sdk::{AuthStrategy, HttpClient, PatAuth, RetryPolicy};
use dayseam_core::{
    error_codes, ActivityKind, DayseamError, SourceId, SourceIdentity, SourceIdentityKind,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- Test scaffolding ----------------------------------------------------

const SELF_LOGIN: &str = "vedanth";
const SELF_USER_ID: i64 = 17;
const OTHER_USER_ID: i64 = 99;

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()
}

fn source_id() -> SourceId {
    Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap()
}

fn http_for_tests() -> HttpClient {
    HttpClient::new()
        .expect("HttpClient::new")
        .with_policy(RetryPolicy::instant())
}

fn auth_for_tests() -> Arc<dyn AuthStrategy> {
    Arc::new(PatAuth::github("ghp-test", "dayseam.github", SELF_LOGIN))
}

fn self_identity_both() -> Vec<SourceIdentity> {
    vec![
        SourceIdentity {
            id: Uuid::new_v4(),
            person_id: Uuid::new_v4(),
            kind: SourceIdentityKind::GitHubUserId,
            external_actor_id: SELF_USER_ID.to_string(),
            source_id: Some(source_id()),
        },
        SourceIdentity {
            id: Uuid::new_v4(),
            person_id: Uuid::new_v4(),
            kind: SourceIdentityKind::GitHubLogin,
            external_actor_id: SELF_LOGIN.into(),
            source_id: Some(source_id()),
        },
    ]
}

fn api_base(server: &MockServer) -> Url {
    Url::parse(&format!("{}/", server.uri())).unwrap()
}

/// Mount an always-empty `/search/issues` stub so tests focused on the
/// events endpoint don't accidentally fail on an unmocked search call.
async fn mount_empty_search(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 0,
            "incomplete_results": false,
            "items": []
        })))
        .mount(server)
        .await;
}

fn pr_opened_event(
    event_id: &str,
    actor_id: i64,
    actor_login: &str,
    created_at: &str,
    repo_full: &str,
    number: u64,
    title: &str,
) -> serde_json::Value {
    let (owner, name) = repo_full.split_once('/').unwrap();
    json!({
        "id": event_id,
        "type": "PullRequestEvent",
        "actor": {
            "id": actor_id,
            "login": actor_login,
            "display_login": actor_login
        },
        "repo": {
            "id": 1,
            "name": repo_full,
            "url": format!("https://api.github.com/repos/{owner}/{name}")
        },
        "created_at": created_at,
        "payload": {
            "action": "opened",
            "number": number,
            "pull_request": {
                "id": number * 100,
                "number": number,
                "title": title,
                "state": "open",
                "html_url": format!("https://github.com/{repo_full}/pull/{number}"),
                "user": {
                    "id": actor_id,
                    "login": actor_login
                }
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn pr_review_event(
    event_id: &str,
    actor_id: i64,
    actor_login: &str,
    created_at: &str,
    repo_full: &str,
    number: u64,
    title: &str,
    state: &str,
) -> serde_json::Value {
    let (owner, name) = repo_full.split_once('/').unwrap();
    json!({
        "id": event_id,
        "type": "PullRequestReviewEvent",
        "actor": {
            "id": actor_id,
            "login": actor_login,
            "display_login": actor_login
        },
        "repo": {
            "id": 1,
            "name": repo_full,
            "url": format!("https://api.github.com/repos/{owner}/{name}")
        },
        "created_at": created_at,
        "payload": {
            "action": "submitted",
            "review": {
                "id": 90,
                "state": state,
                "html_url": format!("https://github.com/{repo_full}/pull/{number}#review"),
                "user": {
                    "id": actor_id,
                    "login": actor_login
                }
            },
            "pull_request": {
                "id": number * 100,
                "number": number,
                "title": title,
                "state": "open",
                "html_url": format!("https://github.com/{repo_full}/pull/{number}"),
                "user": {
                    "id": 5,
                    "login": "author"
                }
            }
        }
    })
}

async fn run_walk(server: &MockServer) -> Result<WalkOutcome, DayseamError> {
    walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &api_base(server),
        source_id(),
        &self_identity_both(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
}

// ---- 1. 401 on events endpoint ------------------------------------------

#[tokio::test]
async fn walk_day_maps_401_to_github_auth_invalid_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials",
            "documentation_url": "https://docs.github.com/rest"
        })))
        .mount(&server)
        .await;
    mount_empty_search(&server).await;

    let err = run_walk(&server).await.expect_err("401 must fail the walk");
    assert_eq!(err.code(), error_codes::GITHUB_AUTH_INVALID_CREDENTIALS);
    assert!(
        matches!(err, DayseamError::Auth { .. }),
        "expected Auth variant, got {err:?}"
    );
}

// ---- 2. 410 on events endpoint ------------------------------------------

#[tokio::test]
async fn walk_day_maps_410_to_github_resource_gone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(410).set_body_json(json!({
            "message": "This user's activity is no longer available."
        })))
        .mount(&server)
        .await;
    mount_empty_search(&server).await;

    let err = run_walk(&server).await.expect_err("410 must fail the walk");
    assert_eq!(err.code(), error_codes::GITHUB_RESOURCE_GONE);
}

// ---- 3. Self-filter ------------------------------------------------------

#[tokio::test]
async fn walk_day_drops_events_from_other_actors() {
    let server = MockServer::start().await;
    let events = json!([
        pr_opened_event(
            "evt-1",
            OTHER_USER_ID,
            "someone-else",
            "2026-04-20T10:00:00Z",
            "modulr/foo",
            42,
            "Random PR"
        ),
        pr_opened_event(
            "evt-2",
            SELF_USER_ID,
            SELF_LOGIN,
            "2026-04-20T11:00:00Z",
            "modulr/foo",
            43,
            "My PR"
        ),
    ]);
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(events))
        .mount(&server)
        .await;
    mount_empty_search(&server).await;

    let outcome = run_walk(&server).await.expect("self-filter walk succeeds");
    assert_eq!(outcome.events.len(), 1, "only the self-authored PR remains");
    assert_eq!(outcome.filtered_by_identity, 1);
    assert_eq!(
        outcome.events[0].kind,
        ActivityKind::GitHubPullRequestOpened
    );
    assert!(outcome.events[0].title.contains("My PR"));
}

// ---- 4. Day window filter ------------------------------------------------

#[tokio::test]
async fn walk_day_filters_events_outside_local_day_window() {
    let server = MockServer::start().await;
    // Three events: yesterday, today, tomorrow. Only the middle one
    // belongs in the walk for NaiveDate(2026-04-20) in UTC.
    let events = json!([
        pr_opened_event(
            "evt-today",
            SELF_USER_ID,
            SELF_LOGIN,
            "2026-04-20T10:00:00Z",
            "modulr/foo",
            1,
            "In window"
        ),
        pr_opened_event(
            "evt-yesterday",
            SELF_USER_ID,
            SELF_LOGIN,
            "2026-04-19T10:00:00Z",
            "modulr/foo",
            2,
            "Yesterday"
        ),
        pr_opened_event(
            "evt-tomorrow",
            SELF_USER_ID,
            SELF_LOGIN,
            "2026-04-21T10:00:00Z",
            "modulr/foo",
            3,
            "Tomorrow"
        ),
    ]);
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(events))
        .mount(&server)
        .await;
    mount_empty_search(&server).await;

    let outcome = run_walk(&server).await.expect("day-window walk succeeds");
    assert_eq!(outcome.events.len(), 1, "only today's event survives");
    assert!(outcome.events[0].title.contains("In window"));
    assert!(
        outcome.filtered_by_date >= 2,
        "both out-of-window events counted as filtered_by_date"
    );
}

// ---- 5. Search dedup -----------------------------------------------------

#[tokio::test]
async fn walk_day_dedupes_events_that_also_appear_in_search() {
    let server = MockServer::start().await;

    // Events stream emits the PR opened event.
    let events = json!([pr_opened_event(
        "evt-1",
        SELF_USER_ID,
        SELF_LOGIN,
        "2026-04-20T09:00:00Z",
        "modulr/foo",
        42,
        "Fix payment gateway"
    )]);
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(events))
        .mount(&server)
        .await;

    // Search also surfaces the same PR (same repo + number → same
    // external_id). The walker must collapse it.
    let search_body = json!({
        "total_count": 1,
        "incomplete_results": false,
        "items": [{
            "id": 1001,
            "number": 42,
            "title": "Fix payment gateway",
            "state": "open",
            "html_url": "https://github.com/modulr/foo/pull/42",
            "repository_url": "https://api.github.com/repos/modulr/foo",
            "created_at": "2026-04-20T09:00:00Z",
            "user": {
                "id": SELF_USER_ID,
                "login": SELF_LOGIN
            },
            "pull_request": {
                "url": "https://api.github.com/repos/modulr/foo/pulls/42"
            }
        }]
    });
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body))
        .mount(&server)
        .await;

    let outcome = run_walk(&server).await.expect("dedup walk succeeds");
    assert_eq!(
        outcome.events.len(),
        1,
        "search + events collapse to one activity event: got {:#?}",
        outcome.events
    );
    assert_eq!(
        outcome.events[0].kind,
        ActivityKind::GitHubPullRequestOpened
    );
    assert!(
        outcome.deduped_by_external_id >= 1,
        "dedup counter must record the collapse"
    );
}

// ---- 6. Rapid-review collapse -------------------------------------------

#[tokio::test]
async fn walk_day_collapses_three_rapid_reviews_into_one_event() {
    let server = MockServer::start().await;

    // Three reviews on the same PR in a 30-second window. The rollup
    // must collapse them into one GitHubPullRequestReviewed event.
    let events = json!([
        pr_review_event(
            "evt-r1",
            SELF_USER_ID,
            SELF_LOGIN,
            "2026-04-20T12:00:00Z",
            "modulr/foo",
            42,
            "Refactor handler",
            "commented"
        ),
        pr_review_event(
            "evt-r2",
            SELF_USER_ID,
            SELF_LOGIN,
            "2026-04-20T12:00:10Z",
            "modulr/foo",
            42,
            "Refactor handler",
            "changes_requested"
        ),
        pr_review_event(
            "evt-r3",
            SELF_USER_ID,
            SELF_LOGIN,
            "2026-04-20T12:00:30Z",
            "modulr/foo",
            42,
            "Refactor handler",
            "approved"
        ),
    ]);
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(events))
        .mount(&server)
        .await;
    mount_empty_search(&server).await;

    let outcome = run_walk(&server).await.expect("rapid-review walk succeeds");
    assert_eq!(
        outcome.events.len(),
        1,
        "three rapid reviews collapse into one: got {:#?}",
        outcome.events
    );
    let ev = &outcome.events[0];
    assert_eq!(ev.kind, ActivityKind::GitHubPullRequestReviewed);
    assert_eq!(
        ev.metadata.get("review_count").and_then(|v| v.as_i64()),
        Some(3),
        "collapsed metadata carries the count"
    );
}

// ---- 7. No identity — early bail ----------------------------------------

#[tokio::test]
async fn walk_day_returns_empty_outcome_when_no_github_identity_configured() {
    let server = MockServer::start().await;

    // Mount a mock that would fire (and fail `.expect(0)`) if the
    // walker made a request anyway.
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 0,
            "incomplete_results": false,
            "items": []
        })))
        .expect(0)
        .mount(&server)
        .await;

    // Scope a lone GitLabUserId (wrong kind) to the github source —
    // the walker must not treat it as a match.
    let identities = vec![SourceIdentity {
        id: Uuid::new_v4(),
        person_id: Uuid::new_v4(),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: SELF_USER_ID.to_string(),
        source_id: Some(source_id()),
    }];

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &api_base(&server),
        source_id(),
        &identities,
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("missing identity should early-bail, not error");

    assert!(outcome.events.is_empty());
    assert_eq!(outcome.fetched_count, 0);
}

// ---- 8. Query-string shape (regression-proof on self-filter path) -------
//
// A belt-and-braces on the search-issues `q=` clause: the walker must
// include both `involves:<login>` and `updated:<start>..<end>` so
// GitHub scopes the search to the user's activity within the window.
// Without this, the scaffolded `/search/issues` call would return
// every issue ever touched by the login — burning rate-limit budget
// and exposing cross-day rows we'd have to filter out anyway.

#[tokio::test]
async fn walk_day_sends_involves_and_updated_clause_to_search_issues() {
    let server = MockServer::start().await;

    // Events stream: empty, so the walker reaches search.
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    // Strict matcher: only match when `q` contains both clauses.
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param(
            "q",
            "involves:vedanth updated:2026-04-20T00:00:00Z..2026-04-21T00:00:00Z",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 0,
            "incomplete_results": false,
            "items": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let outcome = run_walk(&server).await.expect("q-shape walk succeeds");
    assert!(outcome.events.is_empty());
}
