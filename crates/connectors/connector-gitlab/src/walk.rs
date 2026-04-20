//! Day-window walker for the GitLab Events API.
//!
//! Given a [`chrono::NaiveDate`] and a user-local timezone, the walker
//! paginates `GET /api/v4/users/:user_id/events?after=&before=` until
//! either the server returns an empty page or the last row's
//! `created_at` falls out of the UTC window for that local day.
//!
//! Rate-limit (429) handling, backoff, and retry-progress emission
//! are owned by [`connectors_sdk::HttpClient`]; this walker only
//! paginates. Plan invariant 4 ("a 200-commit push does not produce
//! 200 bullets") is enforced in [`crate::normalise`] — the walker
//! emits exactly one [`dayseam_core::ActivityEvent`] per
//! [`crate::events::GitlabEvent`], and per-push enrichment (Task 2)
//! plugs in at the normalise layer.

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, TimeZone, Utc};
use connectors_sdk::{AuthStrategy, HttpClient};
use dayseam_core::{
    error_codes, ActivityEvent, DayseamError, LogLevel, SourceId, SourceIdentity,
    SourceIdentityKind,
};
use dayseam_events::{LogSender, ProgressSender};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::errors::GitlabUpstreamError;
use crate::events::GitlabEvent;
use crate::normalise::normalise_event;

/// Page size we request. GitLab caps this at 100.
const PAGE_SIZE: u32 = 100;

/// Upper bound on pages per day-window sync. A safety net: at 100
/// events/page this allows 2 000 events in a single day for one user,
/// which is well past any real user's output.
const MAX_PAGES: u32 = 20;

/// Outcome of a single-day walk.
#[derive(Debug, Clone)]
pub struct WalkOutcome {
    pub events: Vec<ActivityEvent>,
    pub fetched_count: u64,
    pub filtered_by_identity: u64,
    pub filtered_by_date: u64,
    /// Count of events whose shape we could not recognise and
    /// silently dropped (unknown action, unknown target type). The
    /// caller decides whether to surface this as a warning or as a
    /// `ShapeChanged` log depending on volume.
    pub dropped_by_shape: u64,
}

