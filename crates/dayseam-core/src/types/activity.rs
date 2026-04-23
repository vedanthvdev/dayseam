//! Activity events — the normalised, source-agnostic record produced by
//! every connector. One row here is one thing the user did or had done to
//! them on a given date.
//!
//! **Naming convention for [`ActivityKind`] variants (DAY-89 CONS-v0.2-05).**
//! Each connector gets its own verb family that matches how the upstream
//! product *describes* the action; renaming across connectors to a single
//! shared verb would flatten real semantic difference (a GitLab commit is
//! `Authored`; a Jira issue is `Created`; a Confluence page is `Created`
//! and later `Edited`). The pattern below is the contract — new variants
//! must either fit an existing family or extend one documented here:
//!
//! - **Local-git / GitLab commit**: `{Source}CommitAuthored` — git's own
//!   object-graph vernacular.
//! - **GitLab merge request**: `MrOpened` / `MrMerged` / `MrClosed` /
//!   `MrReviewComment` / `MrUnassigned` — lifecycle verbs match the
//!   review UI.
//! - **GitLab issue**: `IssueOpened` / `IssueClosed` / `IssueComment` —
//!   same.
//! - **Jira issue**: `JiraIssueCreated` / `JiraIssueTransitioned` /
//!   `JiraIssueAssigned` / `JiraIssueUnassigned` / `JiraIssueComment` —
//!   Jira's own CRUD + workflow verbs.
//! - **Confluence**: `ConfluencePageCreated` / `ConfluencePageEdited` /
//!   `ConfluenceComment` — Confluence's own page-lifecycle verbs.
//! - **GitHub**: `GitHubPullRequest{Opened,Merged,Closed,Reviewed,Commented}`
//!   / `GitHubIssue{Opened,Closed,Commented,Assigned}` — GitHub's own
//!   PR / issue lifecycle verbs. Parallels the GitLab `Mr*` family
//!   one-to-one except for naming (GitHub's "pull request" vs
//!   GitLab's "merge request"); the renaming matters because the
//!   v0.4 first-class `ArtifactPayload::MergeRequest` variant
//!   unifies them at the report-render layer while keeping the
//!   upstream-verb fidelity at the event-emit layer.
//!
//! A new variant that breaks the pattern (e.g. `ConfluencePageAuthored`)
//! should be rejected at review; the connector-specific verb family is
//! the load-bearing convention for cross-source reporting.

use chrono::{DateTime, Utc};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;
use ts_rs::TS;
use uuid::Uuid;

use super::source::SourceId;

