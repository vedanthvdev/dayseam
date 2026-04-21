//! End-to-end wiremock-driven tests for the DAY-80 Confluence CQL
//! walker.
//!
//! The plan's Task 8 matrix, reflecting the spike's captured field
//! shapes in `docs/spikes/2026-04-20-atlassian-connectors-data-shape.md`:
//!
//! 1. **Happy / created vs edited** — a first-version self-authored
//!    page whose `createdDate` is inside the window emits
//!    [`ActivityKind::ConfluencePageCreated`]. A later-version
//!    self-authored page emits [`ActivityKind::ConfluencePageEdited`].
//! 2. **Comment emission** — an ADF-bodied comment authored by self
//!    emits [`ActivityKind::ConfluenceComment`] whose `body` renders
//!    the ADF paragraph (mentions resolve to `@displayName`).
//! 3. **Rapid-save collapse** — five page edits on the same page by
//!    the same author inside a five-minute window collapse into one
//!    `ConfluencePageEdited` event with `metadata.save_count == 5`.
//!    The CQL endpoint itself returns one row per content-id today
//!    (its `contributor = currentUser()` query folds all versions of
//!    a page into one row), so this test hand-assembles a five-version
//!    fixture by mounting five distinct CQL result rows whose
//!    normalised events the walker's post-pass then folds.
//! 4. **Self-filter** — a comment authored by someone *other* than
//!    the configured `AtlassianAccountId` is silently dropped, and a
//!    page-version authored by someone else (e.g. the walker matched
//!    the page because the user commented on it) does not claim a
//!    `ConfluencePageEdited` for them.
//! 5. **Pagination** — the walker drives multiple pages via
//!    `_links.next` cursor extraction (spike §5) and only stops when
//!    `_links.next` is absent.
//! 6. **Rate limit** — the walker's `429` path surfaces as
//!    [`DayseamError::RateLimited`] with
//!    `code: confluence.walk.rate_limited`, never leaking the SDK's
//!    internal `http.*` code.
//! 7. **Shape guard** — a CQL response missing the `results` array
//!    fails loudly with `confluence.walk.upstream_shape_changed`
//!    (the DAY-71 invariant: a silent empty report is the worst
//!    outcome).
//! 8. **Identity miss** — no `AtlassianAccountId` identity registered
//!    for the source returns an empty outcome without ever issuing a
//!    CQL request (the early-bail arm in `walk_day`).
//! 9. **ADF assertion** — every expanded-body request sent by the
//!    walker carries `expand=...content.body.atlas_doc_format...` in
//!    its query string (spike §8.5). A regression that flipped to
//!    `storage` format would hard-fail the orchestrator's body
//!    rendering because `adf_to_plain` would see HTML, not ADF.

use std::sync::Arc;

use chrono::{FixedOffset, NaiveDate};
use connector_confluence::walk::walk_day;
use connectors_sdk::{AuthStrategy, BasicAuth, HttpClient, RetryPolicy};
use dayseam_core::{
    error_codes, ActivityKind, DayseamError, SourceId, SourceIdentity, SourceIdentityKind,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param, query_param_contains};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ---- Test scaffolding ----------------------------------------------------

const SELF_ACCOUNT: &str = "5d53f3cbc6b9320d9ea5bdc2";

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
    Arc::new(BasicAuth::atlassian(
        "me@acme.com",
        "api-token",
        "dayseam.confluence",
        "acme",
    ))
}

fn self_identity() -> Vec<SourceIdentity> {
    vec![SourceIdentity {
        id: Uuid::new_v4(),
        person_id: Uuid::new_v4(),
        kind: SourceIdentityKind::AtlassianAccountId,
        external_actor_id: SELF_ACCOUNT.into(),
        source_id: Some(source_id()),
    }]
}

fn workspace(server: &MockServer) -> Url {
    // `url::Url::join` replaces the last path segment unless the base
    // ends with `/`. The walker joins `wiki/rest/api/search` onto this,
    // so we need the trailing slash or the resulting URL drops the
    // workspace host altogether.
    Url::parse(&format!("{}/", server.uri())).unwrap()
}

