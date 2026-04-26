//! Day-window walker for Outlook calendar events.
//!
//! Given a local-timezone [`chrono::NaiveDate`], the walker:
//!
//! 1. Computes the UTC half-open bounds for that local day.
//! 2. Calls `GET /me/calendarView?startDateTime=<start>&endDateTime=<end>`
//!    on Microsoft Graph with `Prefer: outlook.timezone="UTC"` so
//!    every timestamp comes back in UTC regardless of the tenant's
//!    default zone. Graph's `calendarView` expands recurring series
//!    into per-occurrence rows within the window, so we don't need
//!    to hydrate master events.
//! 3. Paginates through the `@odata.nextLink` cursor until exhausted
//!    or [`MAX_PAGES`] trips, whichever comes first. The cap guards
//!    against a server that keeps advertising a next-link — real
//!    calendars rarely exceed a page.
//! 4. Drops rows the user didn't actually attend:
//!    * `isCancelled == true` → cancelled
//!    * `isAllDay == true` → not a real meeting
//!    * `showAs == "free"` → blocked-off "free" time
//!    * attendee response is `declined` → skipped
//!    * the user is not the organiser and not on the attendee list
//!      at all → the event was only visible because the calendar was
//!      shared; not an attendance signal.
//! 5. Normalises each surviving row via [`crate::normalise::normalise_event`].
//! 6. Sorts the result oldest-first by `occurred_at` so the rollup
//!    layer sees a stable order.
//!
//! Rate-limit (429) + 5xx retry handling is owned by
//! [`connectors_sdk::HttpClient`]; this walker only paginates. Auth
//! errors (401, 403, 410) terminate the walk with a typed
//! `outlook.*` error code.

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, TimeZone, Utc};
use connectors_sdk::{AuthStrategy, HttpClient};
use dayseam_core::{
    error_codes, ActivityEvent, Actor, DayseamError, LogLevel, SourceId, SourceIdentity,
    SourceIdentityKind,
};
use dayseam_events::{LogSender, ProgressSender};
use reqwest::Url;
use tokio_util::sync::CancellationToken;

use crate::errors::{map_status, OutlookUpstreamError};
use crate::events::{GraphEvent, GraphEventsPage};
use crate::normalise::normalise_event;

/// Upper bound on pages per day. Each page carries up to 50 events
/// (the Graph default for `calendarView`); at 50 pages that's 2 500
/// meetings in a day — a clear safety-net rather than a real user
/// limit. See [`crate::walk`] module docs for the tripping semantics.
const MAX_PAGES: u32 = 50;

/// `$top` we request from Graph. 50 matches Graph's documented
/// default for `calendarView`; bumping would risk 400s from the
/// server.
const PAGE_SIZE: u32 = 50;

/// Outcome of a single-day walk. Mirrors the connector-github shape
/// so the orchestrator's per-source metrics row can stay uniform.
#[derive(Debug, Default, Clone)]
pub struct WalkOutcome {
    pub events: Vec<ActivityEvent>,
    /// Raw rows returned by Graph before any filtering.
    pub fetched_count: u64,
    /// Rows dropped because the calendar owner wasn't an attendee.
    pub filtered_by_identity: u64,
    /// Rows dropped because `isCancelled`, `isAllDay`, declined
    /// response, or `showAs == free`.
    pub filtered_by_status: u64,
    /// Rows whose shape we could not recognise (unparseable
    /// timestamp etc.); counted separately from status drops so the
    /// dogfood log can distinguish "Graph changed on us" from "user
    /// had weird calendar".
    pub dropped_by_shape: u64,
}