/// A single piece of evidence from a source — one commit, one merge request
/// state change, one issue comment, etc. Everything the report engine sees
/// is an `ActivityEvent`.
///
/// DAY-109 TST-v0.4-01: carries `#[derive(SerdeDefaultAudit)]` as a
/// forward-looking guard. `ActivityEvent` rows round-trip through the
/// `activity_events` table on every sync; a future author adding a
/// `#[serde(default)]` field (e.g. a `dedup_token` with a back-compat
/// default for rows written before the field existed) without a paired
/// `#[serde_default_audit(...)]` annotation is exactly the DOG-v0.2-04
/// silent-failure shape, and the derive is the class detector.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, dayseam_macros::SerdeDefaultAudit,
)]
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
///
/// The `Jira*` and `Confluence*` variants were added in DAY-73 (v0.2
/// Atlassian connectors) to anchor the event vocabulary before any
/// connector can emit them. No connector in this PR produces them — the
/// walkers in DAY-77 (Jira) and DAY-80 (Confluence) do. Keeping the
/// enum additive here means later tasks can TDD walker behaviour against
/// a stable vocabulary without an intermediate core-types amendment.
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
    /// Jira issue status transition (e.g. "In Progress" → "In Review").
    /// Emitted once per `changelog` item where `field == "status"` and
    /// `author.accountId == self`. Rapid cascades within the
    /// `RAPID_TRANSITION_WINDOW_SECONDS` window collapse into one
    /// event with `metadata.transition_count` in DAY-77.
    JiraIssueTransitioned,
    /// Jira issue comment authored by the user. `body` is the
    /// ADF-to-plain-text rendering (DAY-75 `connector-atlassian-common::adf`).
    JiraIssueCommented,
    /// Jira issue assignee changed to the user (a changelog item where
    /// `field == "assignee"` and `toString == self.displayName`).
    /// Distinct from `JiraIssueTransitioned` because being assigned a
    /// ticket is a discrete calendar event in a dev's EOD narrative even
    /// when the status stays the same.
    JiraIssueAssigned,
    /// Jira issue unassigned from the user (a changelog item where
    /// `field == "assignee"` and `from == self.accountId`, regardless of
    /// what `to` is set to — empty for true unassignments, another
    /// accountId for reassignments to a teammate). Symmetric with
    /// `JiraIssueAssigned`: a dev wants to see "I handed off CAR-5117"
    /// on their EOD as much as "I picked up CAR-5117". Added in DAY-88
    /// (CORR-v0.2-07 reshaped): the original v0.2 review noted "assigned
    /// to "" nonsense" but the walker filter already dropped empty `to`;
    /// the real bug was losing the `from == self` side entirely.
    JiraIssueUnassigned,
    /// Jira issue created by the user (`reporter == self AND created_at in window`).
    JiraIssueCreated,
    /// Confluence page created by the user — `createdDate == lastModified`
    /// AND `createdBy.accountId == self`. Distinct from `ConfluencePageEdited`
    /// because "I wrote a new doc today" is different signal from
    /// "I revised a doc today".
    ConfluencePageCreated,
    /// Confluence page edited by the user — any `version.number > 1`
    /// authored by `self`. Multiple rapid autosave versions collapse to
    /// one event in DAY-80 rollup.
    ConfluencePageEdited,
    /// Confluence comment (inline or footer) authored by the user.
    /// `body` is the ADF-to-plain-text rendering.
    ConfluenceComment,
    /// GitHub pull request opened by the user. Emitted when the
    /// `/users/{login}/events` stream surfaces a
    /// `PullRequestEvent { action: "opened" }`. Added in DAY-93
    /// (v0.4); the walker in DAY-96 is the first producer.
    GitHubPullRequestOpened,
    /// GitHub pull request merged. Emitted when the user's
    /// `PullRequestEvent { action: "closed", payload.pull_request.merged: true }`
    /// lands. Distinct from `GitHubPullRequestClosed` because
    /// "shipped" and "abandoned" read very differently in an EOD
    /// narrative.
    GitHubPullRequestMerged,
    /// GitHub pull request closed without merging. Paired with
    /// `GitHubPullRequestMerged` — together they cover the
    /// `closed` action space.
    GitHubPullRequestClosed,
    /// GitHub pull request review submitted by the user —
    /// `PullRequestReviewEvent { action: "submitted" }`. The
    /// `metadata` carries the review `state` (`"approved"` /
    /// `"changes_requested"` / `"commented"`); rapid-review collapse
    /// within `RAPID_REVIEW_WINDOW_SECONDS` folds multiple reviews
    /// on the same PR into one event in DAY-96 rollup.
    GitHubPullRequestReviewed,
    /// GitHub PR review-thread comment authored by the user —
    /// `IssueCommentEvent` on an issue carrying a `pull_request`
    /// link, or `PullRequestReviewCommentEvent`. Distinct from
    /// `GitHubIssueCommented` because a PR review comment renders
    /// under `## Merge requests` while an issue comment renders
    /// under `## Issues`.
    GitHubPullRequestCommented,
    /// GitHub issue opened by the user — `IssuesEvent { action: "opened" }`.
    GitHubIssueOpened,
    /// GitHub issue closed by the user — `IssuesEvent { action: "closed" }`.
    GitHubIssueClosed,
    /// GitHub issue comment authored by the user on a pure issue
    /// (not a PR's issue-thread comment, which maps to
    /// `GitHubPullRequestCommented`).
    GitHubIssueCommented,
    /// GitHub issue assigned to the user — `IssuesEvent { action: "assigned" }`
    /// where `assignee.login == self.login`. Symmetric with
    /// `JiraIssueAssigned`: being handed a ticket is a discrete
    /// calendar event worth surfacing regardless of state change.
    GitHubIssueAssigned,
}