/// Build a single CQL `result` row that normalises to the requested
/// event kind. `created_by` / `version_by` default to `SELF_ACCOUNT`
/// so the self-filter lets the event through; tests opt into a
/// different author by passing `Some("…")`.
fn page_result_row(
    content_id: &str,
    title: &str,
    version_number: u64,
    created_date: &str,
    version_when: &str,
    created_by: Option<&str>,
    version_by: Option<&str>,
) -> Value {
    let created_by = created_by.unwrap_or(SELF_ACCOUNT);
    let version_by = version_by.unwrap_or(SELF_ACCOUNT);
    json!({
        "content": {
            "id": content_id,
            "type": "page",
            "status": "current",
            "title": title,
            "space": {"key": "ENG", "name": "Engineering"},
            "history": {
                "createdDate": created_date,
                "createdBy": {
                    "accountId": created_by,
                    "displayName": "Me"
                }
            },
            "version": {
                "number": version_number,
                "when": version_when,
                "by": {
                    "accountId": version_by,
                    "displayName": "Me"
                }
            },
            "body": {
                "atlas_doc_format": {
                    "value": "{\"type\":\"doc\",\"content\":[]}",
                    "representation": "atlas_doc_format"
                }
            },
            "_links": {"webui": format!("/spaces/ENG/pages/{content_id}/{title}")}
        },
        "url": format!("/spaces/ENG/pages/{content_id}/{title}"),
        "_links": {"base": "https://acme.atlassian.net/wiki"}
    })
}

/// Build a CQL `result` row for a comment on a given page.
fn comment_result_row(
    comment_id: &str,
    page_container_id: &str,
    page_title: &str,
    created_date: &str,
    adf_body_json: &str,
    location: &str,
    author: Option<&str>,
) -> Value {
    let author = author.unwrap_or(SELF_ACCOUNT);
    json!({
        "content": {
            "id": comment_id,
            "type": "comment",
            "status": "current",
            "title": format!("Re: {page_title}"),
            "space": {"key": "ENG", "name": "Engineering"},
            "container": {
                "id": page_container_id,
                "type": "page",
                "title": page_title
            },
            "history": {
                "createdDate": created_date,
                "createdBy": {
                    "accountId": author,
                    "displayName": "Me"
                }
            },
            "version": {"number": 1, "when": created_date,
                        "by": {"accountId": author, "displayName": "Me"}},
            "extensions": {"location": location},
            "body": {
                "atlas_doc_format": {
                    "value": adf_body_json,
                    "representation": "atlas_doc_format"
                }
            },
            "_links": {"webui": format!("/spaces/ENG/pages/{page_container_id}/{page_title}?focusedCommentId={comment_id}")}
        },
        "url": format!("/spaces/ENG/pages/{page_container_id}/{page_title}?focusedCommentId={comment_id}"),
        "_links": {"base": "https://acme.atlassian.net/wiki"}
    })
}

fn envelope(results: Vec<Value>, next_link: Option<&str>) -> Value {
    let mut links = json!({});
    if let Some(link) = next_link {
        links["next"] = json!(link);
    }
    json!({
        "results": results,
        "limit": 25,
        "size": results.len(),
        "_links": links
    })
}

