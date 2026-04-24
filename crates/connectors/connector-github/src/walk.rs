//! Day-window walker for GitHub events.
//!
//! Given a [`chrono::NaiveDate`] + the user's local timezone, the
//! walker combines two upstream surfaces:
//!
//! 1. `GET /users/{login}/events` — the authenticated-user events
//!    stream. Covers every event type the normaliser knows how to
//!    emit (PullRequest, PullRequestReview, PullRequestReviewComment,
//!    Issues, IssueComment). Paginates via
//!    [`crate::pagination::next_link`] until the page's
//!    newest-last row falls out of the window, or the `Link` header
//!    stops advertising a `rel="next"`.
//! 2. `GET /search/issues?q=involves:<login>+updated:<start>..<end>`
//!    — catches PRs / issues on repos where an org policy restricts
//!    the events endpoint (private-repo owner restrictions), plus
//!    rows the 90-day events-API truncation drops. Synthesises
//!    `opened` / `closed` / `merged` events when the row's
//!    `created_at` / `closed_at` / `merged_at` falls in the window,
//!    then dedupes against the events stream on
//!    `(ActivityKind, external_id)` — events-stream wins on
//!    conflict because its payload carries the actor + actual
//!    action, while search/issues only carries the snapshot.
//!
//! Rate-limit (429) + 5xx retry handling is owned by
//! [`connectors_sdk::HttpClient`]; this walker only paginates. Hard
//! errors (401, 410) fail the walk with a `github.*` typed code.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, TimeZone, Utc};
use connectors_sdk::{AuthStrategy, HttpClient};
use dayseam_core::{
    error_codes, ActivityEvent, ActivityKind, DayseamError, LogLevel, SourceId, SourceIdentity,
    SourceIdentityKind,
};
use dayseam_events::{LogSender, ProgressSender};
use reqwest::Url;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::errors::{map_status, GithubUpstreamError};
use crate::events::{GithubEvent, GithubSearchIssue, GithubSearchPage};
use crate::normalise::normalise_event;
use crate::pagination::parse_next_from_link_header;
use crate::rollup::collapse_rapid_reviews;

/// Upper bound on pages per endpoint. At 100 rows/page this allows
/// 3 000 rows in a single day for one user — well past any real
/// user's output; the cap is a safety net against a paginator that
/// never terminates.
const MAX_PAGES: u32 = 30;

/// Page size we request. GitHub caps `/users/:login/events` at 100
/// and `/search/issues` at 100.
const PAGE_SIZE: u32 = 100;

/// Outcome of a single-day walk.
#[derive(Debug, Default, Clone)]
pub struct WalkOutcome {
    pub events: Vec<ActivityEvent>,
    pub fetched_count: u64,
    pub filtered_by_identity: u64,
    pub filtered_by_date: u64,
    /// Count of events whose shape we could not recognise and
    /// silently dropped.
    pub dropped_by_shape: u64,
    /// Count of rows the dedup pass collapsed (search stream hits
    /// that duplicated an events-stream hit).
    pub deduped_by_external_id: u64,
}

