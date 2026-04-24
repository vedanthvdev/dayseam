//! End-to-end wiremock fixture for GitHub `Link`-header pagination
//! (**TST-v0.4-01 / DAY-110**).
//!
//! The unit tests in `pagination.rs` exercise
//! [`parse_next_from_link_header`] against synthetic strings, and the
//! reqwest-boundary test in `tests/pagination.rs` pins the
//! `reqwest::Response` → header-string → parser seam against a single
//! response. Neither of those would catch a regression *inside* the
//! walker's page-follow loop at
//! [`connector_github::walk::walk_user_events`] (`src/walk.rs:205..316`):
//!
//! * Drop the `next_url = link_header…` line at `:312` and the walker
//!   silently stops after page 1. `tests/pagination.rs` still passes
//!   (the Link header is parsed correctly) and `tests/walk.rs` still
//!   passes (every existing mock returns a single page). The user
//!   gets a silently-truncated daily report — exactly the DAY-88
//!   silent-failure class the v0.4 review TST-v0.4-02 item called out.
//! * Drop the `for page in 0..MAX_PAGES` cap at `:205` in favour of a
//!   `loop { … }` and a server that accidentally serves a self-referential
//!   `rel="next"` will drain the authenticated PAT's 5 000-req/hour
//!   budget before anyone notices.
//!
//! The two tests below pin those invariants at the walk layer:
//!
//! 1. `walker_collects_events_across_link_header_pages` — a real
//!    two-page response sequence where page 1 advertises page 2 via
//!    `Link: rel="next"` and page 2 is terminal (no `Link`). Asserts
//!    all 50 in-window events land in the outcome, in the order the
//!    server served them.
//! 2. `walker_terminates_at_max_pages_on_cycle` — a server whose
//!    `Link: rel="next"` points back at itself. Asserts the walker
//!    makes exactly `MAX_PAGES` events requests and then stops
//!    cleanly, rather than looping forever. Wrapped in a
//!    `tokio::time::timeout` so reverting the `MAX_PAGES` cap would
//!    surface as a hang rather than a pass.
//!
//! Both use the same scaffolding shape as `tests/walk.rs` so a future
//! refactor of `walk_day`'s signature breaks every GitHub walker
//! integration test in lockstep.

use std::sync::Arc;
use std::time::Duration;

use chrono::{FixedOffset, NaiveDate};
use connector_github::walk::{walk_day, WalkOutcome};
use connectors_sdk::{AuthStrategy, HttpClient, PatAuth, RetryPolicy};
use dayseam_core::{error_codes, DayseamError, SourceId, SourceIdentity, SourceIdentityKind};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- Scaffolding (mirrors `tests/walk.rs`) -------------------------------

const SELF_LOGIN: &str = "vedanth";
const SELF_USER_ID: i64 = 17;

/// Upper bound on pages the walker will follow per endpoint. Duplicates
/// the private `MAX_PAGES` constant at `src/walk.rs:52`. Keeping it
/// private there (rather than exporting it) preserves the walker's
/// encapsulation — the test pins the externally observable request
/// count, not an internal symbol. If `MAX_PAGES` ever changes there,
/// this literal and the cycle-test assertion need to move in lockstep.
const MAX_PAGES: usize = 30;

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()
}