/// Walk GitLab events for one local-timezone day. The walker authenticates
/// every outbound request via `auth`, paginates, and normalises each row
/// into an [`ActivityEvent`]. The caller supplies `source_identities` so
/// the walker can filter by `author.id` before returning.
#[allow(clippy::too_many_arguments)]
pub async fn walk_day(
    http: &HttpClient,
    auth: Arc<dyn AuthStrategy>,
    base_url: &str,
    user_id: i64,
    source_id: SourceId,
    source_identities: &[SourceIdentity],
    day: NaiveDate,
    local_tz: FixedOffset,
    cancel: &CancellationToken,
    progress: Option<&ProgressSender>,
    logs: Option<&LogSender>,
) -> Result<WalkOutcome, DayseamError> {
    let (start_utc, end_utc) = day_bounds_utc(day, local_tz);
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/v4/users/{user_id}/events");

    // GitLab's `after`/`before` are day-granularity. We still
    // double-filter in post because the UTC window for a local day
    // spans at most two calendar days; filtering in post costs
    // nothing and keeps the walker honest even if the query params
    // silently round.
    let gitlab_after = (start_utc.date_naive() - ChronoDuration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let gitlab_before = (end_utc.date_naive() + ChronoDuration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    let identity_ids = identity_user_ids(source_identities, source_id);

    let mut events: Vec<ActivityEvent> = Vec::new();
    let mut fetched: u64 = 0;
    let mut filtered_by_identity: u64 = 0;
    let mut filtered_by_date: u64 = 0;
    let mut dropped_by_shape: u64 = 0;

    for page in 1..=MAX_PAGES {
        if cancel.is_cancelled() {
            return Err(DayseamError::Cancelled {
                code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "walk cancelled".to_string(),
            });
        }

        let request = http
            .reqwest()
            .get(&url)
            .query(&[
                ("after", gitlab_after.as_str()),
                ("before", gitlab_before.as_str()),
                ("per_page", "100"),
            ])
            .query(&[("page", page.to_string())]);
        let request = auth.authenticate(request).await?;

        let response = http.send(request, cancel, progress, logs).await?;

        let status = response.status();
        if !status.is_success() {
            // HttpClient already translates 429 / 5xx into typed
            // `DayseamError` variants; anything else (404, 400…) we
            // map via `map_status` so the code stays inside the
            // `gitlab.*` namespace.
            let body = response.text().await.unwrap_or_default();
            let mapped: DayseamError = crate::errors::map_status(status, body).into();
            return Err(mapped);
        }

        let page_events: Vec<GitlabEvent> =
            response
                .json()
                .await
                .map_err(|e| GitlabUpstreamError::ShapeChanged {
                    message: format!("events page {page} failed to decode: {e}"),
                })?;

        let page_len = page_events.len();
        fetched = fetched.saturating_add(page_len as u64);

        if page_events.is_empty() {
            debug!(page, "empty page, stopping walk");
            break;
        }

        let mut reached_window_floor = false;
        for ev in page_events.iter() {
            // Identity filter by numeric user id — the v0.1 invariant.
            if !identity_ids.is_empty() && !identity_ids.contains(&ev.author_id) {
                filtered_by_identity = filtered_by_identity.saturating_add(1);
                continue;
            }

            // Day-window filter. `after`/`before` are inclusive on
            // the day boundaries — tighten to the exact UTC window
            // the local day maps to.
            let occurred = ev.created_at;
            if occurred < start_utc {
                reached_window_floor = true;
                filtered_by_date = filtered_by_date.saturating_add(1);
                continue;
            }
            if occurred > end_utc {
                filtered_by_date = filtered_by_date.saturating_add(1);
                continue;
            }

            match normalise_event(source_id, base, ev) {
                Some(n) => events.push(n),
                None => dropped_by_shape = dropped_by_shape.saturating_add(1),
            }
        }

        // Early stop: events come back newest-first, so once a page
        // carries rows older than the window the next page will too.
        if reached_window_floor || page_len < PAGE_SIZE as usize {
            break;
        }
    }

    // Deterministic order: oldest-first by occurred_at, breaking ties
    // by id. The report layer re-sorts, but stable output here makes
    // golden-file tests trivial.
    events.sort_by(|a, b| {
        a.occurred_at
            .cmp(&b.occurred_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(WalkOutcome {
        events,
        fetched_count: fetched,
        filtered_by_identity,
        filtered_by_date,
        dropped_by_shape,
    })
}

/// UTC start + end (exclusive at midnight boundaries) of a local day.
/// Extracted so tests can drive the boundary logic without an HTTP
/// server. Plan Task 1.10 defers moving this to `dayseam-core::time`
/// to MNT-02 in a later phase.
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

fn identity_user_ids(identities: &[SourceIdentity], source_id: SourceId) -> Vec<i64> {
    identities
        .iter()
        .filter(|si| matches!(si.kind, SourceIdentityKind::GitLabUserId))
        .filter(|si| si.source_id.is_none() || si.source_id == Some(source_id))
        .filter_map(|si| si.external_actor_id.parse::<i64>().ok())
        .collect()
}

/// Helper for the rate-limit progress invariant — the connector's
/// `sync` wraps the `walk_day` call with this so a 429 surfaces in
/// the UI with the GitLab code rather than the generic HTTP one.
pub fn emit_rate_limit_log(logs: &LogSender, source_id: SourceId, retry_after_secs: u64) {
    logs.send(
        LogLevel::Warn,
        Some(source_id),
        format!("GitLab rate-limited us; retrying after {retry_after_secs}s"),
        serde_json::json!({
            "code": error_codes::GITLAB_RATE_LIMITED,
            "retry_after_secs": retry_after_secs,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[test]
    fn day_bounds_utc_crosses_utc_midnight_for_negative_offset() {
        // Pacific Standard Time = UTC-08. Local midnight on 2026-04-19
        // == 2026-04-19T08:00:00Z.
        let pst = FixedOffset::west_opt(8 * 3600).unwrap();
        let (start, end) = day_bounds_utc(NaiveDate::from_ymd_opt(2026, 4, 19).unwrap(), pst);
        assert_eq!(start.to_rfc3339(), "2026-04-19T08:00:00+00:00");
        // End-of-day = local 23:59:59 → UTC 07:59:59 on the next day.
        assert_eq!(end.to_rfc3339(), "2026-04-20T07:59:59+00:00");
    }

    #[test]
    fn day_bounds_utc_is_simple_for_utc() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let (start, end) = day_bounds_utc(NaiveDate::from_ymd_opt(2026, 4, 19).unwrap(), utc);
        assert_eq!(start.to_rfc3339(), "2026-04-19T00:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-04-19T23:59:59+00:00");
    }

    #[test]
    fn identity_user_ids_picks_only_gitlab_user_id_rows() {
        let sid: SourceId = uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let other: SourceId =
            uuid::Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let identities = vec![
            SourceIdentity {
                id: uuid::Uuid::new_v4(),
                person_id: uuid::Uuid::new_v4(),
                kind: SourceIdentityKind::GitLabUserId,
                external_actor_id: "17".into(),
                source_id: Some(sid),
            },
            // Wrong kind — ignore.
            SourceIdentity {
                id: uuid::Uuid::new_v4(),
                person_id: uuid::Uuid::new_v4(),
                kind: SourceIdentityKind::GitEmail,
                external_actor_id: "me@example.com".into(),
                source_id: Some(sid),
            },
            // Bound to a different source — ignore.
            SourceIdentity {
                id: uuid::Uuid::new_v4(),
                person_id: uuid::Uuid::new_v4(),
                kind: SourceIdentityKind::GitLabUserId,
                external_actor_id: "99".into(),
                source_id: Some(other),
            },
            // Source-agnostic (source_id = None) — keep.
            SourceIdentity {
                id: uuid::Uuid::new_v4(),
                person_id: uuid::Uuid::new_v4(),
                kind: SourceIdentityKind::GitLabUserId,
                external_actor_id: "23".into(),
                source_id: None,
            },
            // Numeric junk — ignore (parse fails).
            SourceIdentity {
                id: uuid::Uuid::new_v4(),
                person_id: uuid::Uuid::new_v4(),
                kind: SourceIdentityKind::GitLabUserId,
                external_actor_id: "not-a-number".into(),
                source_id: Some(sid),
            },
        ];
        let mut ids = identity_user_ids(&identities, sid);
        ids.sort();
        assert_eq!(ids, vec![17, 23]);
    }
}
