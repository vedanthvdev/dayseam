//! Activity events — the normalised, source-agnostic record produced by
//! every connector. One row here is one thing the user did or had done to
//! them on a given date.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::source::SourceId;

/// A single piece of evidence from a source — one commit, one merge request
/// state change, one issue comment, etc. Everything the report engine sees
/// is an `ActivityEvent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityEvent {
    /// Deterministic id computed from `(source_id, external_id, kind)` so
    /// re-syncing the same upstream record produces the same primary key.
    pub id: Uuid,
    pub source_id: SourceId,
    /// Stable identifier assigned by the upstream system (MR iid, commit
    /// SHA, issue iid). Used together with `kind` to compute `id`.
    pub external_id: String,
    pub kind: ActivityKind,
    /// Stored UTC on disk; the UI converts to local time at render time.
    pub occurred_at: DateTime<Utc>,
    pub actor: Actor,
    pub title: String,
    pub body: Option<String>,
    pub links: Vec<Link>,
    pub entities: Vec<EntityRef>,
    /// Upstream parent id for rollup (e.g. MR iid for a review comment).
    pub parent_external_id: Option<String>,
    /// Connector-specific attributes that don't warrant a first-class field.
    pub metadata: serde_json::Value,
    pub raw_ref: RawRef,
    pub privacy: Privacy,
}

impl ActivityEvent {
    /// Compute the deterministic id for a given upstream record.
    ///
    /// We derive the id via UUIDv5 using a namespace that itself is derived
    /// from the `source_id`. That guarantees two distinct sources can never
    /// collide even if they happen to use the same `external_id` + `kind`.
    pub fn deterministic_id(source_id: &str, external_id: &str, kind: &str) -> Uuid {
        let ns = Uuid::new_v5(&Uuid::NAMESPACE_OID, source_id.as_bytes());
        Uuid::new_v5(&ns, format!("{kind}::{external_id}").as_bytes())
    }
}

/// The kinds of activity Dayseam currently recognises. Adding a variant is a
/// minor bump; renaming or removing one is a breaking change that must be
/// reflected in the upstream connectors and report templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ActivityKind {
    CommitAuthored,
    MrOpened,
    MrMerged,
    MrClosed,
    MrReviewComment,
    MrApproved,
    IssueOpened,
    IssueClosed,
    IssueComment,
}

/// The person who caused the event. Populated by the connector from
/// whatever identity metadata the upstream provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Actor {
    pub display_name: String,
    pub email: Option<String>,
    /// Upstream id (e.g. GitLab user id as a string). Absent for local git
    /// commits when the only identity we have is an email.
    pub external_id: Option<String>,
}

/// A link that points the user back at the upstream artefact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Link {
    pub url: String,
    pub label: Option<String>,
}

/// A reference to another upstream object (repo, MR, issue, project, ...).
/// The `kind` is free-form because each connector names its own entity
/// taxonomy; the report engine only compares `(kind, external_id)` pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EntityRef {
    pub kind: String,
    pub external_id: String,
    pub label: Option<String>,
}

/// Pointer to the raw upstream payload we kept for replay/debugging. The
/// `storage_key` identifies a row in the `raw_payloads` table or a file
/// under the raw cache directory; the actual bytes never flow through this
/// struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RawRef {
    pub storage_key: String,
    pub content_type: String,
}

/// Privacy classification of an event. `RedactedPrivateRepo` means we
/// recorded that *something* happened but stripped body/title content so
/// the report never leaks private-repo contents by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Privacy {
    Normal,
    RedactedPrivateRepo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_id_is_stable_across_calls() {
        let a = ActivityEvent::deterministic_id("src1", "ext1", "CommitAuthored");
        let b = ActivityEvent::deterministic_id("src1", "ext1", "CommitAuthored");
        assert_eq!(a, b);
    }

    #[test]
    fn deterministic_id_differs_by_kind() {
        let a = ActivityEvent::deterministic_id("src1", "ext1", "CommitAuthored");
        let b = ActivityEvent::deterministic_id("src1", "ext1", "MrOpened");
        assert_ne!(a, b);
    }

    #[test]
    fn deterministic_id_differs_by_source() {
        let a = ActivityEvent::deterministic_id("src-a", "ext1", "CommitAuthored");
        let b = ActivityEvent::deterministic_id("src-b", "ext1", "CommitAuthored");
        assert_ne!(a, b);
    }

    #[test]
    fn deterministic_id_differs_by_external_id() {
        let a = ActivityEvent::deterministic_id("src1", "ext1", "CommitAuthored");
        let b = ActivityEvent::deterministic_id("src1", "ext2", "CommitAuthored");
        assert_ne!(a, b);
    }
}
