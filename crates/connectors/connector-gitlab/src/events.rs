//! Serde-typed wrappers around the GitLab Events API response shape.
//!
//! <https://docs.gitlab.com/ee/api/events.html>
//!
//! The Events endpoint (`GET /api/v4/users/:user_id/events`) returns a
//! JSON array where every element has the shape:
//!
//! ```json
//! {
//!   "id": 123456789,
//!   "project_id": 42,
//!   "action_name": "pushed to",
//!   "target_id": null,
//!   "target_type": null,
//!   "target_title": null,
//!   "target_iid": null,
//!   "created_at": "2026-04-19T23:04:12.000Z",
//!   "author_id": 17,
//!   "author": { "id": 17, "username": "vedanth", ... },
//!   "push_data": { "ref": "refs/heads/…", "commit_count": 3, ... },
//!   "note": { "id": ..., "body": "..." }
//! }
//! ```
//!
//! We model the `action` and `target_type` fields as small exhaustive
//! enums with a [`#[serde(other)]`][serde-other] catch-all; the catch-all
//! is what lets a forward-compatible GitLab release (e.g. adding
//! `target_type: "WikiPage"`) degrade into a typed
//! [`crate::errors::GitlabUpstreamError::ShapeChanged`] rather than a
//! `serde::de::Error` surfacing as a generic
//! [`dayseam_core::DayseamError::Internal`].
//!
//! [serde-other]: https://serde.rs/enum-representations.html#other

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// One row from `GET /api/v4/users/:user_id/events`.
///
/// We deliberately decode only the fields we need downstream; any
/// additional keys GitLab adds in a future release are silently
/// ignored by serde's default behaviour, which is exactly what lets
/// the connector survive a minor upstream version bump.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitlabEvent {
    /// GitLab's monotonically-increasing event id. We surface it as the
    /// `external_id` on the resulting
    /// [`dayseam_core::ActivityEvent`] so re-syncing the same day is
    /// idempotent against the `INSERT OR IGNORE` path DAY-52 added.
    pub id: i64,

    /// Canonical action string from the API (`"pushed to"`, `"opened"`,
    /// `"closed"`, …). Decoded into [`GitlabAction`] via a custom
    /// `String` → enum mapping because GitLab's own strings contain
    /// spaces, which precludes the usual `rename_all` approach.
    #[serde(rename = "action_name")]
    pub action_name: String,

    /// The kind of thing the action acted on. `target_type` is `null`
    /// for push events (the target *is* the project, implicitly) so we
    /// unwrap via [`Option`] with a [`GitlabTargetType::Unknown`]
    /// fallback.
    pub target_type: Option<GitlabTargetType>,

    /// Upstream-assigned integer within the project (MR iid, issue
    /// iid). Absent for push events and comment events on non-iid
    /// targets.
    pub target_iid: Option<i64>,

    /// Upstream-assigned integer across the instance (note id, MR
    /// global id, issue global id). Absent for push events.
    pub target_id: Option<i64>,

    /// Human-readable title GitLab echoed back; we pass it straight
    /// through to the `ActivityEvent::title` in most cases.
    pub target_title: Option<String>,

    /// Project id the event belongs to. Used to compose deep-links and
    /// to disambiguate MR iids (iids are per-project, not globally
    /// unique).
    pub project_id: Option<i64>,

    /// ISO-8601 timestamp GitLab stamped on the event. Comes back UTC;
    /// we store it verbatim in the `ActivityEvent::occurred_at` field
    /// and let the report engine bucket into the user's local day.
    pub created_at: DateTime<Utc>,

    /// Stable numeric user id of whoever caused the event. The v0.1
    /// identity filter matches on this field, never on `username` /
    /// `email`, so a user whose handle rotates stays correctly
    /// attributed.
    pub author_id: i64,

    /// Denormalised author object. Present on every row in practice;
    /// optional here so a missing object degrades to "unknown user"
    /// rather than a `ShapeChanged` error.
    pub author: Option<GitlabAuthor>,

    /// Comment payload when the event is a discussion/note. Absent
    /// otherwise. We pull the `body` into `ActivityEvent::body`.
    pub note: Option<GitlabNote>,

    /// Push-event payload. Present when `action_name == "pushed to"` /
    /// `"pushed new"`; absent otherwise. Carries the commit count and
    /// (optionally, on self-hosted instances with commit_from/to set)
    /// the ref name we surface in the bullet.
    pub push_data: Option<GitlabPushData>,
}

