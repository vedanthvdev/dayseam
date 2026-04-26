//! Wire shapes for Microsoft Graph calendar event responses.
//!
//! We deserialise **only the fields the walker / normaliser need** —
//! Graph's full event object ships ~40 fields including attachments,
//! extended properties, and online-meeting metadata. Unused fields
//! land in `#[serde(default)]` skips via `serde`'s default behaviour
//! of silently dropping unknown keys.
//!
//! Timezones: Graph returns `start` / `end` as
//! `{ "dateTime": "2026-04-23T14:00:00.0000000", "timeZone": "UTC" }`.
//! When the client sends `Prefer: outlook.timezone="UTC"` (the default
//! in [`crate::walk`]) the `timeZone` field is always `"UTC"`, so the
//! `dateTime` naive form can be parsed as UTC without consulting
//! `timeZone`. The struct keeps `timeZone` as `String` so a future
//! regression that forgets to send the `Prefer` header is caught by
//! the normaliser's assertion rather than silently mis-timing events.

use serde::Deserialize;

/// One row from `GET /me/calendarView`. See module docs for the
/// field-trimming rationale.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphEvent {
    /// Opaque event id — the string Graph uses on `GET /me/events/{id}`.
    /// Stable for a given occurrence unless the organiser deletes and
    /// recreates the series.
    pub id: String,
    /// Meeting subject / title. May be absent (Graph documents it as
    /// nullable for events created without a subject). Falls back to
    /// `"(No subject)"` at render time.
    #[serde(default)]
    pub subject: Option<String>,
    /// Body preview — first ~256 characters of the event body as
    /// plain text. Used only when a Jira key is pulled out by the
    /// DAY-204 enricher; the walker itself never logs it.
    #[serde(rename = "bodyPreview", default)]
    pub body_preview: Option<String>,
    /// Whether the event was marked cancelled. Cancelled events stay
    /// on the calendar with a strike-through; Dayseam filters them
    /// out so a cancelled 3 PM standup doesn't render as "attended".
    #[serde(rename = "isCancelled", default)]
    pub is_cancelled: bool,
    /// Whether the event is all-day. All-day events (company holidays,
    /// birthdays, OOO markers) are filtered out — they don't represent
    /// actual meeting time.
    #[serde(rename = "isAllDay", default)]
    pub is_all_day: bool,
    /// `"private"`, `"confidential"`, `"personal"`, or `"normal"`.
    /// Graph returns `"normal"` when unset. Private-flagged events
    /// have their subject redacted at render time.
    #[serde(default)]
    pub sensitivity: Option<String>,
    /// Start timestamp. See module docs — parsed as UTC naive when
    /// the client sends `Prefer: outlook.timezone="UTC"`.
    pub start: GraphDateTime,
    /// End timestamp. Same shape as `start`.
    pub end: GraphDateTime,
    /// Event organizer. The walker uses this to decide whether the
    /// user was the organiser vs an invitee for the `## Meetings`
    /// section's copy.
    #[serde(default)]
    pub organizer: Option<GraphAttendeeRef>,
    /// Invited attendees. Each entry carries response status
    /// (`accepted` / `declined` / `tentativelyAccepted` / `notResponded`)
    /// — the walker filters on `accepted` / `tentativelyAccepted`.
    #[serde(default)]
    pub attendees: Vec<GraphAttendee>,
    /// Whether the event was scheduled as a Teams online meeting.
    /// Surfaces the v0.9 DAY-204 attendance-verification preference
    /// — only Teams meetings have a verifiable attendance record.
    #[serde(rename = "isOnlineMeeting", default)]
    pub is_online_meeting: bool,
    /// Show-as state (`busy`, `free`, `tentative`, `workingElsewhere`,
    /// `oof`, `unknown`). `free` blocks are ignored by the walker —
    /// they don't represent work.
    #[serde(rename = "showAs", default)]
    pub show_as: Option<String>,
}