impl ActivityKind {
    /// Every variant, in declaration order.
    ///
    /// Exists so tests can exhaustively cover every kind without
    /// depending on a third-party `EnumIter` derive. The returned slice
    /// is guaranteed to have exactly one entry per variant at compile
    /// time; adding a new variant without updating this array breaks
    /// the `all_activity_kinds_matches_declaration_order` test. DAY-73.
    pub const fn all() -> &'static [ActivityKind] {
        &[
            ActivityKind::CommitAuthored,
            ActivityKind::MrOpened,
            ActivityKind::MrMerged,
            ActivityKind::MrClosed,
            ActivityKind::MrReviewComment,
            ActivityKind::MrApproved,
            ActivityKind::IssueOpened,
            ActivityKind::IssueClosed,
            ActivityKind::IssueComment,
            ActivityKind::JiraIssueTransitioned,
            ActivityKind::JiraIssueCommented,
            ActivityKind::JiraIssueAssigned,
            ActivityKind::JiraIssueUnassigned,
            ActivityKind::JiraIssueCreated,
            ActivityKind::ConfluencePageCreated,
            ActivityKind::ConfluencePageEdited,
            ActivityKind::ConfluenceComment,
            ActivityKind::GitHubPullRequestOpened,
            ActivityKind::GitHubPullRequestMerged,
            ActivityKind::GitHubPullRequestClosed,
            ActivityKind::GitHubPullRequestReviewed,
            ActivityKind::GitHubPullRequestCommented,
            ActivityKind::GitHubIssueOpened,
            ActivityKind::GitHubIssueClosed,
            ActivityKind::GitHubIssueCommented,
            ActivityKind::GitHubIssueAssigned,
        ]
    }
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
/// The `(kind, external_id)` pair is the report engine's stable key for
/// the referenced entity; each connector picks the kind for the objects
/// it emits.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS, dayseam_macros::SerdeDefaultAudit,
)]
#[ts(export)]
pub struct EntityRef {
    pub kind: EntityKind,
    pub external_id: String,
    pub label: Option<String>,
}

/// Discriminant for the upstream object an [`EntityRef`] points at.
///
/// **Why an enum, not a `String` (DAY-89 CONS-v0.2-03).** Until v0.2.1 the
/// `kind` was a free-form `String` and every call site re-encoded the
/// convention (`"jira_issue"`, `"confluence_page"`, `"repo"`, …) as a
/// literal. That allowed three bug classes the review doc called out:
/// typos that silently mis-routed events (`"jira-issue"` vs
/// `"jira_issue"`), drift between emit-site and query-site, and no
/// exhaustive-match guarantee in the report engine. Making `kind` an enum
/// turns each of those into a compile-time error.
///
/// **Serialised form is lossless with v0.2.1 rows.** Variants serialise
/// as their documented `snake_case` strings (`JiraIssue` → `"jira_issue"`,
/// etc.) — identical to the strings every connector already wrote. A
/// kind the current binary doesn't recognise (either a v0.2.1 `"mr"`
/// that never reached production or a future connector's new kind)
/// deserialises as [`EntityKind::Other`] carrying the original string,
/// so round-trips are byte-stable and we need no boot-time repair to
/// read old `activity_events` rows.
///
/// The custom [`Serialize`] / [`Deserialize`] impls below enforce the
/// string shape; `#[derive(Serialize, Deserialize)]` would have produced
/// a tagged object which the v0.2.1 upgrade path would not tolerate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, TS)]
#[ts(export, type = "string")]
pub enum EntityKind {
    /// A git repository — the unit local-git and GitLab both bucket
    /// commits by.
    Repo,
    /// A GitLab project (distinct from `Repo`: GitLab projects carry
    /// MRs/issues, the repo is their git storage).
    Project,
    /// The target of a GitLab merge-request / issue event — an MR or
    /// an issue referenced by `iid` within a project.
    Target,
    /// A Jira project (`PROJ` key).
    JiraProject,
    /// A Jira issue (`PROJ-123` key).
    JiraIssue,
    /// A Confluence space.
    ConfluenceSpace,
    /// A Confluence page.
    ConfluencePage,
    /// A Confluence comment on a page.
    ConfluenceComment,
    /// A GitHub repository — `owner/name` form; the unit GitHub
    /// commits / PR-linked push events bucket by. Parallels
    /// [`Self::Repo`] for local-git + GitLab; kept separate so
    /// evidence-popover URL templates don't have to carry a
    /// "which provider is this repo from?" side-channel.
    /// Added in DAY-93.
    GitHubRepo,
    /// A GitHub pull request — `owner/repo#number` form. Added
    /// in DAY-93; promoted to first-class at the render layer via
    /// [`super::artifact::ArtifactPayload::MergeRequest`] in DAY-98.
    GitHubPullRequest,
    /// A GitHub issue — `owner/repo#number` form. Distinct from
    /// [`Self::GitHubPullRequest`] even though both are numbered
    /// in the same per-repo sequence, because report layout and
    /// enrichment rules differ (PRs annotate Jira transitions;
    /// issues do not).
    GitHubIssue,
    /// A cross-source "account / tenant / workspace" container —
    /// the GitHub account, the GitLab group, the Atlassian cloud
    /// instance, the Slack workspace. The v0.3 capstone
    /// (DAY-89) called this variant out as deferred-by-design
    /// pending a concrete call site; v0.4's GitHub connector is
    /// that call site. Each [`crate::SourceIdentity`] whose
    /// external id names an account / tenant carries an
    /// `EntityRef { kind: Workspace, external_id: <tenant>, label: Some(<display>) }`
    /// so the report engine can surface "I did work across N
    /// workspaces today" without re-parsing per-connector id
    /// formats. Added in DAY-93.
    Workspace,
    /// A kind string this binary doesn't enumerate — either a row
    /// written by a newer Dayseam version or a legacy convention a
    /// past connector emitted. Preserved verbatim so re-serialising
    /// the row produces byte-identical JSON.
    Other(String),
}