async fn mount_cql_returning(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .and(query_param_contains(
            "expand",
            "content.body.atlas_doc_format",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

// ---- 1. Created vs edited distinction -----------------------------------

#[tokio::test]
async fn walk_day_distinguishes_created_from_edited_for_self_authored_pages() {
    let server = MockServer::start().await;

    // Row A: first-version page created today by self → emits
    // ConfluencePageCreated. Row B: later-version page edited today
    // by self → emits ConfluencePageEdited.
    let rows = vec![
        page_result_row(
            "100",
            "New runbook",
            1,
            "2026-04-20T09:00:00.000Z",
            "2026-04-20T09:00:00.000Z",
            None,
            None,
        ),
        page_result_row(
            "200",
            "Existing runbook",
            4,
            "2026-03-15T09:00:00.000Z",
            "2026-04-20T14:30:00.000Z",
            Some("other-account-id"),
            None,
        ),
    ];
    mount_cql_returning(&server, envelope(rows, None)).await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("happy path should succeed");

    assert_eq!(outcome.fetched_count, 2);
    assert_eq!(outcome.events.len(), 2, "one event per row");
    // Events sort by occurred_at ascending.
    assert_eq!(outcome.events[0].kind, ActivityKind::ConfluencePageCreated);
    assert_eq!(outcome.events[0].metadata["version_number"], json!(1));
    assert_eq!(outcome.events[1].kind, ActivityKind::ConfluencePageEdited);
    assert_eq!(outcome.events[1].metadata["version_number"], json!(4));
}

// ---- 2. Comment emission with ADF body ----------------------------------

#[tokio::test]
async fn walk_day_emits_confluence_comment_and_renders_adf_mention_as_display_name() {
    let server = MockServer::start().await;

    // ADF comment: "hey @Saravanan could you update the replication
    // steps?" — the walker must render the mention's displayName,
    // never the raw accountId.
    let adf = r#"{
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [
                {"type": "text", "text": "hey "},
                {"type": "mention",
                 "attrs": {"id": "colleague-account-id", "text": "@Saravanan"}},
                {"type": "text", "text": " could you update the replication steps?"}
            ]
        }]
    }"#;

    let rows = vec![comment_result_row(
        "900",
        "200",
        "Existing+runbook",
        "2026-04-20T14:16:00.000Z",
        adf,
        "inline",
        None,
    )];
    mount_cql_returning(&server, envelope(rows, None)).await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("comment path should succeed");

    assert_eq!(outcome.events.len(), 1);
    let ev = &outcome.events[0];
    assert_eq!(ev.kind, ActivityKind::ConfluenceComment);
    let body = ev.body.as_deref().expect("comment body rendered");
    assert!(body.contains("@Saravanan"), "mention renders display name");
    assert!(
        !body.contains("colleague-account-id"),
        "mention must never leak accountId: {body}"
    );
    assert!(body.contains("replication steps"));
    assert_eq!(ev.metadata["location"], json!("inline"));
}

// ---- 3. Self-filter — other author's comment / page drops ----------------

#[tokio::test]
async fn walk_day_drops_comments_and_page_versions_authored_by_others() {
    let server = MockServer::start().await;

    // Row A: comment authored by colleague — must drop.
    // Row B: page whose latest version is authored by a colleague —
    //        the walker matched it because SELF commented on it, but
    //        emitting a page-edit would falsely claim credit.
    let adf = r#"{"type":"doc","content":[{"type":"paragraph",
        "content":[{"type":"text","text":"reply"}]}]}"#;
    let rows = vec![
        comment_result_row(
            "901",
            "200",
            "Runbook",
            "2026-04-20T15:00:00.000Z",
            adf,
            "footer",
            Some("colleague-account-id"),
        ),
        page_result_row(
            "200",
            "Runbook",
            3,
            "2026-03-15T09:00:00.000Z",
            "2026-04-20T16:00:00.000Z",
            Some("other-account-id"),
            Some("other-account-id"),
        ),
    ];
    mount_cql_returning(&server, envelope(rows, None)).await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("self-filter path should succeed");

    assert!(
        outcome.events.is_empty(),
        "no self-events expected: got {:#?}",
        outcome.events
    );
    assert_eq!(outcome.fetched_count, 2, "both rows still observed");
}

// ---- 4. Rapid-save collapse ---------------------------------------------