/// Walk Outlook calendar events for one local-timezone day. See the
/// module docs for the full filter set.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    fields(connector = "outlook", source_id = %source_id, day = %day)
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
    let (start_utc, end_utc_exclusive) = day_bounds_utc(day, local_tz);

    let Some(self_actor) = self_actor(source_identities, source_id, logs) else {
        return Ok(WalkOutcome::default());
    };

    let mut out = WalkOutcome::default();

    let initial = build_initial_url(api_base_url, start_utc, end_utc_exclusive, PAGE_SIZE)
        .map_err(|e| DayseamError::InvalidConfig {
            code: "outlook.config.bad_api_base_url".to_string(),
            message: format!("cannot join `/me/calendarView`: {e}"),
        })?;

    let mut next_url: Option<String> = Some(initial.to_string());

    for page_idx in 0..MAX_PAGES {
        if cancel.is_cancelled() {
            return Err(DayseamError::Cancelled {
                code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "outlook walk cancelled".to_string(),
            });
        }
        let Some(url) = next_url.take() else {
            break;
        };

        let req = http
            .reqwest()
            .get(&url)
            // Force Graph to return timestamps in UTC regardless of
            // the tenant's default. Pairs with the normaliser's
            // UTC-naive parse assumption.
            .header("Prefer", "outlook.timezone=\"UTC\"")
            .header("Accept", "application/json");
        let req = auth.authenticate(req).await?;
        let response = http.send(req, cancel, progress, logs).await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let mapped: DayseamError = map_status(status, body).into();
            return Err(mapped);
        }

        let page: GraphEventsPage =
            response
                .json()
                .await
                .map_err(|e| OutlookUpstreamError::ShapeChanged {
                    message: format!("calendarView page {page_idx} failed to decode: {e}"),
                })?;

        let page_len = page.value.len();
        out.fetched_count = out.fetched_count.saturating_add(page_len as u64);

        for ev in &page.value {
            if !keeps_event(ev, &self_actor, &mut out, source_id, logs) {
                continue;
            }
            match normalise_event(source_id, &self_actor, ev) {
                Ok(normalised) => out.events.push(normalised),
                Err(err) => {
                    out.dropped_by_shape = out.dropped_by_shape.saturating_add(1);
                    if let Some(tx) = logs {
                        tx.send(
                            LogLevel::Debug,
                            Some(source_id),
                            format!("outlook: dropped event id={} (shape={err:?})", ev.id),
                            serde_json::json!({
                                "event_id": ev.id,
                                "error": format!("{err:?}"),
                            }),
                        );
                    }
                }
            }
        }

        next_url = page.next_link.clone();
    }

    if next_url.is_some() {
        // We hit the page cap before Graph stopped handing out
        // next-links. Treat this the same way the GitHub walker does
        // — surface as a typed Internal error so the run fails
        // visibly rather than silently truncating the day.
        return Err(DayseamError::Internal {
            code: "outlook.pagination.cap_tripped".to_string(),
            message: format!(
                "outlook calendarView pagination cap hit: {MAX_PAGES} pages × {PAGE_SIZE} rows \
                 exceeded for source_id={source_id} without an exhausted next-link"
            ),
        });
    }

    out.events.sort_by(|a, b| {
        a.occurred_at
            .cmp(&b.occurred_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(out)
}

/// Decide whether to keep a Graph event after the filter gauntlet.
/// Returns `true` when the event should be normalised; the `false`
/// path bumps the appropriate counter on `out` and emits a debug
/// log. Kept as a freestanding function so every filter lives in one
/// place and unit tests can exercise each branch without booting an
/// HTTP client.
fn keeps_event(
    ev: &GraphEvent,
    self_actor: &Actor,
    out: &mut WalkOutcome,
    source_id: SourceId,
    logs: Option<&LogSender>,
) -> bool {
    if ev.is_cancelled {
        out.filtered_by_status = out.filtered_by_status.saturating_add(1);
        return false;
    }
    if ev.is_all_day {
        out.filtered_by_status = out.filtered_by_status.saturating_add(1);
        return false;
    }
    if ev
        .show_as
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("free"))
    {
        out.filtered_by_status = out.filtered_by_status.saturating_add(1);
        return false;
    }

    let attendance = user_attendance(ev, self_actor);
    match attendance {
        Attendance::Organizer => true,
        Attendance::AcceptedOrTentative => true,
        Attendance::Declined => {
            out.filtered_by_status = out.filtered_by_status.saturating_add(1);
            false
        }
        Attendance::NotInvolved => {
            out.filtered_by_identity = out.filtered_by_identity.saturating_add(1);
            if let Some(tx) = logs {
                tx.send(
                    LogLevel::Debug,
                    Some(source_id),
                    format!("outlook: event id={} owner not on attendee list", ev.id),
                    serde_json::json!({
                        "event_id": ev.id,
                    }),
                );
            }
            false
        }
    }
}

/// Possible attendance states for one event, resolved from organiser
/// email + attendee list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attendance {
    Organizer,
    AcceptedOrTentative,
    Declined,
    NotInvolved,
}

/// Resolve the calendar owner's attendance state for one event.
fn user_attendance(ev: &GraphEvent, self_actor: &Actor) -> Attendance {
    let self_email = self_actor.email.as_deref();
    if let Some(email) = self_email {
        let organiser_email = ev
            .organizer
            .as_ref()
            .and_then(|o| o.email_address.as_ref())
            .and_then(|e| e.address.as_deref());
        if organiser_email.is_some_and(|addr| addr.eq_ignore_ascii_case(email)) {
            return Attendance::Organizer;
        }
        for a in &ev.attendees {
            let a_email = a.email_address.as_ref().and_then(|e| e.address.as_deref());
            if a_email.is_some_and(|addr| addr.eq_ignore_ascii_case(email)) {
                return match a
                    .status
                    .as_ref()
                    .and_then(|s| s.response.as_deref())
                    .map(|s| s.to_ascii_lowercase())
                    .as_deref()
                {
                    Some("declined") => Attendance::Declined,
                    _ => Attendance::AcceptedOrTentative,
                };
            }
        }
    }
    Attendance::NotInvolved
}