/// Decoded [`GitlabEvent::action_name`]. Canonical names and spaces
/// preserved (we parse them explicitly rather than using serde's
/// `rename_all`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitlabAction {
    /// `"pushed to"` / `"pushed new"` — each push produces a
    /// [`GitlabPushData`] payload.
    Pushed,
    /// `"opened"` — MR or issue opened.
    Opened,
    /// `"closed"` — MR or issue closed without merge.
    Closed,
    /// `"merged"` — MR merged.
    Merged,
    /// `"approved"` — MR approval.
    Approved,
    /// `"commented on"` — note added to MR, issue, or commit.
    Commented,
    /// Any other action string GitLab ships in the future. Producing
    /// this variant is a signal for the walker to skip the event
    /// rather than crash; it is *not* a shape-changed error because
    /// GitLab adds action names regularly (e.g. "approved", added
    /// after the original plan) and "ignore what we don't understand"
    /// is the safer default.
    Other,
}

impl GitlabAction {
    /// Parse the canonical GitLab string. Unknown strings map to
    /// [`GitlabAction::Other`], which the walker skips.
    pub fn parse(s: &str) -> Self {
        match s {
            "pushed to" | "pushed new" | "pushed" => Self::Pushed,
            "opened" | "created" => Self::Opened,
            "closed" => Self::Closed,
            "merged" | "accepted" => Self::Merged,
            "approved" => Self::Approved,
            "commented on" | "commented" => Self::Commented,
            _ => Self::Other,
        }
    }
}

/// Decoded [`GitlabEvent::target_type`]. The `#[serde(other)]` arm is
/// the schema-drift guard: a future `"WikiPage"` / `"Epic"` target
/// decodes as [`GitlabTargetType::Unknown`] and the walker either
/// ignores it or reports a typed shape-changed error — never a
/// generic serde panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum GitlabTargetType {
    MergeRequest,
    Issue,
    Note,
    DiffNote,
    DiscussionNote,
    #[serde(other)]
    Unknown,
}

/// Author object embedded in an event row.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitlabAuthor {
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Canonical profile URL. Surfaced as the `Actor` profile link on
    /// the rendered bullet.
    #[serde(default)]
    pub web_url: Option<String>,
}

/// Comment payload — a subset of the `notes` API response.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitlabNote {
    #[serde(default)]
    pub body: Option<String>,
}

