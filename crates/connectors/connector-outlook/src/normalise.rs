//! `GraphEvent` → [`dayseam_core::ActivityEvent`] mapping.
//!
//! The Outlook connector produces a single [`ActivityKind`] variant —
//! [`ActivityKind::OutlookMeetingAttended`] — so there's no router,
//! just one normaliser plus helpers for the pieces of the
//! [`ActivityEvent`] we compute non-trivially:
//!
//! * **`title`** always carries the meeting's subject, modulo the
//!   private-sensitivity redaction that keeps
//!   `sensitivity == "private"` rows from leaking their subject to
//!   the report surface (the user opted them private on their
//!   calendar; Dayseam respects the same contract the Outlook UI
//!   does). A missing subject falls back to `"(No subject)"` so the
//!   row still renders.
//! * **`body`** carries the Graph `bodyPreview` snippet (nullable),
//!   which we pass through to let the DAY-204 Jira-key enricher
//!   scan it. Never interpreted here.
//! * **`links`** includes the `https://outlook.office.com/calendar/`
//!   deep-link so a click on the evidence row opens the event in the
//!   web UI. Constructed from the event id.
//! * **`actor`** is the user whose calendar this walker is running
//!   against — **not** the organiser — because "meeting attended" is
//!   an event about the calendar owner. The walker passes the
//!   resolved identity in via the `self_ident` argument.
//! * **`metadata`** captures raw-form organiser, `isOnlineMeeting`,
//!   and UPN so a later report pass can decide how to render the
//!   copy ("You hosted a meeting with …" vs "You attended …")
//!   without re-hydrating the Graph row.
//!
//! Self-filtering (did the calendar owner actually attend?) lives in
//! [`crate::walk`], not here — this module trusts its caller, the
//! same contract the GitHub normaliser uses.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityKind, EntityRef, Link, Privacy, RawRef, SourceId,
};

use crate::events::{GraphDateTime, GraphEvent};

/// A normaliser error — currently only raised when Graph hands us a
/// timestamp we cannot parse. Bubbles up to the walker which drops
/// the offending row and increments the `dropped_by_shape` counter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormaliseError {
    /// Graph's `start` / `end` value didn't match the documented
    /// `YYYY-MM-DDTHH:MM:SS.fffffff` naive form. Signals either a
    /// Graph contract change or a regression in the client's
    /// `Prefer: outlook.timezone="UTC"` header.
    UnparseableTimestamp { raw: String },
}

/// Short token used to seed [`ActivityEvent::deterministic_id`]. Kept
/// in a helper so a future renamer of the kind can't silently break
/// the id-stability contract — deterministic ids encode this string
/// into the UUIDv5, so changing it reshuffles every historic row.
pub(crate) fn kind_token(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::OutlookMeetingAttended => "OutlookMeetingAttended",
        // Every other ActivityKind is produced by a different
        // connector's normaliser; reaching this arm is a programmer
        // bug, not user data. The panic matches the
        // connector-github / connector-gitlab idiom and is caught at
        // CI by the exhaustive-match lint on the host enum.
        ActivityKind::CommitAuthored
        | ActivityKind::MrOpened
        | ActivityKind::MrMerged
        | ActivityKind::MrClosed
        | ActivityKind::MrReviewComment
        | ActivityKind::MrApproved
        | ActivityKind::IssueOpened
        | ActivityKind::IssueClosed
        | ActivityKind::IssueComment
        | ActivityKind::JiraIssueTransitioned
        | ActivityKind::JiraIssueCommented
        | ActivityKind::JiraIssueAssigned
        | ActivityKind::JiraIssueUnassigned
        | ActivityKind::JiraIssueCreated
        | ActivityKind::ConfluencePageCreated
        | ActivityKind::ConfluencePageEdited
        | ActivityKind::ConfluenceComment
        | ActivityKind::GitHubPullRequestOpened
        | ActivityKind::GitHubPullRequestMerged
        | ActivityKind::GitHubPullRequestClosed
        | ActivityKind::GitHubPullRequestReviewed
        | ActivityKind::GitHubPullRequestCommented
        | ActivityKind::GitHubIssueOpened
        | ActivityKind::GitHubIssueClosed
        | ActivityKind::GitHubIssueCommented
        | ActivityKind::GitHubIssueAssigned => unreachable!(
            "Outlook normaliser saw non-Outlook ActivityKind {kind:?}: kind production is local \
             to this module",
        ),
    }
}

