//! Per-kind report sections.
//!
//! Every [`ArtifactPayload`] variant maps to exactly one
//! [`ReportSection`], and every non-empty section renders as its own
//! `## Heading` in the final markdown. This is the v0.3 replacement
//! for the v0.1/v0.2 shape where `render.rs` threw every bullet —
//! commit, Jira issue, Confluence page — into a single section
//! titled `Commits`, producing reports that labelled a Jira
//! transition as a "Commit". The dogfood issue that prompted this
//! split is tracked in issue #86.
//!
//! ## Why the enum is the single source of truth
//!
//! The mapping lives in [`ReportSection::from_payload`] as an
//! exhaustive `match` on [`ArtifactPayload`]. Adding a new payload
//! variant therefore fails to compile here first — the author has to
//! pick a section for it before any other layer of the engine will
//! accept the change. That compile-time nudge is deliberate: silent
//! fall-through to `Commits` is what caused the dogfood regression
//! in the first place.
//!
//! ## Ordering
//!
//! Sections render in the enum's declaration order, via the derived
//! `Ord` — *not* artifact-arrival order, so a day with commits
//! appearing after Jira activity in the rollup output still puts
//! `## Commits` before `## Jira issues`. The order is fixed (not
//! alphabetical) because the intended reading order is
//! "what I shipped → what I triaged → what I wrote" and that is
//! not the alphabetical order of the section titles. The
//! `ord_matches_render_order` test below is the lock keeping the
//! declaration order honest.
//!
//! Empty sections are omitted from the draft entirely (see
//! `render::build_sections`). The empty-*day* fallback — when the
//! entire report has zero events — still renders a single
//! `## Commits` section with a "No tracked activity" bullet, to
//! preserve the existing UI contract the desktop streaming preview
//! depends on.

use dayseam_core::{ActivityKind, ArtifactPayload};
use serde_json::json;

use crate::rollup::RolledUpArtifact;

/// A top-level section of the rendered report.
///
/// The enum is `pub(crate)` because the outside world sees
/// [`dayseam_core::RenderedSection`] — a plain `(id, title,
/// bullets)` triple — not this routing-time discriminant. Keeping it
/// internal leaves us free to add a fourth variant (e.g. a future
/// `MergeRequests` section once `ArtifactPayload::MergeRequest`
/// exists) without churning the IPC surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ReportSection {
    /// Commits authored today — currently fed by
    /// [`ArtifactPayload::CommitSet`] (local-git + GitLab). Future
    /// MR artifacts will get their own section; until they exist,
    /// commits that rolled into an MR still render here with the
    /// verbose-mode `(rolled into !42)` suffix.
    Commits,
    /// Jira issue activity — transitions, comments, assignments —
    /// fed by [`ArtifactPayload::JiraIssue`]. One bullet per event
    /// (not per issue) so a day that transitioned the same issue
    /// twice renders both transitions.
    JiraIssues,
    /// Confluence page activity — page created, edited, commented
    /// on — fed by [`ArtifactPayload::ConfluencePage`]. One bullet
    /// per event.
    ConfluencePages,
    /// Catch-all for events the primary-kind sections can't host
    /// honestly. DAY-88 / CORR-v0.2-06 adds this for Confluence
    /// comments whose CQL result carried neither `ancestors[]` nor
    /// `container`, so the normaliser couldn't resolve a parent
    /// page. Before, those comments rolled up under a synthetic
    /// `page_id = "UNKNOWN"` and rendered in `## Confluence pages`
    /// as if they belonged to a real page; routing them here
    /// instead keeps the Confluence section truthful (every bullet
    /// has a real parent page) and still surfaces the work to the
    /// user. Ordered *last* deliberately — the reading order is
    /// "what I shipped → what I triaged → what I wrote → stray
    /// activity I should triage later".
    Other,
}

impl ReportSection {
    /// Route an artifact payload to its section.
    ///
    /// Implemented as an exhaustive `match` so a new
    /// [`ArtifactPayload`] variant fails to compile until its
    /// routing is decided here. Do *not* add a wildcard arm —
    /// silent fall-through is the bug this module exists to
    /// prevent.
    ///
    /// This is the payload-only path: it knows nothing about the
    /// events in the group. For event-aware routing (DAY-88 /
    /// CORR-v0.2-06 unattached-comment override) callers should
    /// prefer [`Self::from_group`], which defers to this function
    /// after first checking event-level predicates.
    pub(crate) fn from_payload(payload: &ArtifactPayload) -> Self {
        match payload {
            ArtifactPayload::CommitSet { .. } => Self::Commits,
            ArtifactPayload::JiraIssue { .. } => Self::JiraIssues,
            ArtifactPayload::ConfluencePage { .. } => Self::ConfluencePages,
        }
    }