impl EntityKind {
    /// The stable serialised form — what a v0.2.1 row stored in the
    /// `activity_events.entities` JSON column and what every connector
    /// still writes. Keep in sync with [`Serialize`] /
    /// [`Deserialize`]; the pair are symmetric by construction.
    pub fn as_str(&self) -> &str {
        match self {
            EntityKind::Repo => "repo",
            EntityKind::Project => "project",
            EntityKind::Target => "target",
            EntityKind::JiraProject => "jira_project",
            EntityKind::JiraIssue => "jira_issue",
            EntityKind::ConfluenceSpace => "confluence_space",
            EntityKind::ConfluencePage => "confluence_page",
            EntityKind::ConfluenceComment => "confluence_comment",
            EntityKind::GitHubRepo => "github_repo",
            EntityKind::GitHubPullRequest => "github_pull_request",
            EntityKind::GitHubIssue => "github_issue",
            EntityKind::Workspace => "workspace",
            EntityKind::Other(s) => s.as_str(),
        }
    }

    /// Parse from the stable serialised form. Unknown strings land in
    /// [`EntityKind::Other`] — we never lose data at the deserialise
    /// boundary.
    ///
    /// This is an inherent method rather than an `std::str::FromStr`
    /// impl because the parse is infallible: there is no error
    /// condition to plumb through a `Result`. Clippy would prefer
    /// `FromStr`; we disagree — forcing every caller to write
    /// `"..".parse::<EntityKind>().unwrap()` for a parse that can't
    /// fail is worse ergonomics than a plainly-named method.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "repo" => EntityKind::Repo,
            "project" => EntityKind::Project,
            "target" => EntityKind::Target,
            "jira_project" => EntityKind::JiraProject,
            "jira_issue" => EntityKind::JiraIssue,
            "confluence_space" => EntityKind::ConfluenceSpace,
            "confluence_page" => EntityKind::ConfluencePage,
            "confluence_comment" => EntityKind::ConfluenceComment,
            "github_repo" => EntityKind::GitHubRepo,
            "github_pull_request" => EntityKind::GitHubPullRequest,
            "github_issue" => EntityKind::GitHubIssue,
            "workspace" => EntityKind::Workspace,
            other => EntityKind::Other(other.to_string()),
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for EntityKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EntityKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntityKindVisitor;

        impl Visitor<'_> for EntityKindVisitor {
            type Value = EntityKind;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a snake_case entity-kind string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<EntityKind, E> {
                Ok(EntityKind::from_str(value))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<EntityKind, E> {
                Ok(EntityKind::from_str(&value))
            }
        }

