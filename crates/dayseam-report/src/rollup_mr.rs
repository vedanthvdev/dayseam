//! Annotate `CommitAuthored` events that are "rolled into" an MR.
//!
//! When a user's work flows commit → push → MR, every commit SHA on
//! the MR's branch appears both as a `CommitAuthored` event (from
//! local-git or from GitLab's push enrichment) *and* as a child of
//! the MR. The verbose template wants to render these as
//! `- <commit title> (rolled into !42)` so the reader can see the MR
//! the commit landed on without clicking into the evidence popover.
//!
//! ## Contract
//!
//! [`annotate_rolled_into_mr`] walks `events` in place. For every
//! `CommitAuthored` whose `external_id` appears in any MR's
//! `commit_shas`, it sets `event.parent_external_id =
//! Some(mr.external_id)`. Events without a matching MR are left
//! untouched. Non-`CommitAuthored` events are never considered
//! (`MrReviewComment`s already carry their parent via the connector).
//!
//! The helper is pure, idempotent, and independent of the set of MRs
//! in the input — an empty `mrs` slice is a no-op.
//!
//! ### Collision handling
//!
//! If the same SHA appears in two different MRs' commit lists (rare,
//! but possible when a commit is cherry-picked from one MR to
//! another), the **first MR encountered** wins. The input order of
//! `mrs` is therefore significant; callers are expected to pass MRs
//! in the order the connector emitted them (which is deterministic,
//! keyed on `(occurred_at, external_id, id)` after sorting).

use dayseam_core::{ActivityEvent, ActivityKind};

/// Pure data describing an MR's commit list, independent of the
/// rest of the `Artifact` machinery.
///
/// Kept local to this module because `ArtifactKind::MergeRequest` is
/// not yet a first-class concept in [`dayseam_core`]; v0.1 only
/// materialises MR *events* (`MrOpened` etc.). Task 3's follow-up
/// enrichment populates this struct from the GitLab API response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequestArtifact {
    /// Stable MR identifier the connector uses as
    /// `ActivityEvent::external_id` on its `MrOpened` / `MrMerged`
    /// events. Typically a `!42` iid for GitLab. Used verbatim as
    /// the annotated `parent_external_id`.
    pub external_id: String,

    /// The commit SHAs that belong to this MR's branch at the time
    /// the MR was observed. Matched against
    /// `ActivityEvent::external_id` (verbatim — no case folding,
    /// no short-SHA expansion).
    pub commit_shas: Vec<String>,
}