/// Push-event payload.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitlabPushData {
    /// Ref pushed to — `"refs/heads/main"`, `"refs/tags/v1"`, or the
    /// short form. We surface whatever GitLab sent us in the bullet.
    #[serde(rename = "ref", default)]
    pub git_ref: Option<String>,

    /// Number of commits pushed. Load-bearing: the walker uses this to
    /// cap enrichment at 50 (plan invariant 4 — a 200-commit push does
    /// not produce 200 bullets).
    #[serde(default)]
    pub commit_count: Option<i64>,

    /// Most recent commit SHA in the push. Some GitLab configurations
    /// only surface this pair; others surface the full SHA range via
    /// the commits endpoint. Optional so a minimal payload still
    /// decodes.
    #[serde(default)]
    pub commit_to: Option<String>,

    /// Previous tip before the push. Used with [`Self::commit_to`] to
    /// derive the short-SHA label for the bullet.
    #[serde(default)]
    pub commit_from: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_push_event_fixture() {
        let json = r#"{
            "id": 1,
            "action_name": "pushed to",
            "target_type": null,
            "target_iid": null,
            "target_id": null,
            "target_title": null,
            "project_id": 42,
            "created_at": "2026-04-19T23:04:12.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth", "web_url": "https://gitlab.example/vedanth" },
            "push_data": { "ref": "refs/heads/main", "commit_count": 3, "commit_to": "abc123" }
        }"#;
        let e: GitlabEvent = serde_json::from_str(json).unwrap();
        assert_eq!(e.id, 1);
        assert_eq!(GitlabAction::parse(&e.action_name), GitlabAction::Pushed);
        assert!(e.target_type.is_none());
        assert_eq!(e.push_data.as_ref().unwrap().commit_count, Some(3));
    }

    #[test]
    fn parses_mr_opened_event_fixture() {
        let json = r#"{
            "id": 2,
            "action_name": "opened",
            "target_type": "MergeRequest",
            "target_iid": 11,
            "target_id": 2001,
            "target_title": "Add payments slice",
            "project_id": 42,
            "created_at": "2026-04-19T12:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        }"#;
        let e: GitlabEvent = serde_json::from_str(json).unwrap();
        assert_eq!(
            e.target_type,
            Some(GitlabTargetType::MergeRequest),
            "MR target type"
        );
        assert_eq!(GitlabAction::parse(&e.action_name), GitlabAction::Opened);
        assert_eq!(e.target_iid, Some(11));
    }

    #[test]
    fn parses_note_event_fixture() {
        let json = r#"{
            "id": 3,
            "action_name": "commented on",
            "target_type": "Note",
            "target_iid": 11,
            "target_id": 555,
            "target_title": "Add payments slice",
            "project_id": 42,
            "created_at": "2026-04-19T13:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" },
            "note": { "body": "LGTM" }
        }"#;
        let e: GitlabEvent = serde_json::from_str(json).unwrap();
        assert_eq!(e.note.as_ref().unwrap().body.as_deref(), Some("LGTM"));
        assert_eq!(GitlabAction::parse(&e.action_name), GitlabAction::Commented);
    }

    /// Schema-drift invariant (plan Task 1 invariant 8). An unknown
    /// `target_type` must decode to `Unknown` — **not** a serde error.
    /// The walker is expected to then emit a `ShapeChanged` for the
    /// event in question while still processing neighbouring rows.
    #[test]
    fn unknown_target_type_decodes_to_unknown_not_error() {
        let json = r#"{
            "id": 4,
            "action_name": "edited",
            "target_type": "WikiPage",
            "target_iid": null,
            "target_id": 9001,
            "target_title": "Onboarding",
            "project_id": 42,
            "created_at": "2026-04-19T14:00:00.000Z",
            "author_id": 17,
            "author": { "id": 17, "username": "vedanth" }
        }"#;
        let e: GitlabEvent = serde_json::from_str(json).expect("serde should not fail");
        assert_eq!(e.target_type, Some(GitlabTargetType::Unknown));
        assert_eq!(GitlabAction::parse(&e.action_name), GitlabAction::Other);
    }

    #[test]
    fn missing_author_object_is_tolerated() {
        let json = r#"{
            "id": 5,
            "action_name": "opened",
            "target_type": "Issue",
            "target_iid": 7,
            "target_id": 3001,
            "target_title": "Bug report",
            "project_id": 42,
            "created_at": "2026-04-19T15:00:00.000Z",
            "author_id": 17
        }"#;
        let e: GitlabEvent = serde_json::from_str(json).expect("missing author is OK");
        assert!(e.author.is_none());
    }

    #[test]
    fn action_parse_covers_synonyms() {
        assert_eq!(GitlabAction::parse("pushed new"), GitlabAction::Pushed);
        assert_eq!(GitlabAction::parse("accepted"), GitlabAction::Merged);
        assert_eq!(GitlabAction::parse("created"), GitlabAction::Opened);
        assert_eq!(GitlabAction::parse("commented"), GitlabAction::Commented);
        assert_eq!(GitlabAction::parse("unknown thing"), GitlabAction::Other);
    }
}