/// Walk GitHub events for one local-timezone day.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    fields(connector = "github", source_id = %source_id, day = %day)
)]
pub async fn walk_day(
    http: &HttpClient,
    auth: Arc<dyn AuthStrategy>,
    api_base_url: &Url,
    source_id: SourceId,
    source_identities: &[SourceIdentity],
    day: NaiveDate,
    local_tz: FixedOffset,
    cancel: &CancellationToken,
    progress: Option<&ProgressSender>,
    logs: Option<&LogSender>,
) -> Result<WalkOutcome, DayseamError> {
    let (start_utc, end_utc_inclusive) = day_bounds_utc(day, local_tz);
    let end_utc_exclusive = end_utc_inclusive + ChronoDuration::seconds(1);

    let Some(self_ident) = self_identity(source_identities, source_id, logs) else {
        return Ok(WalkOutcome::default());
    };

    let mut out = WalkOutcome::default();

    // Events endpoint — primary source of truth.
    walk_user_events(
        http,
        auth.clone(),
        api_base_url,
        source_id,
        &self_ident,
        start_utc,
        end_utc_exclusive,
        cancel,
        progress,
        logs,
        &mut out,
    )
    .await?;

    // Capture what events stream already surfaced — dedup key is
    // (kind, external_id) because the search endpoint only produces
    // opened / closed / merged kinds, and those share the same
    // `owner/repo#N` external_id shape as the events arm.
    let already_seen: HashSet<(ActivityKind, String)> = out
        .events
        .iter()
        .map(|e| (e.kind, e.external_id.clone()))
        .collect();

    // Search endpoint — catches private-repo activity the events
    // stream misses.
    walk_search_issues(
        http,
        auth,
        api_base_url,
        source_id,
        &self_ident,
        start_utc,
        end_utc_exclusive,
        cancel,
        progress,
        logs,
        &already_seen,
        &mut out,
    )
    .await?;

    // Rapid-review collapse (N reviews in 60s on the same PR fold
    // into one).
    let pre_collapse = out.events.len();
    out.events = collapse_rapid_reviews(std::mem::take(&mut out.events));
    let collapsed = pre_collapse.saturating_sub(out.events.len());
    if collapsed > 0 {
        if let Some(tx) = logs {
            tx.send(
                LogLevel::Debug,
                Some(source_id),
                format!("github: collapsed {collapsed} rapid-review events"),
                serde_json::json!({
                    "collapsed_count": collapsed,
                    "window_seconds": crate::rollup::RAPID_REVIEW_WINDOW_SECONDS,
                }),
            );
        }
    }

    // Deterministic order: oldest-first by occurred_at, breaking
    // ties by id.
    out.events.sort_by(|a, b| {
        a.occurred_at
            .cmp(&b.occurred_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn walk_user_events(
    http: &HttpClient,
    auth: Arc<dyn AuthStrategy>,
    api_base_url: &Url,
    source_id: SourceId,
    self_ident: &SelfIdentity,
    start_utc: DateTime<Utc>,
    end_utc_exclusive: DateTime<Utc>,
    cancel: &CancellationToken,
    progress: Option<&ProgressSender>,
    logs: Option<&LogSender>,
    out: &mut WalkOutcome,
) -> Result<(), DayseamError> {
    let initial = api_base_url
        .join(&format!("users/{}/events", self_ident.login))
        .map_err(|e| DayseamError::InvalidConfig {
            code: "github.config.bad_api_base_url".to_string(),
            message: format!("cannot join `/users/:login/events`: {e}"),
        })?;

    let mut next_url: Option<String> = Some(initial.to_string());
    // Dedup within the events stream keyed by the raw GitHub event
    // id — unique per row in `/users/:login/events`. Keying by
    // `(kind, external_id)` here would incorrectly collapse multiple
    // distinct-but-legitimate reviews on the same PR before the
    // rapid-review rollup gets to see them; belt-and-braces against
    // GitHub's very-rare "same event served on two pages during a
    // stream reshuffle" mode, not a cross-PR collapse.
    let mut seen_event_ids: HashSet<String> = HashSet::new();

    for page in 0..MAX_PAGES {
        if cancel.is_cancelled() {
            return Err(DayseamError::Cancelled {
                code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "github walk cancelled".to_string(),
            });
        }
        let Some(url) = next_url.take() else {
            break;
        };

        let mut req = http
            .reqwest()
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if page == 0 {
            // Only the first request sets the page size; subsequent
            // pages follow the Link header which carries its own
            // `per_page`.
            req = req.query(&[("per_page", PAGE_SIZE.to_string())]);
        }
        let req = auth.authenticate(req).await?;
        let response = http.send(req, cancel, progress, logs).await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let mapped: DayseamError = map_status(status, body).into();
            return Err(mapped);
        }

        // Parse Link header *before* consuming body.
        let link_header = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let page_events: Vec<GithubEvent> =
            response
                .json()
                .await
                .map_err(|e| GithubUpstreamError::ShapeChanged {
                    message: format!("events page {page} failed to decode: {e}"),
                })?;

        let page_len = page_events.len();
        out.fetched_count = out.fetched_count.saturating_add(page_len as u64);

        if page_events.is_empty() {
            debug!(page, "empty events page, stopping walk");
            break;
        }

        let mut reached_window_floor = false;
        for ev in &page_events {
            if ev.actor.id != self_ident.user_id {
                out.filtered_by_identity = out.filtered_by_identity.saturating_add(1);
                continue;
            }
            if ev.created_at < start_utc {
                reached_window_floor = true;
                out.filtered_by_date = out.filtered_by_date.saturating_add(1);
                continue;
            }
            if ev.created_at >= end_utc_exclusive {
                out.filtered_by_date = out.filtered_by_date.saturating_add(1);
                continue;
            }
            if !seen_event_ids.insert(ev.id.clone()) {
                // Same GitHub event id already processed on an earlier
                // page — collapse silently. Not counted against
                // `deduped_by_external_id` (that metric is reserved
                // for the search/events cross-stream collapse).
                continue;
            }
            match normalise_event(source_id, ev) {
                Some(normalised) => {
                    out.events.push(normalised);
                }
                None => {
                    out.dropped_by_shape = out.dropped_by_shape.saturating_add(1);
                    if let Some(tx) = logs {
                        tx.send(
                            LogLevel::Debug,
                            Some(source_id),
                            format!(
                                "github: dropped unrecognised event type={} id={}",
                                ev.event_type, ev.id
                            ),
                            serde_json::json!({
                                "event_type": ev.event_type,
                                "event_id": ev.id,
                            }),
                        );
                    }
                }
            }
        }

        if reached_window_floor {
            // Events come back newest-first; one row older than the
            // window means every subsequent page is too.
            break;
        }

        next_url = link_header
            .as_deref()
            .and_then(parse_next_from_link_header)
            .map(|u| u.to_string());
    }

    // DAY-122 / C-2. If we exited the loop with `next_url` still
    // carrying a URL, the paginator kept advertising `rel="next"`
    // past `MAX_PAGES`. Pre-C-2 we silently broke out, truncating
    // the day's data with no signal to the UI or logs. Emit a
    // typed `Internal` error instead so the orchestrator surfaces
    // the cap trip and the run fails visibly. `break` on
    // `reached_window_floor` sets `next_url = None` implicitly
    // (via the `let Some(url) = next_url.take() else { break };`
    // pattern on the *next* iteration that never runs) — but the
    // window-floor branch above `break`s *before* re-assigning
    // `next_url`, so `next_url` can be `Some` here only if we
    // hit the cap with more pages genuinely available.
    if next_url.is_some() {
        return Err(DayseamError::Internal {
            code: error_codes::GITHUB_PAGINATION_CYCLE_GUARD_TRIPPED.to_string(),
            message: format!(
                "github events pagination cap hit: {MAX_PAGES} pages × {PAGE_SIZE} rows \
                 exceeded for source_id={source_id} without a rel=\"next\" terminator"
            ),
        });
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn walk_search_issues(
    http: &HttpClient,
    auth: Arc<dyn AuthStrategy>,
    api_base_url: &Url,
    source_id: SourceId,
    self_ident: &SelfIdentity,
    start_utc: DateTime<Utc>,
    end_utc_exclusive: DateTime<Utc>,
    cancel: &CancellationToken,
    progress: Option<&ProgressSender>,
    logs: Option<&LogSender>,
    already_seen: &HashSet<(ActivityKind, String)>,
    out: &mut WalkOutcome,
) -> Result<(), DayseamError> {
    // GitHub's `/search/issues` `updated` clause accepts ISO-8601
    // timestamps directly: `updated:2026-04-20T00:00:00Z..2026-04-21T00:00:00Z`.
    let start_iso = start_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let end_iso = end_utc_exclusive.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let q = format!(
        "involves:{login} updated:{start}..{end}",
        login = self_ident.login,
        start = start_iso,
        end = end_iso,
    );

    let initial = api_base_url
        .join("search/issues")
        .map_err(|e| DayseamError::InvalidConfig {
            code: "github.config.bad_api_base_url".to_string(),
            message: format!("cannot join `/search/issues`: {e}"),
        })?;

    let mut next_url: Option<String> = Some(initial.to_string());
    let mut first_page = true;
    let mut new_seen: HashSet<(ActivityKind, String)> = HashSet::new();

    for page in 0..MAX_PAGES {
        if cancel.is_cancelled() {
            return Err(DayseamError::Cancelled {
                code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "github search walk cancelled".to_string(),
            });
        }
        let Some(url) = next_url.take() else {
            break;
        };

        let mut req = http
            .reqwest()
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if first_page {
            req = req.query(&[("q", q.as_str()), ("per_page", &PAGE_SIZE.to_string())]);
            first_page = false;
        }
        let req = auth.authenticate(req).await?;
        let response = http.send(req, cancel, progress, logs).await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let mapped: DayseamError = map_status(status, body).into();
            return Err(mapped);
        }

        let link_header = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let page_body: GithubSearchPage =
            response
                .json()
                .await
                .map_err(|e| GithubUpstreamError::ShapeChanged {
                    message: format!("search/issues page {page} failed to decode: {e}"),
                })?;

        out.fetched_count = out
            .fetched_count
            .saturating_add(page_body.items.len() as u64);

        if page_body.items.is_empty() {
            break;
        }

        for hit in page_body.items {
            let Some(user) = hit.user.as_ref() else {
                continue;
            };
            if user.id != self_ident.user_id {
                out.filtered_by_identity = out.filtered_by_identity.saturating_add(1);
                continue;
            }
            for synthesised in
                synthesise_events_from_search_hit(&hit, source_id, start_utc, end_utc_exclusive)
            {
                let key = (synthesised.kind, synthesised.external_id.clone());
                if already_seen.contains(&key) || !new_seen.insert(key) {
                    out.deduped_by_external_id = out.deduped_by_external_id.saturating_add(1);
                    continue;
                }
                out.events.push(synthesised);
            }
        }

        next_url = link_header
            .as_deref()
            .and_then(parse_next_from_link_header)
            .map(|u| u.to_string());
    }

    // DAY-122 / C-2. Same cycle-guard trip semantics as
    // [`walk_user_events`] — a `search/issues` paginator that
    // keeps advertising `rel="next"` past `MAX_PAGES` is almost
    // certainly a bug (search queries with >3000 matching rows
    // are far past realistic single-day activity for one user
    // and hit the 1000-row API cap long before our cap). Surface
    // it as `Internal` rather than silently truncating.
    if next_url.is_some() {
        return Err(DayseamError::Internal {
            code: error_codes::GITHUB_PAGINATION_CYCLE_GUARD_TRIPPED.to_string(),
            message: format!(
                "github search/issues pagination cap hit: {MAX_PAGES} pages × {PAGE_SIZE} rows \
                 exceeded for source_id={source_id} without a rel=\"next\" terminator"
            ),
        });
    }

    Ok(())
}

/// Turn a `/search/issues` hit into zero-or-more synthetic
/// [`ActivityEvent`]s when its `created_at` / `closed_at` /
/// `merged_at` timestamps fall inside the walk window.
fn synthesise_events_from_search_hit(
    hit: &GithubSearchIssue,
    source_id: SourceId,
    start_utc: DateTime<Utc>,
    end_utc_exclusive: DateTime<Utc>,
) -> Vec<ActivityEvent> {
    let Some(full_name) = hit.repo_full_name() else {
        return Vec::new();
    };
    let is_pr = hit.is_pull_request();
    let external_id = format!("{full_name}#{number}", number = hit.number);
    let mut out = Vec::new();

    let in_window = |t: &DateTime<Utc>| -> bool { *t >= start_utc && *t < end_utc_exclusive };

    if let Some(created) = hit.created_at {
        if in_window(&created) {
            let kind = if is_pr {
                ActivityKind::GitHubPullRequestOpened
            } else {
                ActivityKind::GitHubIssueOpened
            };
            out.push(search_hit_to_activity_event(
                hit,
                &full_name,
                &external_id,
                kind,
                created,
                source_id,
            ));
        }
    }

    if let Some(closed) = hit.closed_at {
        if in_window(&closed) {
            let kind = if is_pr {
                // Search-issues doesn't distinguish merged-vs-closed
                // for PRs. We default to Closed; the events-stream
                // arm would have won the dedup if the PR actually
                // merged (PullRequestEvent.closed_with_merged=true
                // produces the Merged kind there).
                ActivityKind::GitHubPullRequestClosed
            } else {
                ActivityKind::GitHubIssueClosed
            };
            out.push(search_hit_to_activity_event(
                hit,
                &full_name,
                &external_id,
                kind,
                closed,
                source_id,
            ));
        }
    }

    out
}

fn search_hit_to_activity_event(
    hit: &GithubSearchIssue,
    repo_full_name: &str,
    external_id: &str,
    kind: ActivityKind,
    occurred_at: DateTime<Utc>,
    source_id: SourceId,
) -> ActivityEvent {
    use dayseam_core::{Actor, EntityKind, EntityRef, Link, Privacy, RawRef};

    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(
        &source_id_str,
        external_id,
        crate::normalise::kind_token(kind),
    );

    let title = match kind {
        ActivityKind::GitHubPullRequestOpened => format!("Opened PR: {}", hit.title),
        ActivityKind::GitHubPullRequestClosed => format!("Closed PR: {}", hit.title),
        ActivityKind::GitHubIssueOpened => format!("Opened issue: {}", hit.title),
        ActivityKind::GitHubIssueClosed => format!("Closed issue: {}", hit.title),
        _ => hit.title.clone(),
    };

    let user = hit.user.clone().unwrap_or(crate::events::GithubUserRef {
        id: 0,
        login: String::from("unknown"),
        html_url: None,
    });
    let actor = Actor {
        display_name: user.login.clone(),
        email: None,
        external_id: Some(user.id.to_string()),
    };
    let repo_label = repo_full_name.rsplit('/').next().map(|s| s.to_string());
    let entity_kind = if hit.is_pull_request() {
        EntityKind::GitHubPullRequest
    } else {
        EntityKind::GitHubIssue
    };

    // Jira-key enrichment happens downstream in the shared
    // `dayseam_report::extract_ticket_keys` pass — see DAY-112 +
    // `docs/dogfood/2026-04-20-cross-source-enrichment-parity-audit.md`.
    let entities = vec![
        EntityRef {
            kind: EntityKind::GitHubRepo,
            external_id: repo_full_name.to_string(),
            label: repo_label,
        },
        EntityRef {
            kind: entity_kind,
            external_id: external_id.to_string(),
            label: Some(format!("#{}", hit.number)),
        },
    ];

    ActivityEvent {
        id,
        source_id,
        external_id: external_id.to_string(),
        kind,
        occurred_at,
        actor,
        title,
        body: None,
        links: vec![Link {
            url: hit.html_url.clone(),
            label: Some(format!("#{}", hit.number)),
        }],
        entities,
        parent_external_id: Some(external_id.to_string()),
        metadata: serde_json::json!({
            "source": "search/issues",
            "repo": repo_full_name,
            "number": hit.number,
        }),
        raw_ref: RawRef {
            storage_key: format!("github:search:{}", hit.id),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

/// UTC start + end (inclusive) of a local day. Mirrors
/// `connector_gitlab::walk::day_bounds_utc`.
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

/// Resolved self identity — both the numeric user id (for event
/// actor filtering) and the login (for URL composition).
struct SelfIdentity {
    user_id: i64,
    login: String,
}

/// Pull self identity from the registered identities. We need:
/// - A [`SourceIdentityKind::GitHubUserId`] row — the authoritative
///   id used for filtering `event.actor.id`.
/// - A [`SourceIdentityKind::GitHubLogin`] row — optional, but
///   required for composing `/users/{login}/events`. When absent
///   (identity seed only stores the user id, or the login row
///   drifted), we fall back to `user-{id}` which GitHub's events
///   endpoint will 404 on; `list_identities` in DAY-95 seeds both
///   rows so production paths carry both.
fn self_identity(
    identities: &[SourceIdentity],
    source_id: SourceId,
    logs: Option<&LogSender>,
) -> Option<SelfIdentity> {
    let user_id_row = identities.iter().find(|i| {
        i.kind == SourceIdentityKind::GitHubUserId
            && (i.source_id.is_none() || i.source_id == Some(source_id))
    });
    let user_id = match user_id_row.and_then(|r| r.external_actor_id.parse::<i64>().ok()) {
        Some(id) => id,
        None => {
            if let Some(tx) = logs {
                tx.send(
                    LogLevel::Warn,
                    Some(source_id),
                    "github: no GitHubUserId identity for source; walk returns zero events"
                        .to_string(),
                    serde_json::json!({ "source_id": source_id }),
                );
            }
            return None;
        }
    };

    let login_row = identities.iter().find(|i| {
        i.kind == SourceIdentityKind::GitHubLogin
            && (i.source_id.is_none() || i.source_id == Some(source_id))
    });
    let login = match login_row {
        Some(r) => r.external_actor_id.clone(),
        None => {
            if let Some(tx) = logs {
                tx.send(
                    LogLevel::Warn,
                    Some(source_id),
                    "github: no GitHubLogin identity for source; walk cannot compose user URL"
                        .to_string(),
                    serde_json::json!({ "source_id": source_id }),
                );
            }
            return None;
        }
    };

    Some(SelfIdentity { user_id, login })
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
    fn self_identity_requires_both_user_id_and_login() {
        let src = uuid::Uuid::new_v4();
        let user_only = vec![SourceIdentity {
            id: uuid::Uuid::new_v4(),
            person_id: uuid::Uuid::new_v4(),
            kind: SourceIdentityKind::GitHubUserId,
            external_actor_id: "17".into(),
            source_id: Some(src),
        }];
        assert!(self_identity(&user_only, src, None).is_none());

        let both = vec![
            SourceIdentity {
                id: uuid::Uuid::new_v4(),
                person_id: uuid::Uuid::new_v4(),
                kind: SourceIdentityKind::GitHubUserId,
                external_actor_id: "17".into(),
                source_id: Some(src),
            },
            SourceIdentity {
                id: uuid::Uuid::new_v4(),
                person_id: uuid::Uuid::new_v4(),
                kind: SourceIdentityKind::GitHubLogin,
                external_actor_id: "vedanth".into(),
                source_id: Some(src),
            },
        ];
        let ident = self_identity(&both, src, None).unwrap();
        assert_eq!(ident.user_id, 17);
        assert_eq!(ident.login, "vedanth");
    }

    #[test]
    fn synthesise_events_emits_opened_when_created_in_window() {
        let hit = GithubSearchIssue {
            id: 1,
            number: 42,
            title: "Add payments".into(),
            html_url: "https://github.com/modulr/foo/pull/42".into(),
            state: "open".into(),
            user: Some(crate::events::GithubUserRef {
                id: 17,
                login: "vedanth".into(),
                html_url: None,
            }),
            assignees: vec![],
            pull_request: Some(serde_json::json!({"url": "..."})),
            created_at: Some(Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap()),
            updated_at: None,
            closed_at: None,
            repository_url: Some("https://api.github.com/repos/modulr/foo".into()),
        };
        let sid = uuid::Uuid::new_v4();
        let start = Utc.with_ymd_and_hms(2026, 4, 20, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).unwrap();
        let events = synthesise_events_from_search_hit(&hit, sid, start, end);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ActivityKind::GitHubPullRequestOpened);
        assert_eq!(events[0].external_id, "modulr/foo#42");
    }

    #[test]
    fn synthesise_events_emits_nothing_when_repo_url_missing() {
        let hit = GithubSearchIssue {
            id: 1,
            number: 42,
            title: "Add payments".into(),
            html_url: "https://example/".into(),
            state: "open".into(),
            user: Some(crate::events::GithubUserRef {
                id: 17,
                login: "vedanth".into(),
                html_url: None,
            }),
            assignees: vec![],
            pull_request: None,
            created_at: Some(Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap()),
            updated_at: None,
            closed_at: None,
            repository_url: None,
        };
        let sid = uuid::Uuid::new_v4();
        let start = Utc.with_ymd_and_hms(2026, 4, 20, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).unwrap();
        assert!(synthesise_events_from_search_hit(&hit, sid, start, end).is_empty());
    }
}