fn source_id() -> SourceId {
    Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap()
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

async fn run_walk(server: &MockServer) -> WalkOutcome {
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
    .expect("walk succeeds")
}

/// Variant of [`run_walk`] that returns the raw `Result` instead of
/// unwrapping. Introduced in DAY-122 / C-2 so the cycle-guard-trip
/// test can assert on the returned error shape — pre-C-2 the walker
/// silently truncated on cap trip and returned `Ok`, so the original
/// `walker_terminates_at_max_pages_on_cycle` test only asserted
/// request counts. The stricter error-shape assertion is what pins
/// the fix.
async fn run_walk_result(server: &MockServer) -> Result<WalkOutcome, DayseamError> {
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

/// Always-empty `/search/issues` stub so the pagination tests focus on
/// the events endpoint. Mirrors `tests/walk.rs::mount_empty_search`.
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

/// Build a `PullRequestEvent` JSON payload inside the 2026-04-20 UTC
/// day window. `hour_utc` + `minute_utc` let the caller spread events
/// across the window so they all survive the walker's date filter.
fn pr_opened_event_in_window(
    event_id: &str,
    number: u64,
    hour_utc: u32,
    minute_utc: u32,
) -> serde_json::Value {
    let created_at = format!("2026-04-20T{hour_utc:02}:{minute_utc:02}:00Z");
    json!({
        "id": event_id,
        "type": "PullRequestEvent",
        "actor": {
            "id": SELF_USER_ID,
            "login": SELF_LOGIN,
            "display_login": SELF_LOGIN
        },
        "repo": {
            "id": 1,
            "name": "company/foo",
            "url": "https://api.github.com/repos/company/foo"
        },
        "created_at": created_at,
        "payload": {
            "action": "opened",
            "number": number,
            "pull_request": {
                "id": number * 100,
                "number": number,
                "title": format!("PR #{number}"),
                "state": "open",
                "html_url": format!("https://github.com/company/foo/pull/{number}"),
                "user": {
                    "id": SELF_USER_ID,
                    "login": SELF_LOGIN
                }
            }
        }
    })
}

// ---- Test 1: multi-page collection ---------------------------------------
//
// Walker contract: issue page 1, follow `Link: rel="next"` to page 2,
// stop when page 2 returns no `Link` header. The walker accumulates
// events across both pages with no loss and no duplication.
//
// What a revert would look like: a refactor of the for-loop at
// `src/walk.rs:205` that replaces `next_url = link_header…` at `:312`
// with `next_url = None` (or accidentally re-assigns `next_url` before
// the new page lands). The page-1-only events would survive, page-2's
// would silently disappear, and the outcome's event count would drop
// from 50 to 30.

#[tokio::test]
async fn walker_collects_events_across_link_header_pages() {
    let server = MockServer::start().await;

    // Page 1: 30 events, one per minute across 9:00..9:30 UTC — all
    // comfortably inside 2026-04-20's UTC day window, so the walker
    // never flips `reached_window_floor` and actually has to follow
    // the `Link: rel="next"` to reach page 2.
    let page1_body: Vec<serde_json::Value> = (0..30)
        .map(|i| {
            pr_opened_event_in_window(
                &format!("evt-p1-{i:02}"),
                /* pr number */ 1_000 + i,
                /* hour */ 9,
                /* minute */ i as u32,
            )
        })
        .collect();

    // Page 2 URL points back at the same mock server but with
    // `?page=2` — the shape GitHub actually serves. We use a
    // `query_param` matcher to route page-2 requests to the terminal
    // mock (no `Link` header → walker loop terminates on the next
    // iteration).
    let page2_url = format!("{}/users/{SELF_LOGIN}/events?page=2", server.uri());
    let link_to_page2 = format!(r#"<{page2_url}>; rel="next""#);

    let page2_body: Vec<serde_json::Value> = (0..20)
        .map(|i| {
            pr_opened_event_in_window(
                &format!("evt-p2-{i:02}"),
                /* pr number */ 2_000 + i,
                /* hour */ 10,
                /* minute */ i as u32,
            )
        })
        .collect();

    // Page 2 mock — mounted first so its narrower matcher (path AND
    // `?page=2`) takes priority over the page-1 catch-all below.
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(page2_body)))
        .expect(1)
        .mount(&server)
        .await;

    // Page 1 mock — any GET to the events path that is NOT already
    // claimed by the page-2 matcher above. This is the walker's
    // initial request (which carries `per_page=100` but no `page=N`).
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Link", link_to_page2.as_str())
                .set_body_json(json!(page1_body)),
        )
        .expect(1)
        .mount(&server)
        .await;

    mount_empty_search(&server).await;

    let outcome = run_walk(&server).await;

    // (1) No loss, no duplication — 30 + 20 events land in the outcome.
    assert_eq!(
        outcome.events.len(),
        50,
        "walker must collect every in-window event across both pages: got {:#?}",
        outcome
            .events
            .iter()
            .map(|e| &e.external_id)
            .collect::<Vec<_>>()
    );

    // (2) Fetched count sums both pages — the walker's own counter
    //     would under-report if an early-break snuck in.
    assert_eq!(outcome.fetched_count, 50);

    // (3) Ordering: the walker sorts by `occurred_at` after collection,
    //     so page 1's 9:00..9:30 events come before page 2's 10:00..10:20
    //     events. Assert the first and last events are from the
    //     expected pages — a stronger check than pure count, catches a
    //     regression where page-2 events are silently appended to
    //     (say) `fetched_count` without landing in `events`.
    assert!(
        outcome.events[0].external_id.ends_with("#1000"),
        "first event after sort should be page-1's earliest PR: got {}",
        outcome.events[0].external_id
    );
    assert!(
        outcome.events[49].external_id.ends_with("#2019"),
        "last event after sort should be page-2's latest PR: got {}",
        outcome.events[49].external_id
    );

    // (4) Exactly two events requests hit the server — no third probe
    //     once page 2 returned no `Link` header. The `.expect(1)`
    //     calls above enforce this at mock-drop time; this count
    //     assertion is a belt-and-braces against a future wiremock
    //     version that defers the assertion.
    let events_requests = server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .iter()
        .filter(|r| r.url.path() == format!("/users/{SELF_LOGIN}/events"))
        .count();
    assert_eq!(
        events_requests, 2,
        "walker must stop after page 2's terminal (no-Link) response"
    );
}