/// Build the first-page `calendarView` URL. Subsequent pages follow
/// the `@odata.nextLink` Graph returns verbatim, so we only format
/// once.
fn build_initial_url(
    api_base_url: &Url,
    start_utc: DateTime<Utc>,
    end_utc_exclusive: DateTime<Utc>,
    page_size: u32,
) -> Result<Url, url::ParseError> {
    let mut url = api_base_url.join("me/calendarView")?;
    url.query_pairs_mut()
        .append_pair("startDateTime", &start_utc.to_rfc3339())
        .append_pair("endDateTime", &end_utc_exclusive.to_rfc3339())
        .append_pair("$top", &page_size.to_string())
        .append_pair(
            "$select",
            "id,subject,bodyPreview,isCancelled,isAllDay,sensitivity,start,end,\
             organizer,attendees,isOnlineMeeting,showAs",
        )
        .append_pair("$orderby", "start/dateTime");
    Ok(url)
}

/// Half-open UTC bounds for one local-timezone day.
fn day_bounds_utc(day: NaiveDate, local_tz: FixedOffset) -> (DateTime<Utc>, DateTime<Utc>) {
    let local_start = local_tz
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("00:00:00 is a valid time"))
        .single()
        .expect("midnight always resolves in a fixed-offset zone");
    let start_utc = local_start.with_timezone(&Utc);
    let end_utc = start_utc + ChronoDuration::days(1);
    (start_utc, end_utc)
}

