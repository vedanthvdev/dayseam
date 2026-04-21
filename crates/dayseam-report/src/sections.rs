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

use dayseam_core::ArtifactPayload;

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
}

impl ReportSection {
    /// Route an artifact payload to its section.
    ///
    /// Implemented as an exhaustive `match` so a new
    /// [`ArtifactPayload`] variant fails to compile until its
    /// routing is decided here. Do *not* add a wildcard arm —
    /// silent fall-through is the bug this module exists to
    /// prevent.
    pub(crate) fn from_payload(payload: &ArtifactPayload) -> Self {
        match payload {
            ArtifactPayload::CommitSet { .. } => Self::Commits,
            ArtifactPayload::JiraIssue { .. } => Self::JiraIssues,
            ArtifactPayload::ConfluencePage { .. } => Self::ConfluencePages,
        }
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
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

    /// The `id` strings are part of the engine's external contract
    /// (the markdown sink and the streaming preview both key on
    /// them). Pin the exact bytes so a rename is a loud test
    /// failure, not a silent UI regression.
    #[test]
    fn ids_are_pinned_snake_case() {
        assert_eq!(ReportSection::Commits.id(), "commits");
        assert_eq!(ReportSection::JiraIssues.id(), "jira_issues");
        assert_eq!(ReportSection::ConfluencePages.id(), "confluence_pages");
    }

    /// Titles appear as `## <title>` in Obsidian notes users have
    /// already saved. Pin them for the same reason as `id`.
    #[test]
    fn titles_are_pinned_sentence_case() {
        assert_eq!(ReportSection::Commits.title(), "Commits");
        assert_eq!(ReportSection::JiraIssues.title(), "Jira issues");
        assert_eq!(ReportSection::ConfluencePages.title(), "Confluence pages");
    }

    /// The derived `Ord` is what `build_sections` relies on to emit
    /// sections in render order. Declaration order IS render order;
    /// this test is the lock. If someone reorders the enum for
    /// "alphabetical" or similar reasons, Confluence pages will
    /// render before Commits and this assertion fires.
    #[test]
    fn ord_matches_render_order() {
        let mut sections = vec![
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
            ],
            "derived Ord must render Commits before Jira issues before Confluence pages",
        );
    }
}