#[tokio::test]
async fn walk_day_collapses_rapid_page_edits_into_one_event() {
    let server = MockServer::start().await;

    // Five edits on content-id "300" by self, all inside a
    // five-minute window. In the real API each page yields exactly
    // one row, so we fabricate five fixture rows with the same
    // content-id and ascending version-numbers to exercise the
    // `edits_by_page` staging map + `collapse_rapid_edits` post-pass
    // end-to-end.
    let rows = vec![
        page_result_row(
            "300",
            "Release checklist",
            2,
            "2026-03-20T09:00:00.000Z",
            "2026-04-20T09:00:00.000Z",
            Some("other-account-id"),
            None,
        ),
        page_result_row(
            "300",
            "Release checklist",
            3,
            "2026-03-20T09:00:00.000Z",
            "2026-04-20T09:01:00.000Z",
            Some("other-account-id"),
            None,
        ),
        page_result_row(
            "300",
            "Release checklist",
            4,
            "2026-03-20T09:00:00.000Z",
            "2026-04-20T09:02:00.000Z",
            Some("other-account-id"),
            None,
        ),
        page_result_row(
            "300",
            "Release checklist",
            5,
            "2026-03-20T09:00:00.000Z",
            "2026-04-20T09:03:00.000Z",
            Some("other-account-id"),
            None,
        ),
        page_result_row(
            "300",
            "Release checklist",
            6,
            "2026-03-20T09:00:00.000Z",
            "2026-04-20T09:04:00.000Z",
            Some("other-account-id"),
            None,
        ),
    ];
    mount_cql_returning(&server, envelope(rows, None)).await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("rapid-save walk should succeed");

    assert_eq!(
        outcome.events.len(),
        1,
        "five autosaves collapse into one event, got {} events",
        outcome.events.len()
    );
    let ev = &outcome.events[0];
    assert_eq!(ev.kind, ActivityKind::ConfluencePageEdited);
    assert_eq!(ev.metadata["save_count"], json!(5));
    assert_eq!(
        ev.metadata["version_number"],
        json!(6),
        "latest version wins"
    );
    assert!(
        ev.title.contains("rolled up from 5 saves"),
        "rolled-up title should hint at collapse: {}",
        ev.title
    );
}

// ---- 5. Pagination via _links.next --------------------------------------

#[tokio::test]
async fn walk_day_paginates_via_links_next_cursor() {
    let server = MockServer::start().await;

    let page1 = envelope(
        vec![page_result_row(
            "101",
            "Page one",
            1,
            "2026-04-20T09:00:00.000Z",
            "2026-04-20T09:00:00.000Z",
            None,
            None,
        )],
        Some("/wiki/rest/api/search?cursor=page-2&limit=25"),
    );
    let page2 = envelope(
        vec![page_result_row(
            "102",
            "Page two",
            1,
            "2026-04-20T10:00:00.000Z",
            "2026-04-20T10:00:00.000Z",
            None,
            None,
        )],
        None,
    );

    // wiremock matches mocks in LIFO order: mount the specific
    // cursor matcher *last* so it wins when the walker's second
    // request carries `cursor=page-2`, and the fallthrough page-1
    // matcher catches the first (cursor-less) request.
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .and(query_param("cursor", "page-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page2))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page1))
        .expect(1)
        .mount(&server)
        .await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("pagination path should succeed");

    assert_eq!(outcome.fetched_count, 2, "both pages observed");
    assert_eq!(outcome.events.len(), 2, "one event per page");
}

// ---- 6. Rate-limit 429 ---------------------------------------------------

#[tokio::test]
async fn walk_day_maps_429_to_confluence_walk_rate_limited() {
    let server = MockServer::start().await;

    // Always-429. With `RetryPolicy::instant()` the SDK retries 5
    // times then surfaces `RateLimited { code: http.retry_budget_exhausted }`;
    // the walker remaps that to `confluence.walk.rate_limited`.
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .mount(&server)
        .await;

    let err = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect_err("429 should surface as a rate-limit error");

    assert_eq!(err.code(), error_codes::CONFLUENCE_WALK_RATE_LIMITED);
    assert!(
        matches!(err, DayseamError::RateLimited { .. }),
        "expected RateLimited variant, got: {err:?}"
    );
}

// ---- 7. Shape guard — missing `results` array ----------------------------

#[tokio::test]
async fn walk_day_flags_missing_results_array_as_upstream_shape_changed() {
    let server = MockServer::start().await;

    // A 200 with a syntactically valid JSON object missing `results`.
    // The walker must refuse to paper over this.
    let body = json!({"_links": {}, "size": 0, "limit": 25});
    mount_cql_returning(&server, body).await;

    let err = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect_err("missing results should error, not succeed silently");

    assert_eq!(
        err.code(),
        error_codes::CONFLUENCE_WALK_UPSTREAM_SHAPE_CHANGED
    );
}

// ---- 8. No identity — early bail ----------------------------------------

#[tokio::test]
async fn walk_day_returns_empty_outcome_when_no_atlassian_identity_configured() {
    let server = MockServer::start().await;

    // The walker must not issue a CQL at all when there's no
    // AtlassianAccountId identity in scope — every event would be
    // filtered out and the request would burn rate-limit budget for
    // no reason. `.expect(0)` makes wiremock fail the test if the
    // walker does issue a request.
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(vec![], None)))
        .expect(0)
        .mount(&server)
        .await;

    // A non-Atlassian identity only — e.g. a GitLab identity
    // accidentally scoped to this Confluence source.
    let wrong_identities = vec![SourceIdentity {
        id: Uuid::new_v4(),
        person_id: Uuid::new_v4(),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: "17".into(),
        source_id: Some(source_id()),
    }];

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &wrong_identities,
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("missing identity should bail with an empty outcome, not error");

    assert!(outcome.events.is_empty());
    assert_eq!(outcome.fetched_count, 0);
}