/// Resolve the calendar-owning identity from the source's identity
/// rows. We prefer the Graph object id (stable, never recycled) but
/// fall back to UPN if the object-id row is missing; a DAY-202
/// source without either is a bug in the IPC layer and we log +
/// skip the walk.
fn self_actor(
    identities: &[SourceIdentity],
    source_id: SourceId,
    logs: Option<&LogSender>,
) -> Option<Actor> {
    let object_id = identities
        .iter()
        .find(|i| i.kind == SourceIdentityKind::OutlookUserObjectId)
        .map(|i| i.external_actor_id.clone());
    let upn = identities
        .iter()
        .find(|i| i.kind == SourceIdentityKind::OutlookUserPrincipalName)
        .map(|i| i.external_actor_id.clone());

    if object_id.is_none() && upn.is_none() {
        if let Some(tx) = logs {
            tx.send(
                LogLevel::Warn,
                Some(source_id),
                "outlook: source has no outlook_user_object_id or outlook_user_principal_name \
                 identity row; skipping walk"
                    .to_string(),
                serde_json::json!({
                    "source_id": source_id.to_string(),
                }),
            );
        }
        return None;
    }

    Some(Actor {
        display_name: upn
            .clone()
            .unwrap_or_else(|| object_id.clone().unwrap_or_default()),
        email: upn,
        external_id: object_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        GraphAttendee, GraphAttendeeRef, GraphAttendeeStatus, GraphDateTime, GraphEmailAddress,
        GraphEvent,
    };

    fn me() -> Actor {
        Actor {
            display_name: "me@contoso.com".to_string(),
            email: Some("me@contoso.com".to_string()),
            external_id: Some("aad-object-id-123".to_string()),
        }
    }

    fn base_event() -> GraphEvent {
        GraphEvent {
            id: "AAMk".into(),
            subject: Some("Ok".into()),
            body_preview: None,
            is_cancelled: false,
            is_all_day: false,
            sensitivity: None,
            start: GraphDateTime {
                date_time: "2026-04-23T15:00:00.0000000".into(),
                time_zone: "UTC".into(),
            },
            end: GraphDateTime {
                date_time: "2026-04-23T15:30:00.0000000".into(),
                time_zone: "UTC".into(),
            },
            organizer: None,
            attendees: Vec::new(),
            is_online_meeting: false,
            show_as: None,
        }
    }

    fn attendee(email: &str, response: Option<&str>) -> GraphAttendee {
        GraphAttendee {
            email_address: Some(GraphEmailAddress {
                name: None,
                address: Some(email.to_string()),
            }),
            kind: Some("required".into()),
            status: Some(GraphAttendeeStatus {
                response: response.map(String::from),
            }),
        }
    }

    #[test]
    fn organizer_counts_as_attended() {
        let mut ev = base_event();
        ev.organizer = Some(GraphAttendeeRef {
            email_address: Some(GraphEmailAddress {
                name: None,
                address: Some("me@contoso.com".into()),
            }),
        });
        assert_eq!(user_attendance(&ev, &me()), Attendance::Organizer);
    }

    #[test]
    fn accepted_counts_as_attended() {
        let mut ev = base_event();
        ev.attendees
            .push(attendee("me@contoso.com", Some("accepted")));
        assert_eq!(user_attendance(&ev, &me()), Attendance::AcceptedOrTentative);
    }

    #[test]
    fn tentative_counts_as_attended() {
        let mut ev = base_event();
        ev.attendees
            .push(attendee("me@contoso.com", Some("tentativelyAccepted")));
        assert_eq!(user_attendance(&ev, &me()), Attendance::AcceptedOrTentative);
    }

    #[test]
    fn declined_is_filtered() {
        let mut ev = base_event();
        ev.attendees
            .push(attendee("me@contoso.com", Some("declined")));
        assert_eq!(user_attendance(&ev, &me()), Attendance::Declined);
    }

    #[test]
    fn not_on_attendee_list_is_filtered() {
        let mut ev = base_event();
        ev.attendees
            .push(attendee("someone@contoso.com", Some("accepted")));
        assert_eq!(user_attendance(&ev, &me()), Attendance::NotInvolved);
    }

    #[test]
    fn email_match_is_case_insensitive() {
        let mut ev = base_event();
        ev.organizer = Some(GraphAttendeeRef {
            email_address: Some(GraphEmailAddress {
                name: None,
                address: Some("ME@CONTOSO.COM".into()),
            }),
        });
        assert_eq!(user_attendance(&ev, &me()), Attendance::Organizer);
    }

    #[test]
    fn cancelled_events_are_filtered_by_keeps() {
        let mut ev = base_event();
        ev.is_cancelled = true;
        let mut out = WalkOutcome::default();
        let keep = keeps_event(&ev, &me(), &mut out, uuid::Uuid::nil(), None);
        assert!(!keep);
        assert_eq!(out.filtered_by_status, 1);
    }

    #[test]
    fn all_day_events_are_filtered() {
        let mut ev = base_event();
        ev.is_all_day = true;
        let mut out = WalkOutcome::default();
        assert!(!keeps_event(&ev, &me(), &mut out, uuid::Uuid::nil(), None));
        assert_eq!(out.filtered_by_status, 1);
    }

    #[test]
    fn free_show_as_is_filtered() {
        let mut ev = base_event();
        ev.show_as = Some("free".into());
        let mut out = WalkOutcome::default();
        assert!(!keeps_event(&ev, &me(), &mut out, uuid::Uuid::nil(), None));
        assert_eq!(out.filtered_by_status, 1);
    }

    #[test]
    fn initial_url_carries_all_query_params() {
        let base = Url::parse("https://graph.microsoft.com/v1.0/").unwrap();
        let start = Utc.with_ymd_and_hms(2026, 4, 23, 4, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 4, 24, 4, 0, 0).unwrap();
        let url = build_initial_url(&base, start, end, 50).expect("builds");
        let s = url.to_string();
        assert!(s.contains("me/calendarView"));
        assert!(s.contains("startDateTime="));
        assert!(s.contains("endDateTime="));
        assert!(s.contains("%24top=50"));
        assert!(s.contains("orderby=start%2FdateTime"));
    }

    #[test]
    fn day_bounds_spans_24_hours() {
        let day = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let (start, end) = day_bounds_utc(day, tz);
        assert_eq!((end - start).num_hours(), 24);
    }

    #[test]
    fn self_actor_requires_an_identity_row() {
        let actor = self_actor(&[], uuid::Uuid::nil(), None);
        assert!(actor.is_none());
    }

    #[test]
    fn self_actor_prefers_object_id_but_falls_back_to_upn() {
        let identities = vec![SourceIdentity {
            id: uuid::Uuid::nil(),
            person_id: uuid::Uuid::nil(),
            source_id: Some(uuid::Uuid::nil()),
            kind: SourceIdentityKind::OutlookUserPrincipalName,
            external_actor_id: "me@contoso.com".to_string(),
        }];
        let actor = self_actor(&identities, uuid::Uuid::nil(), None).expect("builds");
        assert_eq!(actor.email.as_deref(), Some("me@contoso.com"));
        assert_eq!(actor.external_id, None);
    }
}
