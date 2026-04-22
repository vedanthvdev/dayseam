//! Day-window walker for the Jira Cloud `POST /rest/api/3/search/jql`
//! endpoint.
//!
//! Given a local [`NaiveDate`] + a fixed UTC offset, the walker builds
//! one JQL that covers the UTC window the local day maps to, POSTs it
//! against the configured workspace, paginates via
//! [`connector_atlassian_common::JqlTokenPaginator`], and hands each
//! returned issue to [`crate::normalise::normalise_issue`] which
//! emits zero-or-more [`ActivityEvent`]s per issue (status
//! transitions, comments, self-assignments, issue-created).
//!
//! The walker is deliberately thin: it owns pagination, day-window
//! derivation, HTTP error classification, and identity resolution.
//! Everything that turns JSON into events lives in the normaliser and
//! the rollup.
//!
//! ### Identity resolution
//!
//! The walker pulls the `accountId` to filter by from the
//! [`dayseam_core::SourceIdentity`] rows the orchestrator hands it in
//! `ctx.source_identities`. A walk with no matching identity returns
//! early with zero events — the identity gap is surfaced via a warn
//! log the same way the GitLab walker does (DAY-71 invariant).

use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, TimeZone, Utc};
use connector_atlassian_common::{map_status, CursorPaginator, JqlTokenPaginator, Product};
use connectors_sdk::{AuthStrategy, HttpClient};
use dayseam_core::{
    error_codes, ActivityEvent, DayseamError, LogLevel, SourceId, SourceIdentity,
    SourceIdentityKind,
};
use dayseam_events::{LogSender, ProgressSender};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::normalise::{normalise_issue, shape_changed, DayWindow};

/// Upper bound on pages fetched per walk. At 100 issues/page, 50
/// pages = 5 000 issues updated in one day — well past any real user's
/// Jira footprint. The cap is a safety net against a paginator that
/// never terminates.
const MAX_PAGES: u32 = 50;

/// Requested page size. Atlassian caps `POST /search/jql` at 100.
const PAGE_SIZE: u32 = 100;

/// Fields to ask `/search/jql` for. Keeping the list explicit (rather
/// than falling back on `*all`) bounds payload size and keeps the
/// upstream surface observable — a field rename will surface via the
/// normaliser's shape-change guard rather than as silent data loss.
const REQUESTED_FIELDS: &[&str] = &[
    "summary",
    "status",
    "issuetype",
    "project",
    "priority",
    "labels",
    "updated",
    "created",
    "reporter",
    "comment",
];

/// Outcome of a single-day walk. `stats`-adjacent counters the
/// connector surfaces to the orchestrator via `SyncStats`.
#[derive(Debug, Default)]
pub struct WalkOutcome {
    pub events: Vec<ActivityEvent>,
    pub fetched_count: u64,
    pub filtered_by_identity: u64,
    pub filtered_by_date: u64,
    /// Count of `changelog.items[]` entries with a field we don't
    /// recognise (custom fields, new upstream additions). Surfaces in
    /// logs at `Debug`, in stats as a single counter.
    pub dropped_unknown_changelog: u64,
}