// ---- 9. ADF body-format assertion ---------------------------------------

#[tokio::test]
async fn walk_day_requests_atlas_doc_format_body_on_every_page() {
    // Spike §8.5: every body the walker reads must come in ADF so
    // `adf_to_plain` is the only body-normalisation path. A silent
    // flip to `storage` would leak raw HTML into the event body.
    // This test asserts the query param shape on *every* CQL request
    // the walker sends, including paginated follow-ups.
    let server = MockServer::start().await;

    let page1 = envelope(
        vec![page_result_row(
            "101",
            "Page one",
            1,
            "2026-04-20T09:00:00.000Z",
            "2026-04-20T09:00:00.000Z",
            None,
            None,
        )],
        Some("/wiki/rest/api/search?cursor=page-2&limit=25"),
    );
    let page2 = envelope(
        vec![page_result_row(
            "102",
            "Page two",
            1,
            "2026-04-20T10:00:00.000Z",
            "2026-04-20T10:00:00.000Z",
            None,
            None,
        )],
        None,
    );

    // LIFO matching: mount the more-specific cursor matcher last so
    // it's checked first for the paginated follow-up request.
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .and(query_param("cursor", "page-2"))
        .and(query_param_contains(
            "expand",
            "content.body.atlas_doc_format",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(page2))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .and(query_param_contains(
            "expand",
            "content.body.atlas_doc_format",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(page1))
        .expect(1)
        .mount(&server)
        .await;

    let outcome = walk_day(
        &http_for_tests(),
        auth_for_tests(),
        &workspace(&server),
        source_id(),
        &self_identity(),
        day(),
        utc(),
        &CancellationToken::new(),
        None,
        None,
    )
    .await
    .expect("ADF-expand path should succeed across pages");

    assert_eq!(outcome.fetched_count, 2);

    // Belt-and-braces: read every request the server received and
    // assert the expand value on each. `query_param_contains` already
    // pins it per matcher, but a future refactor that lands a matcher
    // on a stricter tuple could silently drop this assertion — this
    // pass walks every received request.
    let received = server.received_requests().await.expect("wiremock requests");
    assert_eq!(received.len(), 2, "exactly two CQL requests");
    for req in received {
        assert_request_expands_atlas_doc_format(&req);
    }
}

fn assert_request_expands_atlas_doc_format(req: &Request) {
    let url = req.url.as_str();
    assert!(
        url.contains("expand="),
        "request must carry expand query param: {url}"
    );
    assert!(
        url.contains("content.body.atlas_doc_format"),
        "request expand must include content.body.atlas_doc_format: {url}"
    );
}
