//! Cursor-pagination helpers for Atlassian Cloud endpoints.
//!
//! Two shapes show up across Jira and Confluence:
//!
//! | Shape | Termination signal | Example endpoint |
//! |---|---|---|
//! | Jira v3 token | `{"isLast": true}` or missing `nextPageToken` | `POST /rest/api/3/search/jql` |
//! | Confluence v2 links | missing / empty `_links.next` | `GET /wiki/api/v2/pages` |
//!
//! Both shapes share one observable behaviour: the paginator stops
//! when the upstream says "no more pages", and the number of HTTP
//! calls made is exactly `N + 1` for an `N`-page resource (the `+1`
//! is the terminal call that returns the termination signal). The
//! trait [`CursorPaginator`] captures that contract; the two
//! implementations here are 20-line state machines.
//!
//! The paginator does **not** own the `HttpClient` — the connector
//! passes in a builder per page so each connector can keep its own
//! retry policy, headers, and body shape. The paginator is
//! stateless w.r.t. HTTP, which lets us wiremock it without mocking
//! the SDK.

use serde_json::Value;

/// One successful page: the raw JSON body the caller can destructure
/// into issues / pages / comments, plus the cursor to carry into the
/// next call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The page body — whatever the upstream returned. Parsing is
    /// the connector's job, not the paginator's.
    pub body: Value,
    /// Opaque continuation token to pass to the next page fetch, or
    /// `None` if this is the terminal page.
    pub next_cursor: Option<String>,
}

/// Drives a cursor-pagination loop. Each `advance` call consumes a
/// raw response body and returns the next [`Page`]; the connector
/// decides how many pages to walk (usually "until `next_cursor` is
/// `None`") and makes the HTTP call itself.
///
/// We keep the trait surface to a single method so the two shapes
/// below stay testable in isolation. The alternative — passing the
/// `HttpClient` and a URL into the paginator — would couple the
/// paginator to `connectors-sdk::HttpClient` and make the unit tests
/// need a live mock for every case.
pub trait CursorPaginator: Send + Sync {
    /// Parse `body` into a [`Page`]. Returns `None` if the body does
    /// not look like a page from this paginator's endpoint (caller
    /// should surface this as `DayseamError::UpstreamChanged`).
    fn parse(&self, body: Value) -> Option<Page>;
}

// ---- Jira v3 token paginator ---------------------------------------------

/// `POST /rest/api/3/search/jql` returns `{ issues: [...], isLast:
/// true|false, nextPageToken?: "..." }`. This paginator terminates
/// when **either** `isLast == true` **or** `nextPageToken` is absent
/// — both signals appear in real responses (the `isLast` field is
/// the canonical one, but Atlassian has been migrating its shape and
/// some historical responses rely on the missing-token signal).
#[derive(Debug, Default, Clone)]
pub struct JqlTokenPaginator;

impl CursorPaginator for JqlTokenPaginator {
    fn parse(&self, body: Value) -> Option<Page> {
        // `body` must be a JSON object to be a valid page at all.
        if !body.is_object() {
            return None;
        }
        let is_last = body.get("isLast").and_then(Value::as_bool).unwrap_or(false);
        let next_cursor = body
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // Terminal if isLast == true or the token is missing. The
        // double-signal rule keeps us safe against both the old and
        // new Atlassian response shapes.
        let next_cursor = if is_last { None } else { next_cursor };
        Some(Page { body, next_cursor })
    }
}

// ---- Confluence v2 `_links.next` paginator -------------------------------

/// `GET /wiki/api/v2/pages?cursor=...` returns `{ results: [...],
/// _links: { next: "/wiki/api/v2/pages?cursor=..." | absent } }`.
/// The `_links.next` value is a **relative path including query
/// string** — the connector combines it with the workspace URL to
/// form the next absolute URL. Per the spike §5, this paginator
/// extracts the raw `cursor` query parameter from that URL so the
/// connector can reuse its existing request builder instead of
/// re-parsing the relative path.
#[derive(Debug, Default, Clone)]
pub struct V2CursorPaginator;