/// Tag every `CommitAuthored` in `events` whose SHA is in any MR's
/// `commit_shas` with `parent_external_id = Some(mr.external_id)`.
///
/// * `O(E + total_shas)` time with a single scratch set.
/// * In-place over `&mut [ActivityEvent]` — callers already own the
///   vec (the orchestrator's fan-out path produced it); taking
///   `Vec` by value would force an unnecessary shuffle.
/// * Idempotent: running the helper twice never re-tags or un-tags.
///
/// Events that already have a `parent_external_id` set (e.g. an MR
/// review comment whose connector-emitted parent is the MR iid it
/// was left on) are **never overwritten**. The design reasoning is
/// "parents set by the connector are authoritative; rollup only
/// fills in the blanks". A connector mistakenly emitting
/// `parent_external_id` on a `CommitAuthored` is a connector bug,
/// not a rollup concern.
pub fn annotate_rolled_into_mr(events: &mut [ActivityEvent], mrs: &[MergeRequestArtifact]) {
    if mrs.is_empty() {
        return;
    }

    for event in events.iter_mut() {
        if event.kind != ActivityKind::CommitAuthored {
            continue;
        }
        if event.parent_external_id.is_some() {
            continue;
        }
        // First-MR-wins: iterate in caller order and stop on the
        // first hit. Cloning the small `external_id` string avoids a
        // borrow-after-assign when writing back to the event.
        if let Some(mr) = mrs
            .iter()
            .find(|mr| mr.commit_shas.iter().any(|sha| sha == &event.external_id))
        {
            event.parent_external_id = Some(mr.external_id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{ActivityEvent, Actor, Privacy, RawRef, SourceId};
    use uuid::Uuid;

    fn src() -> SourceId {
        Uuid::from_u128(0x1111)
    }

    fn commit(sha: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::new_v5(&Uuid::NAMESPACE_OID, sha.as_bytes()),
            source_id: src(),
            external_id: sha.into(),
            kind: ActivityKind::CommitAuthored,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Self".into(),
                email: Some("self@example.com".into()),
                external_id: None,
            },
            title: format!("commit {sha}"),
            body: None,
            links: Vec::new(),
            entities: Vec::new(),
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: format!("k:{sha}"),
                content_type: "application/x-git-commit".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn mr(external_id: &str, shas: &[&str]) -> MergeRequestArtifact {
        MergeRequestArtifact {
            external_id: external_id.into(),
            commit_shas: shas.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn commit_in_mr_is_tagged_with_mr_external_id() {
        let mut events = vec![commit("sha1"), commit("sha2")];
        let mrs = vec![mr("!42", &["sha1", "sha2"])];
        annotate_rolled_into_mr(&mut events, &mrs);
        assert_eq!(events[0].parent_external_id.as_deref(), Some("!42"));
        assert_eq!(events[1].parent_external_id.as_deref(), Some("!42"));
    }

    #[test]
    fn commit_not_in_any_mr_stays_none() {
        let mut events = vec![commit("sha1"), commit("lone")];
        let mrs = vec![mr("!42", &["sha1"])];
        annotate_rolled_into_mr(&mut events, &mrs);
        assert_eq!(events[0].parent_external_id.as_deref(), Some("!42"));
        assert_eq!(
            events[1].parent_external_id, None,
            "a commit outside every MR keeps parent_external_id = None"
        );
    }

    #[test]
    fn empty_mrs_is_a_no_op() {
        let mut events = vec![commit("sha1")];
        let before = events.clone();
        annotate_rolled_into_mr(&mut events, &[]);
        assert_eq!(events, before);
    }

    /// Plan invariant 7 — idempotence. Running the helper twice
    /// never changes a field beyond the first call.
    #[test]
    fn mr_rollup_is_idempotent() {
        let mut events = vec![commit("sha1"), commit("sha2"), commit("outside")];
        let mrs = vec![mr("!42", &["sha1", "sha2"])];
        annotate_rolled_into_mr(&mut events, &mrs);
        let after_first = events.clone();
        annotate_rolled_into_mr(&mut events, &mrs);
        assert_eq!(events, after_first);
    }

    /// A SHA that appears in two MRs lands on the first MR passed in.
    #[test]
    fn sha_in_two_mrs_picks_first_mr() {
        let mut events = vec![commit("sha1")];
        let mrs = vec![mr("!10", &["sha1"]), mr("!20", &["sha1"])];
        annotate_rolled_into_mr(&mut events, &mrs);
        assert_eq!(events[0].parent_external_id.as_deref(), Some("!10"));
    }

    /// Non-`CommitAuthored` events are never touched, even if their
    /// external_id happens to match an MR's commit list.
    #[test]
    fn non_commit_authored_events_are_ignored() {
        let mut e = commit("sha1");
        e.kind = ActivityKind::MrOpened;
        let mut events = vec![e];
        annotate_rolled_into_mr(&mut events, &[mr("!42", &["sha1"])]);
        assert_eq!(events[0].parent_external_id, None);
    }

    /// A pre-set `parent_external_id` (e.g. from a review-comment
    /// normaliser) is authoritative and never overwritten.
    #[test]
    fn preset_parent_is_never_overwritten() {
        let mut e = commit("sha1");
        e.parent_external_id = Some("!origin".into());
        let mut events = vec![e];
        annotate_rolled_into_mr(&mut events, &[mr("!override", &["sha1"])]);
        assert_eq!(events[0].parent_external_id.as_deref(), Some("!origin"));
    }
}