/// Entry point: normalise one Graph event to an `ActivityEvent`.
///
/// `self_actor` is the calendar owner — see module docs for why it
/// drives `actor`, not the organiser.
pub fn normalise_event(
    source_id: SourceId,
    self_actor: &Actor,
    event: &GraphEvent,
) -> Result<ActivityEvent, NormaliseError> {
    let occurred_at = parse_graph_timestamp(&event.start)?;

    let kind = ActivityKind::OutlookMeetingAttended;
    let external_id = event.id.clone();
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token(kind));

    // Graph's `sensitivity` field has four documented values:
    // `normal`, `personal`, `private`, `confidential`. Dayseam redacts
    // `private` + `confidential` (the two the Outlook UI itself
    // redacts for shared-calendar viewers); `personal` is passed
    // through because Outlook shows the subject to shared-calendar
    // viewers for that level.
    //
    // The redaction lives here at the title/body-composition layer
    // — we rewrite `title` to `"Private meeting"` and blank `body` —
    // so the `Privacy` flag stays at `Normal` (the enum is a
    // connector-local-git vocabulary today, tracked by DAY-2xx for
    // generalisation). Downstream renderers see the already-redacted
    // fields and never need to re-check sensitivity.
    let is_private = event.sensitivity.as_deref().is_some_and(|s| {
        s.eq_ignore_ascii_case("private") || s.eq_ignore_ascii_case("confidential")
    });
    let privacy = Privacy::Normal;

    let title = compose_title(event, is_private);
    let body = if is_private {
        None
    } else {
        event.body_preview.clone().filter(|s| !s.is_empty())
    };

    let links = vec![Link {
        url: deep_link(&event.id),
        label: Some("Open in Outlook".to_string()),
    }];

    let entities = vec![EntityRef {
        kind: EntityKind::OutlookEvent,
        external_id: external_id.clone(),
        label: Some(
            event
                .subject
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(No subject)".to_string()),
        ),
    }];

    let metadata = serde_json::json!({
        "outlook_event_id": event.id,
        "is_online_meeting": event.is_online_meeting,
        "is_organizer": organizer_matches_self(event, self_actor),
        "organizer_email": event
            .organizer
            .as_ref()
            .and_then(|o| o.email_address.as_ref())
            .and_then(|e| e.address.as_deref()),
        "start_utc": event.start.date_time,
        "end_utc": event.end.date_time,
    });

    Ok(ActivityEvent {
        id,
        source_id,
        external_id: external_id.clone(),
        kind,
        occurred_at,
        actor: self_actor.clone(),
        title,
        body,
        links,
        entities,
        parent_external_id: Some(external_id),
        metadata,
        raw_ref: RawRef {
            storage_key: format!("outlook:event:{}", event.id),
            content_type: "application/json".to_string(),
        },
        privacy,
    })
}

/// Compose the evidence-row title. Private-flagged events get a
/// generic label; otherwise the subject (or `"(No subject)"`) wins.
fn compose_title(event: &GraphEvent, is_private: bool) -> String {
    if is_private {
        return "Private meeting".to_string();
    }
    match event.subject.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "(No subject)".to_string(),
    }
}

/// Build the `outlook.office.com` deep-link URL for one event. Uses
/// the Graph event id directly — the web UI accepts it as the
/// `itemId` query parameter.
fn deep_link(event_id: &str) -> String {
    format!(
        "https://outlook.office.com/calendar/item/{}",
        urlencoding_encode(event_id)
    )
}

/// Tiny URL-encoder — Graph event ids are base64-ish and contain
/// `=` and `/`, both of which need percent-encoding for a safe URL.
/// We avoid pulling in `percent-encoding` as a dep for a two-character
/// alphabet replacement.
fn urlencoding_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '=' => "%3D".to_string(),
            '/' => "%2F".to_string(),
            '+' => "%2B".to_string(),
            ' ' => "%20".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Parse Graph's naive-UTC timestamp form