    /// Route a rolled-up group to its section.
    ///
    /// Extends [`Self::from_payload`] with an event-level override:
    /// a Confluence synthetic-page group whose events all carry
    /// `metadata.unattached == true` (emitted by
    /// `connector_confluence::normalise::normalise_comment` when a
    /// comment has no discoverable parent page — see
    /// CORR-v0.2-06 in DAY-88) is routed to [`Self::Other`] rather
    /// than [`Self::ConfluencePages`]. "All events" is deliberate:
    /// if even one event in the group has a real parent page the
    /// group deserves to render under Confluence pages; the override
    /// only fires when every bullet in the group would otherwise be
    /// lying about belonging to a real page.
    pub(crate) fn from_group(group: &RolledUpArtifact) -> Self {
        if matches!(
            &group.artifact.payload,
            ArtifactPayload::ConfluencePage { .. }
        ) && !group.events.is_empty()
            && group.events.iter().all(|ev| {
                ev.kind == ActivityKind::ConfluenceComment
                    && ev.metadata.get("unattached") == Some(&json!(true))
            })
        {
            return Self::Other;
        }
        Self::from_payload(&group.artifact.payload)
    }

    /// Stable identifier written to
    /// [`dayseam_core::RenderedSection::id`].
    ///
    /// Downstream consumers (the markdown sink, the desktop preview,
    /// the streaming IPC) key on these strings; changing them is a
    /// breaking UI contract. `snake_case` to match the existing
    /// `commits` id shipped in v0.1.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::JiraIssues => "jira_issues",
            Self::ConfluencePages => "confluence_pages",
            Self::Other => "other",
        }
    }

    /// Human-readable heading written to
    /// [`dayseam_core::RenderedSection::title`] and rendered as
    /// `## <title>` by the markdown sink. Sentence case matches the
    /// existing `Commits` heading; the two new titles follow the
    /// same convention.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Commits => "Commits",
            Self::JiraIssues => "Jira issues",
            Self::ConfluencePages => "Confluence pages",
            Self::Other => "Other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone, Utc};
    use dayseam_core::{
        ActivityEvent, Actor, Artifact, ArtifactId, ArtifactKind, EntityRef, Privacy, RawRef,
    };
    use uuid::Uuid;

    fn commit_set() -> ArtifactPayload {
        ArtifactPayload::CommitSet {
            repo_path: "/work/repo-a".into(),
            date: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            event_ids: vec![Uuid::nil()],
            commit_shas: vec!["sha".into()],
        }
    }

    fn jira_issue() -> ArtifactPayload {
        ArtifactPayload::JiraIssue {
            issue_key: "CAR-5117".into(),
            project_key: "CAR".into(),
            date: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            event_ids: vec![Uuid::nil()],
        }
    }

    fn confluence_page() -> ArtifactPayload {
        ArtifactPayload::ConfluencePage {
            page_id: "12345".into(),
            space_key: "ENG".into(),
            date: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            event_ids: vec![Uuid::nil()],
        }
    }

    fn confluence_comment_event(unattached: bool) -> ActivityEvent {
        let metadata = if unattached {
            json!({ "location": "footer", "unattached": true })
        } else {
            json!({ "location": "footer" })
        };
        ActivityEvent {
            id: Uuid::from_u128(1),
            source_id: Uuid::from_u128(0xaaaa),
            external_id: "comment:42".into(),
            kind: ActivityKind::ConfluenceComment,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Me".into(),
                email: None,
                external_id: Some("acct-1".into()),
            },
            title: "Comment on page".into(),
            body: None,
            links: vec![],
            entities: vec![EntityRef {
                kind: "confluence_space".into(),
                external_id: "ST".into(),
                label: None,
            }],
            parent_external_id: None,
            metadata,
            raw_ref: RawRef {
                storage_key: "confluence:comment:42".into(),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn rolled_group(payload: ArtifactPayload, events: Vec<ActivityEvent>) -> RolledUpArtifact {
        let source_id = Uuid::from_u128(0xaaaa);
        RolledUpArtifact {
            artifact: Artifact {
                id: ArtifactId::deterministic(&source_id, ArtifactKind::ConfluencePage, "x"),
                source_id,
                kind: match payload {
                    ArtifactPayload::CommitSet { .. } => ArtifactKind::CommitSet,
                    ArtifactPayload::JiraIssue { .. } => ArtifactKind::JiraIssue,
                    ArtifactPayload::ConfluencePage { .. } => ArtifactKind::ConfluencePage,
                },
                external_id: "x".into(),
                payload,
                created_at: Utc.with_ymd_and_hms(2026, 4, 20, 0, 0, 0).unwrap(),
            },
            events,
        }
    }

    #[test]
    fn from_payload_maps_each_variant_to_its_section() {
        assert_eq!(
            ReportSection::from_payload(&commit_set()),
            ReportSection::Commits
        );
        assert_eq!(
            ReportSection::from_payload(&jira_issue()),
            ReportSection::JiraIssues,
        );
        assert_eq!(
            ReportSection::from_payload(&confluence_page()),
            ReportSection::ConfluencePages,
        );
    }

    /// CORR-v0.2-06. Confluence group whose every event carries
    /// `metadata.unattached == true` (comments whose parent page
    /// couldn't be resolved) must route to `Other`, not
    /// `ConfluencePages`. Without this override the v0.2 shipped bug
    /// re-surfaces: those bullets lie about belonging to a real
    /// page.
    #[test]
    fn from_group_routes_all_unattached_confluence_comments_to_other() {
        let group = rolled_group(
            confluence_page(),
            vec![
                confluence_comment_event(true),
                confluence_comment_event(true),
            ],
        );
        assert_eq!(ReportSection::from_group(&group), ReportSection::Other);
    }

    /// Symmetric guard: one unattached + one attached → group still
    /// routes to `ConfluencePages` because the attached comment
    /// needs its real parent page to render correctly. Prevents an
    /// overzealous predicate from evicting legitimate bullets.
    #[test]
    fn from_group_keeps_mixed_attached_and_unattached_in_confluence_pages() {
        let mut attached = confluence_comment_event(false);
        attached.id = Uuid::from_u128(2);
        let group = rolled_group(
            confluence_page(),
            vec![confluence_comment_event(true), attached],
        );
        assert_eq!(
            ReportSection::from_group(&group),
            ReportSection::ConfluencePages,
        );
    }

    /// The happy path must not drift: Confluence group with only
    /// attached comments stays in `ConfluencePages`.
    #[test]
    fn from_group_keeps_attached_confluence_comments_in_confluence_pages() {
        let group = rolled_group(confluence_page(), vec![confluence_comment_event(false)]);
        assert_eq!(
            ReportSection::from_group(&group),
            ReportSection::ConfluencePages,
        );
    }

    /// The `id` strings are part of the engine's external contract
    /// (the markdown sink and the streaming preview both key on
    /// them). Pin the exact bytes so a rename is a loud test
    /// failure, not a silent UI regression.
    #[test]
    fn ids_are_pinned_snake_case() {
        assert_eq!(ReportSection::Commits.id(), "commits");
        assert_eq!(ReportSection::JiraIssues.id(), "jira_issues");
        assert_eq!(ReportSection::ConfluencePages.id(), "confluence_pages");
        assert_eq!(ReportSection::Other.id(), "other");
    }

    /// Titles appear as `## <title>` in Obsidian notes users have
    /// already saved. Pin them for the same reason as `id`.
    #[test]
    fn titles_are_pinned_sentence_case() {
        assert_eq!(ReportSection::Commits.title(), "Commits");
        assert_eq!(ReportSection::JiraIssues.title(), "Jira issues");
        assert_eq!(ReportSection::ConfluencePages.title(), "Confluence pages");
        assert_eq!(ReportSection::Other.title(), "Other");
    }

    /// The derived `Ord` is what `build_sections` relies on to emit
    /// sections in render order. Declaration order IS render order;
    /// this test is the lock. If someone reorders the enum for
    /// "alphabetical" or similar reasons, Confluence pages will
    /// render before Commits and this assertion fires.
    #[test]
    fn ord_matches_render_order() {
        let mut sections = vec![
            ReportSection::Other,
            ReportSection::ConfluencePages,
            ReportSection::Commits,
            ReportSection::JiraIssues,
        ];
        sections.sort();
        assert_eq!(
            sections,
            vec![
                ReportSection::Commits,
                ReportSection::JiraIssues,
                ReportSection::ConfluencePages,
                ReportSection::Other,
            ],
            "derived Ord must render Commits → Jira issues → Confluence pages → Other",
        );
    }
}