// ---- Test 2: MAX_PAGES cycle-guard ---------------------------------------
//
// Walker contract: even if the server serves a self-referential
// `Link: rel="next"` (cycle), the walker terminates after `MAX_PAGES`
// requests and does not hang.
//
// DAY-122 / C-2 strengthens the contract: when the walker exits the
// loop because `MAX_PAGES` is exhausted *and* the server is still
// advertising `rel="next"`, it now returns `DayseamError::Internal`
// with `code = GITHUB_PAGINATION_CYCLE_GUARD_TRIPPED` instead of
// silently truncating to `Ok`. Pre-C-2 production runs against a
// cycling proxy produced a partial daily report with no UI warning
// and no error log — a Medium-severity silent-failure flagged in
// the DAY-115 v0.5 capstone review (#129 item C-2).
//
// What a revert would look like:
//
// * Replacing `for page in 0..MAX_PAGES` at `src/walk.rs:205` with
//   `loop { … }` — the test hangs inside `run_walk_result` until the
//   outer `tokio::time::timeout` fires.
// * Deleting the `if next_url.is_some() { Err(Internal …) }` guard
//   at `src/walk.rs:317` — the walker returns `Ok(WalkOutcome)` and
//   the `expect_err` below fails, signalling the silent-truncation
//   regression before it can ship.

#[tokio::test]
async fn walker_returns_internal_error_on_max_pages_cycle() {
    let server = MockServer::start().await;

    // The `Link` header points back to the same events path — GitHub
    // would never serve this, but a proxy bug or a replayed-fixture
    // test harness might. We deliberately do NOT include a
    // `query_param` filter on the mock so every cycled request lands
    // on this same matcher.
    let self_cycle_url = format!("{}/users/{SELF_LOGIN}/events", server.uri());
    let link_to_self = format!(r#"<{self_cycle_url}>; rel="next""#);

    // One event per response, inside the day window, with a
    // page-counter-free id so every cycled response looks identical.
    let body = json!([pr_opened_event_in_window(
        "evt-cycle",
        /* pr number */ 7_777,
        /* hour */ 9,
        /* minute */ 0
    )]);

    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_LOGIN}/events")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Link", link_to_self.as_str())
                .set_body_json(body),
        )
        .mount(&server)
        .await;

    mount_empty_search(&server).await;

    // 30 s is ~2 orders of magnitude above the wall-time this test
    // takes on a warm box (~50 ms per mock round-trip, so ~1.5 s for
    // 30 requests). Reverting the MAX_PAGES guard would trip the
    // timeout long before the test suite's default 60 s hang budget.
    let err = tokio::time::timeout(Duration::from_secs(30), run_walk_result(&server))
        .await
        .expect("walker must terminate on a cycle — MAX_PAGES guard missing?")
        .expect_err(
            "DAY-122 / C-2: walker must return Err(Internal) on cycle, not silently truncate",
        );

    match err {
        DayseamError::Internal { ref code, .. } => {
            assert_eq!(
                code,
                error_codes::GITHUB_PAGINATION_CYCLE_GUARD_TRIPPED,
                "cap-trip error must carry the stable code so the UI + log-router can route it; \
                 got {code}"
            );
        }
        other => panic!(
            "cap trip must surface as DayseamError::Internal; got {other:?}. If a future refactor \
             widens this to another variant, update the UI's log-router at the same time so the \
             error is still shown to the user — the whole point of C-2 is no silent truncation."
        ),
    }

    // Belt-and-braces at the HTTP boundary: the mock server saw
    // exactly MAX_PAGES requests to the events path — no fewer (which
    // would mean an unrelated early-break swallowed the cycle) and no
    // more (which would mean the guard is off-by-one or absent).
    let events_requests = server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .iter()
        .filter(|r| r.url.path() == format!("/users/{SELF_LOGIN}/events"))
        .count();
    assert_eq!(
        events_requests, MAX_PAGES,
        "walker must stop at MAX_PAGES requests on a self-referential Link cycle"
    );
}