impl CursorPaginator for V2CursorPaginator {
    fn parse(&self, body: Value) -> Option<Page> {
        if !body.is_object() {
            return None;
        }
        let next_link = body
            .get("_links")
            .and_then(|l| l.get("next"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());

        let next_cursor = next_link.and_then(extract_cursor_query_param);
        Some(Page { body, next_cursor })
    }
}

/// Pull the `cursor=<value>` query parameter out of a relative URL
/// string. Returns `None` if the URL has no query string or no
/// `cursor` parameter. Resilient to leading `?`, absolute-vs-relative
/// paths, and URL-encoded cursor values (left URL-encoded — the
/// caller re-encodes when building the next request URL).
fn extract_cursor_query_param(url: &str) -> Option<String> {
    // Find the first `?` and split — `_links.next` is always either
    // `/wiki/api/v2/pages?cursor=abc&limit=25` or an absolute URL of
    // the same shape.
    let (_, query) = url.split_once('?')?;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("cursor=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- JqlTokenPaginator -----------------------------------------------

    #[test]
    fn jql_paginator_emits_next_cursor_when_is_last_is_false() {
        let body = json!({"issues": [], "isLast": false, "nextPageToken": "tok-1"});
        let page = JqlTokenPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("tok-1"));
    }

    #[test]
    fn jql_paginator_terminates_on_is_last_true_even_if_token_present() {
        // Real responses often carry `nextPageToken` alongside
        // `isLast: true` (the token is still the last cursor value
        // used); the paginator must trust `isLast`.
        let body = json!({"issues": [], "isLast": true, "nextPageToken": "stale-tok"});
        let page = JqlTokenPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn jql_paginator_terminates_on_missing_token_when_is_last_absent() {
        let body = json!({"issues": []});
        let page = JqlTokenPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn jql_paginator_treats_empty_token_as_terminal() {
        let body = json!({"issues": [], "isLast": false, "nextPageToken": ""});
        let page = JqlTokenPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn jql_paginator_rejects_non_object_body() {
        assert!(JqlTokenPaginator.parse(json!([1, 2, 3])).is_none());
        assert!(JqlTokenPaginator.parse(json!("nope")).is_none());
        assert!(JqlTokenPaginator.parse(json!(null)).is_none());
    }

    // ---- V2CursorPaginator ----------------------------------------------

    #[test]
    fn v2_paginator_extracts_cursor_from_relative_next_link() {
        let body = json!({
            "results": [],
            "_links": { "next": "/wiki/api/v2/pages?cursor=abc123&limit=25" }
        });
        let page = V2CursorPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("abc123"));
    }

    #[test]
    fn v2_paginator_terminates_when_next_link_is_missing() {
        let body = json!({"results": [], "_links": {}});
        let page = V2CursorPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn v2_paginator_terminates_when_links_object_is_missing() {
        let body = json!({"results": []});
        let page = V2CursorPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn v2_paginator_terminates_when_next_has_no_cursor_param() {
        let body = json!({"results": [], "_links": {"next": "/wiki/api/v2/pages?limit=25"}});
        let page = V2CursorPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn v2_paginator_extracts_cursor_from_absolute_next_link() {
        let body = json!({
            "results": [],
            "_links": {
                "next": "https://foo.atlassian.net/wiki/api/v2/pages?limit=25&cursor=xyz"
            }
        });
        let page = V2CursorPaginator.parse(body).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("xyz"));
    }

    #[test]
    fn v2_paginator_rejects_non_object_body() {
        assert!(V2CursorPaginator.parse(json!([])).is_none());
    }

    /// Termination invariant (plan §11 row 3): both paginators stop
    /// after `N + 1` calls for an `N`-page fixture. We simulate the
    /// call loop here in pure-code form — wiremock-driven proof of
    /// the same invariant lives in `tests/pagination.rs`.
    #[test]
    fn jql_paginator_terminates_within_n_plus_one_for_three_pages() {
        let pages = [
            json!({"issues": [1], "isLast": false, "nextPageToken": "t1"}),
            json!({"issues": [2], "isLast": false, "nextPageToken": "t2"}),
            json!({"issues": [3], "isLast": true, "nextPageToken": "t2"}),
        ];
        let mut calls = 0;
        let mut i = 0;
        let final_cursor = loop {
            calls += 1;
            let page = JqlTokenPaginator.parse(pages[i].clone()).unwrap();
            if page.next_cursor.is_none() {
                break page.next_cursor;
            }
            i += 1;
            assert!(
                i < pages.len(),
                "paginator ran past last fixture without terminating"
            );
        };
        assert_eq!(calls, 3, "expected exactly 3 parse calls for 3 pages");
        assert!(final_cursor.is_none());
    }
}