/// (`"2026-04-23T15:00:00.0000000"`) into a `DateTime<Utc>`. Relies on
/// the walker sending `Prefer: outlook.timezone="UTC"` so the string
/// form's implicit zone is UTC; [`crate::walk`] enforces that.
fn parse_graph_timestamp(gdt: &GraphDateTime) -> Result<DateTime<Utc>, NormaliseError> {
    // Graph emits seven fractional-second digits; chrono's default
    // parser accepts any number of digits after the decimal when the
    // format string uses `%.f`.
    let naive = NaiveDateTime::parse_from_str(&gdt.date_time, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(&gdt.date_time, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|_| NormaliseError::UnparseableTimestamp {
            raw: gdt.date_time.clone(),
        })?;
    Ok(Utc.from_utc_datetime(&naive))
}

/// Whether the calendar owner was also the organiser of the meeting.
/// Matched by email-case-insensitively; a missing organiser / missing
/// email falls through as `false`.
fn organizer_matches_self(event: &GraphEvent, self_actor: &Actor) -> bool {
    let Some(self_email) = self_actor.email.as_deref() else {
        return false;
    };
    event
        .organizer
        .as_ref()
        .and_then(|o| o.email_address.as_ref())
        .and_then(|e| e.address.as_deref())
        .is_some_and(|addr| addr.eq_ignore_ascii_case(self_email))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{GraphAttendeeRef, GraphDateTime, GraphEmailAddress, GraphEvent};
    use uuid::Uuid;

    fn source() -> SourceId {
        Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()
    }

    fn me() -> Actor {
        Actor {
            display_name: "Vedanth".to_string(),
            email: Some("vedanth@contoso.com".to_string()),
            external_id: Some("graph-object-id-777".to_string()),
        }
    }

    fn graph_event(subject: Option<&str>) -> GraphEvent {
        GraphEvent {
            id: "AAMkAGI0X".to_string(),
            subject: subject.map(String::from),
            body_preview: None,
            is_cancelled: false,
            is_all_day: false,
            sensitivity: None,
            start: GraphDateTime {
                date_time: "2026-04-23T15:00:00.0000000".to_string(),
                time_zone: "UTC".to_string(),
            },
            end: GraphDateTime {
                date_time: "2026-04-23T15:30:00.0000000".to_string(),
                time_zone: "UTC".to_string(),
            },
            organizer: None,
            attendees: Vec::new(),
            is_online_meeting: false,
            show_as: None,
        }
    }

    #[test]
    fn normalises_standard_meeting() {
        let event = graph_event(Some("Standup"));
        let normalised = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(normalised.kind, ActivityKind::OutlookMeetingAttended);
        assert_eq!(normalised.title, "Standup");
        assert_eq!(
            normalised.actor.email.as_deref(),
            Some("vedanth@contoso.com")
        );
        assert_eq!(normalised.external_id, "AAMkAGI0X");
        assert_eq!(normalised.privacy, Privacy::Normal);
        assert_eq!(normalised.links.len(), 1);
        assert!(normalised.links[0]
            .url
            .starts_with("https://outlook.office.com/"));
    }

    #[test]
    fn redacts_private_events() {
        let mut event = graph_event(Some("1:1 about promotion"));
        event.sensitivity = Some("private".to_string());
        event.body_preview = Some("sensitive notes".to_string());
        let normalised = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(normalised.title, "Private meeting");
        assert_eq!(normalised.body, None);
    }

    #[test]
    fn redacts_confidential_events() {
        let mut event = graph_event(Some("Board meeting"));
        event.sensitivity = Some("confidential".to_string());
        let normalised = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(normalised.title, "Private meeting");
    }

    #[test]
    fn missing_subject_falls_back() {
        let event = graph_event(None);
        let normalised = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(normalised.title, "(No subject)");
    }

    #[test]
    fn empty_subject_falls_back() {
        let event = graph_event(Some(""));
        let normalised = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(normalised.title, "(No subject)");
    }

    #[test]
    fn organizer_is_self_when_matching_email() {
        let mut event = graph_event(Some("Strategy"));
        event.organizer = Some(GraphAttendeeRef {
            email_address: Some(GraphEmailAddress {
                name: Some("Vedanth".to_string()),
                address: Some("VEDANTH@contoso.com".to_string()),
            }),
        });
        let normalised = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(
            normalised.metadata.get("is_organizer"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn deterministic_id_is_stable() {
        let event = graph_event(Some("Standup"));
        let a = normalise_event(source(), &me(), &event).expect("parses");
        let b = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(a.id, b.id);
    }

    #[test]
    fn reports_unparseable_timestamp() {
        let mut event = graph_event(Some("Ok"));
        event.start.date_time = "not-a-timestamp".to_string();
        let err = normalise_event(source(), &me(), &event).expect_err("should fail");
        assert!(matches!(err, NormaliseError::UnparseableTimestamp { .. }));
    }

    #[test]
    fn timestamp_without_fractional_seconds_parses() {
        let mut event = graph_event(Some("Ok"));
        event.start.date_time = "2026-04-23T15:00:00".to_string();
        let normalised = normalise_event(source(), &me(), &event).expect("parses");
        assert_eq!(
            normalised.occurred_at.to_rfc3339(),
            "2026-04-23T15:00:00+00:00"
        );
    }
}