        deserializer.deserialize_str(EntityKindVisitor)
    }
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

    /// DAY-73. Guards against two drift modes on `ActivityKind`:
    ///
    /// 1. A future PR adds a variant to the enum but forgets to extend
    ///    [`ActivityKind::all`], which every serde / rollup / render
    ///    test iterates over.
    /// 2. A future PR quietly removes or renames a v0.1 variant, which
    ///    would break on-disk compatibility for any user's persisted
    ///    `activity_events.kind` column.
    ///
    /// Adjusting the expected count here is deliberate: it forces the
    /// change author to acknowledge the enum size shift in review.
    #[test]
    fn all_activity_kinds_has_expected_count_and_is_unique() {
        let kinds = ActivityKind::all();
        assert_eq!(
            kinds.len(),
            26,
            "ActivityKind::all() must list every declared variant exactly once"
        );
        let mut set = std::collections::HashSet::new();
        for k in kinds {
            assert!(
                set.insert(*k),
                "duplicate variant in ActivityKind::all(): {k:?}"
            );
        }
    }

    /// CONS-v0.2-03. Every enumerated [`EntityKind`] variant round-trips
    /// through the serde impl as the exact string the connectors have
    /// been writing since v0.1. Breaks in CI if anyone renames a
    /// variant string or changes the serialised shape; a single rename
    /// would silently bucket v0.2.1 rows under [`EntityKind::Other`].
    #[test]
    fn entity_kind_serialised_form_is_stable_for_every_enumerated_variant() {
        let cases = [
            (EntityKind::Repo, "repo"),
            (EntityKind::Project, "project"),
            (EntityKind::Target, "target"),
            (EntityKind::JiraProject, "jira_project"),
            (EntityKind::JiraIssue, "jira_issue"),
            (EntityKind::ConfluenceSpace, "confluence_space"),
            (EntityKind::ConfluencePage, "confluence_page"),
            (EntityKind::ConfluenceComment, "confluence_comment"),
            (EntityKind::GitHubRepo, "github_repo"),
            (EntityKind::GitHubPullRequest, "github_pull_request"),
            (EntityKind::GitHubIssue, "github_issue"),
            (EntityKind::Workspace, "workspace"),
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).expect("serialize");
            assert_eq!(
                json,
                format!("\"{expected}\""),
                "variant {variant:?} must serialise as \"{expected}\""
            );
            let round: EntityKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(
                round, variant,
                "variant {variant:?} must round-trip via serde"
            );
        }
    }

    /// CONS-v0.2-03. A kind string this binary doesn't enumerate —
    /// either a future connector's new kind or a legacy string from a
    /// pre-v0.3 row — survives round-trip as [`EntityKind::Other`] with
    /// the original value preserved verbatim. This is the v0.2.1 -> v0.3
    /// upgrade-path invariant: no serde-side repair is needed because
    /// the deserialiser never loses data.
    #[test]
    fn entity_kind_unknown_variant_round_trips_as_other_verbatim() {
        let input = "\"hypothetical_future_kind\"";
        let parsed: EntityKind = serde_json::from_str(input).expect("deserialize");
        assert_eq!(parsed, EntityKind::Other("hypothetical_future_kind".into()));
        let re_serialised = serde_json::to_string(&parsed).expect("serialize");
        assert_eq!(
            re_serialised, input,
            "unknown kinds must re-serialise byte-identically"
        );
    }

    /// CONS-v0.2-03. Full `EntityRef` JSON shape matches v0.2.1 so a
    /// `activity_events.entities` column written by v0.2.1 deserialises
    /// into v0.3 without a migration.
    #[test]
    fn entity_ref_json_shape_matches_v0_2_1_layout() {
        let v0_2_1_blob = r#"{"kind":"jira_issue","external_id":"CAR-5117","label":"Do a thing"}"#;
        let parsed: EntityRef = serde_json::from_str(v0_2_1_blob).expect("deserialize");
        assert_eq!(parsed.kind, EntityKind::JiraIssue);
        assert_eq!(parsed.external_id, "CAR-5117");
        assert_eq!(parsed.label.as_deref(), Some("Do a thing"));
        let re_serialised = serde_json::to_string(&parsed).expect("serialize");
        assert_eq!(
            re_serialised, v0_2_1_blob,
            "EntityRef JSON shape must be byte-stable across v0.2.1 -> v0.3"
        );
    }
}