/// Graph's timestamp envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphDateTime {
    /// ISO-8601 naive datetime (seven-digit fractional seconds). UTC
    /// when the client requested `Prefer: outlook.timezone="UTC"`.
    #[serde(rename = "dateTime")]
    pub date_time: String,
    /// Timezone string. Always `"UTC"` with the Prefer header.
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

/// Organizer reference — a single attendee shape.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphAttendeeRef {
    #[serde(rename = "emailAddress", default)]
    pub email_address: Option<GraphEmailAddress>,
}

/// Email + display-name pair attached to both organizer and attendee
/// entries.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphEmailAddress {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
}

/// Invited attendee with response status.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphAttendee {
    #[serde(rename = "emailAddress", default)]
    pub email_address: Option<GraphEmailAddress>,
    /// `"required"`, `"optional"`, or `"resource"`. The walker's
    /// attendance filter doesn't care which — all three are real
    /// attendees from the user's POV.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// Response payload.
    #[serde(default)]
    pub status: Option<GraphAttendeeStatus>,
}

/// The attendee's response (accept / decline / tentative).
#[derive(Debug, Clone, Deserialize)]
pub struct GraphAttendeeStatus {
    /// `"none"`, `"organizer"`, `"tentativelyAccepted"`, `"accepted"`,
    /// `"declined"`, `"notResponded"`.
    #[serde(default)]
    pub response: Option<String>,
}

/// Paged response envelope Graph wraps `calendarView` results in.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphEventsPage {
    pub value: Vec<GraphEvent>,
    /// Next-page cursor. Graph hands this as a fully-formed URL; the
    /// walker uses it verbatim until the server stops returning one.
    #[serde(rename = "@odata.nextLink", default)]
    pub next_link: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialises_minimum_event_shape() {
        let json = r#"{
            "id": "AAMkAGI=",
            "subject": "Standup",
            "start": { "dateTime": "2026-04-23T15:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-04-23T15:30:00.0000000", "timeZone": "UTC" }
        }"#;
        let ev: GraphEvent = serde_json::from_str(json).expect("parses");
        assert_eq!(ev.id, "AAMkAGI=");
        assert_eq!(ev.subject.as_deref(), Some("Standup"));
        assert!(!ev.is_cancelled);
        assert!(!ev.is_all_day);
        assert_eq!(ev.start.time_zone, "UTC");
    }

    #[test]
    fn deserialises_page_with_next_link() {
        let json = r#"{
            "value": [],
            "@odata.nextLink": "https://graph.microsoft.com/v1.0/me/calendarView?$skip=10"
        }"#;
        let page: GraphEventsPage = serde_json::from_str(json).expect("parses");
        assert_eq!(page.value.len(), 0);
        assert_eq!(
            page.next_link.as_deref(),
            Some("https://graph.microsoft.com/v1.0/me/calendarView?$skip=10")
        );
    }

    #[test]
    fn attendees_and_organizer_parse() {
        let json = r#"{
            "id": "AAMk",
            "start": { "dateTime": "2026-04-23T15:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-04-23T15:30:00.0000000", "timeZone": "UTC" },
            "organizer": {
                "emailAddress": { "name": "Alice", "address": "alice@contoso.com" }
            },
            "attendees": [
                {
                    "emailAddress": { "name": "Me", "address": "me@contoso.com" },
                    "type": "required",
                    "status": { "response": "accepted" }
                },
                {
                    "emailAddress": { "address": "bob@contoso.com" },
                    "status": { "response": "declined" }
                }
            ]
        }"#;
        let ev: GraphEvent = serde_json::from_str(json).expect("parses");
        assert_eq!(ev.attendees.len(), 2);
        let first = &ev.attendees[0];
        assert_eq!(first.kind.as_deref(), Some("required"));
        assert_eq!(
            first.status.as_ref().and_then(|s| s.response.as_deref()),
            Some("accepted")
        );
        let org = ev.organizer.as_ref().expect("organizer present");
        assert_eq!(
            org.email_address
                .as_ref()
                .and_then(|e| e.address.as_deref()),
            Some("alice@contoso.com")
        );
    }
}
