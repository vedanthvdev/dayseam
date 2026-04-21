//! Wiremock-driven integration tests for the two cursor paginators.
//!
//! The unit tests in `src/pagination.rs::tests` exercise the `parse`
//! side of each paginator in isolation. These tests prove the
//! end-to-end termination invariant (plan §11 row 3): both
//! paginators drive their HTTP loops to completion in `N + 1` calls
//! for an `N`-page resource, with no off-by-one and no infinite
//! loop on a missing termination signal.

use connector_atlassian_common::{CursorPaginator, JqlTokenPaginator, V2CursorPaginator};
use connectors_sdk::{HttpClient, RetryPolicy};
use dayseam_events::{RunId, RunStreams};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http() -> HttpClient {
    HttpClient::new()
        .expect("build http client")
        .with_policy(RetryPolicy::instant())
}

// ---- Jira token paginator -----------------------------------------------

#[tokio::test]
async fn jql_paginator_walks_three_pages_and_stops_on_is_last_true() {
    let server = MockServer::start().await;

    // Page 1: no token → first call has no `nextPageToken` parameter.
    Mock::given(method("GET"))
        .and(path("/rest/api/3/search/jql"))
        .and(wiremock::matchers::query_param_is_missing("nextPageToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issues": [{"key": "KTON-1"}],
            "isLast": false,
            "nextPageToken": "tok-2"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/search/jql"))
        .and(query_param("nextPageToken", "tok-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issues": [{"key": "KTON-2"}],
            "isLast": false,
            "nextPageToken": "tok-3"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/rest/api/3/search/jql"))
        .and(query_param("nextPageToken", "tok-3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issues": [{"key": "KTON-3"}],
            "isLast": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((ptx, ltx), (_, _lrx)) = streams.split();
    let cancel = CancellationToken::new();
    let client = http();
    let paginator = JqlTokenPaginator;

    let mut cursor: Option<String> = None;
    let mut issues: Vec<String> = Vec::new();
    let mut calls = 0u32;
    loop {
        calls += 1;
        let mut url = format!("{}/rest/api/3/search/jql", server.uri());
        if let Some(tok) = &cursor {
            url.push_str(&format!("?nextPageToken={tok}"));
        }
        let res = client
            .send(client.reqwest().get(url), &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("send");
        let body: Value = res.json().await.expect("json");
        let page = paginator.parse(body).expect("parseable page");
        for issue in page.body["issues"].as_array().unwrap() {
            issues.push(issue["key"].as_str().unwrap().to_string());
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        if calls > 10 {
            panic!("paginator runaway: {calls} calls");
        }
    }

    assert_eq!(calls, 3, "expected exactly 3 HTTP calls for 3 pages");
    assert_eq!(issues, vec!["KTON-1", "KTON-2", "KTON-3"]);
}

// ---- Confluence v2 `_links.next` paginator -------------------------------

#[tokio::test]
async fn v2_paginator_walks_pages_until_next_link_missing() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/pages"))
        .and(wiremock::matchers::query_param_is_missing("cursor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [{"id": "1"}],
            "_links": { "next": "/wiki/api/v2/pages?cursor=c2&limit=25" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/pages"))
        .and(query_param("cursor", "c2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [{"id": "2"}],
            "_links": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let streams = RunStreams::new(RunId::new());
    let ((ptx, ltx), (_, _lrx)) = streams.split();
    let cancel = CancellationToken::new();
    let client = http();
    let paginator = V2CursorPaginator;

    let mut cursor: Option<String> = None;
    let mut pages: Vec<String> = Vec::new();
    let mut calls = 0u32;
    loop {
        calls += 1;
        let mut url = format!("{}/wiki/api/v2/pages", server.uri());
        if let Some(c) = &cursor {
            url.push_str(&format!("?cursor={c}"));
        }
        let res = client
            .send(client.reqwest().get(url), &cancel, Some(&ptx), Some(&ltx))
            .await
            .expect("send");
        let body: Value = res.json().await.expect("json");
        let page = paginator.parse(body).expect("parseable page");
        for p in page.body["results"].as_array().unwrap() {
            pages.push(p["id"].as_str().unwrap().to_string());
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        if calls > 10 {
            panic!("paginator runaway: {calls} calls");
        }
    }

    assert_eq!(calls, 2, "expected exactly 2 HTTP calls for 2 pages");
    assert_eq!(pages, vec!["1", "2"]);
}