/// Walk Jira issues for one local-timezone day.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    fields(connector = "jira", source_id = %source_id, day = %day)
)]
pub async fn walk_day(
    http: &HttpClient,
    auth: Arc<dyn AuthStrategy>,
    workspace_url: &Url,
    source_id: SourceId,
    source_identities: &[SourceIdentity],
    day: NaiveDate,
    local_tz: FixedOffset,
    cancel: &CancellationToken,
    progress: Option<&ProgressSender>,
    logs: Option<&LogSender>,
) -> Result<WalkOutcome, DayseamError> {
    let (start_utc, end_utc_inclusive) = day_bounds_utc(day, local_tz);
    // The normaliser treats the window as a half-open `[start, end)`
    // range; use the exclusive end (`end_inclusive + 1s`) so a
    // transition at 23:59:59 local is still inside the day.
    let end_utc_exclusive = end_utc_inclusive + ChronoDuration::seconds(1);
    let window = DayWindow {
        start: start_utc,
        end: end_utc_exclusive,
    };

    let Some(self_account_id) = self_account_id(source_identities, source_id, logs) else {
        // No identity means every event would be filtered out; bail
        // with zero events rather than issuing a wasted JQL.
        return Ok(WalkOutcome::default());
    };

    let url =
        workspace_url
            .join("rest/api/3/search/jql")
            .map_err(|e| DayseamError::InvalidConfig {
                code: "jira.config.bad_workspace_url".to_string(),
                message: format!("cannot join `/rest/api/3/search/jql`: {e}"),
            })?;

    let jql = build_jql(start_utc, end_utc_exclusive);

    let mut out = WalkOutcome::default();
    let mut next_page_token: Option<String> = None;
    let paginator = JqlTokenPaginator;

    for page_idx in 0..MAX_PAGES {
        if cancel.is_cancelled() {
            return Err(DayseamError::Cancelled {
                code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "jira walk cancelled".to_string(),
            });
        }

        let body = build_request_body(&jql, next_page_token.as_deref());
        let request = http
            .reqwest()
            .post(url.clone())
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&body);
        let request = auth.authenticate(request).await?;

        let response = http
            .send(request, cancel, progress, logs)
            .await
            .map_err(|e| match e {
                // The SDK surfaces an exhausted 429 retry budget as
                // `RateLimited { code: http.retry_budget_exhausted }`.
                // Rebrand it to the `jira.walk.*` code so the UI's
                // reconnect / rate-limit copy keys on the product-
                // scoped code the plan reserves.
                DayseamError::RateLimited {
                    retry_after_secs, ..
                } => DayseamError::RateLimited {
                    code: error_codes::JIRA_WALK_RATE_LIMITED.to_string(),
                    retry_after_secs,
                },
                other => other,
            })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            // 429 at this point was *not* retried by the SDK (which
            // already classifies 429 as retriable + converts to
            // `RateLimited` after the budget). A non-success 429
            // landing here implies the SDK saw a first-attempt 429
            // without exhausting retries; we still honour the
            // product-scoped code rather than leaving the SDK's
            // internal one exposed.
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(DayseamError::RateLimited {
                    code: error_codes::JIRA_WALK_RATE_LIMITED.to_string(),
                    retry_after_secs: 0,
                });
            }
            let mapped: DayseamError = map_status(Product::Jira, status, body_text).into();
            return Err(mapped);
        }

        let page_body: Value = response
            .json()
            .await
            .map_err(|e| shape_changed(format!("page {page_idx} failed to decode: {e}")))?;

        let page = paginator.parse(page_body).ok_or_else(|| {
            shape_changed("JQL response was not a JSON object / paginator refused the shape")
        })?;

        let issues = page
            .body
            .get("issues")
            .and_then(Value::as_array)
            .ok_or_else(|| shape_changed("JQL response missing `issues` array"))?;

        out.fetched_count = out.fetched_count.saturating_add(issues.len() as u64);

        for issue in issues {
            let issue_events = normalise_issue(
                source_id,
                workspace_url,
                &self_account_id,
                window,
                issue,
                logs,
            )?;
            out.dropped_unknown_changelog = out
                .dropped_unknown_changelog
                .saturating_add(issue_events.dropped_unknown_changelog);
            out.events.extend(issue_events.events);
        }

        match page.next_cursor {
            Some(token) => next_page_token = Some(token),
            None => break,
        }
    }

    // Deterministic order for the orchestrator / report layer.
    out.events.sort_by(|a, b| {
        a.occurred_at
            .cmp(&b.occurred_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(out)
}

/// Build the JQL expression for one UTC window. The `updated` clause
/// uses Jira's `"yyyy/MM/dd HH:mm"` literal syntax (which Jira parses
/// in the server's configured timezone — for Cloud, UTC by default).
fn build_jql(start_utc: DateTime<Utc>, end_utc_exclusive: DateTime<Utc>) -> String {
    let start = start_utc.format("%Y/%m/%d %H:%M").to_string();
    let end = end_utc_exclusive.format("%Y/%m/%d %H:%M").to_string();
    format!(
        "(assignee = currentUser() OR comment ~ currentUser() OR reporter = currentUser()) \
         AND updated >= \"{start}\" AND updated < \"{end}\""
    )
}

fn build_request_body(jql: &str, next_page_token: Option<&str>) -> Value {
    let mut body = json!({
        "jql": jql,
        "fields": REQUESTED_FIELDS,
        "expand": "changelog",
        "maxResults": PAGE_SIZE,
    });
    if let Some(token) = next_page_token {
        body["nextPageToken"] = json!(token);
    }
    body
}

/// UTC start + end (inclusive) of a local day. Kept here alongside the
/// walker so tests can drive the boundary logic without instantiating
/// an `HttpClient`. Matches `connector_gitlab::walk::day_bounds_utc`
/// one-for-one — the two will converge into
/// `dayseam_core::time::day_bounds_utc` in a later MNT task.
pub fn day_bounds_utc(day: NaiveDate, tz: FixedOffset) -> (DateTime<Utc>, DateTime<Utc>) {
    let start_local = tz
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .expect("local midnight resolves unambiguously");
    let end_local = tz
        .from_local_datetime(&day.and_hms_opt(23, 59, 59).expect("valid end-of-day"))
        .single()
        .expect("local eod resolves unambiguously");
    (
        start_local.with_timezone(&Utc),
        end_local.with_timezone(&Utc),
    )
}

/// Pull the Atlassian `accountId` out of `source_identities`. Returns
/// `None` if no identity row of kind [`SourceIdentityKind::AtlassianAccountId`]
/// is registered for `source_id`; in that case the walker bails early
/// with an empty outcome after emitting a warn log.
fn self_account_id(
    identities: &[SourceIdentity],
    source_id: SourceId,
    logs: Option<&LogSender>,
) -> Option<String> {
    let found = identities.iter().find(|i| {
        i.kind == SourceIdentityKind::AtlassianAccountId
            && i.source_id.map(|s| s == source_id).unwrap_or(true)
    });
    match found {
        Some(i) => Some(i.external_actor_id.clone()),
        None => {
            if let Some(tx) = logs {
                tx.send(
                    LogLevel::Warn,
                    None,
                    "jira: no AtlassianAccountId identity for source; walk returns zero events"
                        .to_string(),
                    json!({ "source_id": source_id }),
                );
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_bounds_utc_is_trivial_for_utc() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let (start, end) = day_bounds_utc(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(), utc);
        assert_eq!(start.to_rfc3339(), "2026-04-20T00:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-04-20T23:59:59+00:00");
    }

    #[test]
    fn build_jql_encloses_window_in_updated_clause() {
        let start = Utc.with_ymd_and_hms(2026, 4, 20, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).unwrap();
        let jql = build_jql(start, end);
        assert!(jql.contains("updated >= \"2026/04/20 00:00\""));
        assert!(jql.contains("updated < \"2026/04/21 00:00\""));
        assert!(jql.contains("assignee = currentUser()"));
        assert!(jql.contains("reporter = currentUser()"));
        assert!(jql.contains("comment ~ currentUser()"));
    }

    #[test]
    fn build_request_body_includes_expand_changelog_and_max_results() {
        let body = build_request_body("foo", None);
        assert_eq!(body["jql"], "foo");
        assert_eq!(body["expand"], "changelog");
        assert_eq!(body["maxResults"], PAGE_SIZE);
        assert!(body.get("nextPageToken").is_none());
    }

    #[test]
    fn build_request_body_threads_next_page_token() {
        let body = build_request_body("foo", Some("tok-2"));
        assert_eq!(body["nextPageToken"], "tok-2");
    }

    #[test]
    fn self_account_id_returns_external_actor_id_when_registered() {
        let src = uuid::Uuid::new_v4();
        let identities = vec![SourceIdentity {
            id: uuid::Uuid::new_v4(),
            person_id: uuid::Uuid::new_v4(),
            source_id: Some(src),
            kind: SourceIdentityKind::AtlassianAccountId,
            external_actor_id: "abc-123".into(),
        }];
        assert_eq!(
            self_account_id(&identities, src, None).as_deref(),
            Some("abc-123")
        );
    }

    #[test]
    fn self_account_id_is_none_when_no_atlassian_identity_registered() {
        let src = uuid::Uuid::new_v4();
        let identities = vec![SourceIdentity {
            id: uuid::Uuid::new_v4(),
            person_id: uuid::Uuid::new_v4(),
            source_id: Some(src),
            kind: SourceIdentityKind::GitLabUserId,
            external_actor_id: "17".into(),
        }];
        assert!(self_account_id(&identities, src, None).is_none());
    }
}
