//! Canonical upstream artefacts produced by connectors and consumed by the
//! report engine. `ActivityEvent`s describe *what happened*; an `Artifact`
//! is the grouped, upstream-named thing the event belongs to — a day's
//! worth of commits on one repo, a merge request, an issue thread, a
//! docs page. Phase 2 ships one variant (`CommitSet`); later phases add
//! `MergeRequest`, `Issue`, `Thread`, `Page` without schema migration
//! because the on-disk shape is an additive externally-tagged JSON enum.
//!
//! The report engine's rollup stage (`ARCHITECTURE.md` §7A) keys entirely
//! off `(source_id, kind, external_id)` and follows `ActivityEvent::
//! artifact_id` links; anything below the engine only sees the enum.

use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::source::SourceId;

/// Opaque id for an [`Artifact`]. Wrapped [`Uuid`] so callers cannot
/// accidentally pass a [`SourceId`] or a commit SHA in its place. The
/// wire format is a plain string-encoded UUID, identical to every other
/// id in the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactId(pub Uuid);

impl ArtifactId {
    /// Fresh random id. Prefer [`ArtifactId::deterministic`] whenever the
    /// caller already knows the `(source_id, kind, external_id)` tuple
    /// the artefact represents; deterministic ids make re-syncs a no-op
    /// at the DB layer instead of creating duplicate rows.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Deterministic id derived from `(source_id, kind, external_id)` via
    /// UUIDv5. The per-source namespace guarantees two distinct sources
    /// cannot collide even if they happen to reuse the same
    /// `(kind, external_id)` pair.
    ///
    /// Mirrors [`super::activity::ActivityEvent::deterministic_id`] so the
    /// rules for "same upstream record ⇒ same primary key" are identical
    /// on both sides of the rollup edge.
    pub fn deterministic(source_id: &SourceId, kind: ArtifactKind, external_id: &str) -> Self {
        let ns = Uuid::new_v5(&Uuid::NAMESPACE_OID, source_id.as_bytes());
        let kind_str = artifact_kind_token(kind);
        Self(Uuid::new_v5(
            &ns,
            format!("{kind_str}::{external_id}").as_bytes(),
        ))
    }

    /// Inner UUID, exposed for persistence and logging.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for ArtifactId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The persisted canonical artefact record. Shape mirrors `artifacts`
/// in `0002_artifact_syncrun.sql`: `kind` is a short discriminator token
/// and `payload` is the externally-tagged JSON blob the connector wrote.
///
/// DAY-100 TST-v0.3-01: carries `#[derive(SerdeDefaultAudit)]` as a
/// forward-looking guard. No field is currently `#[serde(default)]`;
/// the derive forces the next author who adds one (e.g. a
/// retroactively-added `rolled_up_count` for back-compat) to pair it
/// with a `#[serde_default_audit(...)]` annotation, closing the
/// DOG-v0.2-04 silent-failure avenue on the persisted-artifact layer
/// the same way it already is on `SourceConfig`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, dayseam_macros::SerdeDefaultAudit,
)]
#[ts(export)]
pub struct Artifact {
    pub id: ArtifactId,
    pub source_id: SourceId,
    pub kind: ArtifactKind,
    /// Stable identifier assigned by the upstream (repo path, MR iid,
    /// issue iid, …). Used together with `kind` to compute `id`.
    pub external_id: String,
    pub payload: ArtifactPayload,
    pub created_at: DateTime<Utc>,
}

/// The kinds of artefact Dayseam currently recognises. Adding a variant
/// is additive (minor bump); renaming or removing one is a breaking
/// change that must be reflected in every connector, in the report
/// engine, and in the migrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ArtifactKind {
    /// A day's worth of commits on one local-git repo. First and only
    /// variant shipped in Phase 2; `ARCHITECTURE.md` §7A lists the rest.
    CommitSet,
    /// A single Jira issue, used by the v0.2 Jira connector (DAY-77) to
    /// carry per-issue state (current status, rolled-up transitions,
    /// comment bodies) that survives across multiple `ActivityEvent`s
    /// for the same issue on the same day. Added in DAY-73.
    JiraIssue,
    /// A single Confluence page, used by the v0.2 Confluence connector
    /// (DAY-80) to carry per-page state (final title, version id,
    /// collapsed rapid-save evidence) that survives across edit
    /// events. Added in DAY-73.
    ConfluencePage,
    /// A single GitHub pull request, rolled up across a single
    /// day's opened / merged / closed / reviewed / commented events
    /// so `#42` renders as one bullet under `## Merge requests`
    /// rather than five. Paired with [`ArtifactPayload::MergeRequest`]
    /// carrying `MergeRequestProvider::GitHub`. The v0.4 GitHub
    /// rollup (DAY-96) is the first producer; no v0.3 code emits
    /// this variant.
    GitHubPullRequest,
    /// A single GitHub issue, rolled up across a single day's
    /// opened / closed / commented / assigned events. Kept
    /// distinct from [`Self::GitHubPullRequest`] because GitHub's
    /// report-layer rules differ (PRs annotate Jira transitions,
    /// issues don't). The v0.4 GitHub rollup (DAY-96) is the
    /// first producer. Added in DAY-93.
    GitHubIssue,
}

/// Short string used as the `artifacts.kind` column value and as the
/// discriminator when deriving deterministic ids. Kept in one place so
/// the DB helper and the id derivation can never drift.
pub(crate) const fn artifact_kind_token(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::CommitSet => "CommitSet",
        ArtifactKind::JiraIssue => "JiraIssue",
        ArtifactKind::ConfluencePage => "ConfluencePage",
        ArtifactKind::GitHubPullRequest => "GitHubPullRequest",
        ArtifactKind::GitHubIssue => "GitHubIssue",
    }
}

/// Which forge a [`ArtifactPayload::MergeRequest`] came from. An
/// enum rather than a string so the report-render layer can match
/// exhaustively on the provider (a future `github.com → enterprise`
/// distinction or a third forge lands as an enum variant, not a
/// magic-string bug).
///
/// v0.4 lands two variants so the fifth connector and the v0.3
/// GitLab rollup both land under the same `## Merge requests`
/// section. Added in DAY-93.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeRequestProvider {
    /// GitLab merge request. v0.3's GitLab walker previously
    /// folded MRs into the commit suffix under `## Commits`; the
    /// v0.4 promotion migrates them to first-class bullets under
    /// `## Merge requests` with no schema change (the `Artifact`
    /// row shape was already reserved).
    GitLab,
    /// GitHub pull request. The v0.4 GitHub connector's first
    /// producer.
    GitHub,
}

/// The variant-specific payload, externally tagged so the on-disk JSON
/// carries the variant name and the database `kind` column agrees with
/// the payload discriminator. Matches the `SourceConfig` precedent
/// (`crates/dayseam-core/src/types/source.rs`) verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ArtifactPayload {
    /// One repo, one local-timezone day. Carries the repo path so the
    /// report engine can render "I worked on X" without a join, the
    /// `event_ids` that rolled up into this artefact (so evidence
    /// links stay traceable), and the raw commit SHAs for renderers
    /// that want to surface them directly.
    CommitSet {
        repo_path: PathBuf,
        date: NaiveDate,
        event_ids: Vec<Uuid>,
        commit_shas: Vec<String>,
    },
    /// One Jira issue, aggregated across a single day's transitions,
    /// comments, and assignee changes. The v0.2 Jira walker (DAY-77)
    /// writes one of these per `(source_id, issue_key, date)` tuple so
    /// the report engine can group all of today's activity on
    /// `CAR-5117` under one bullet rather than rendering each
    /// transition separately. Shape is intentionally minimal in
    /// DAY-73; the walker + rollup PRs extend it as concrete
    /// aggregation needs surface. Added in DAY-73.
    JiraIssue {
        /// The Jira issue key as rendered in URLs and UI, e.g.
        /// `"CAR-5117"`. Used as the grouping key for the EOD bullet
        /// and as part of `Artifact::external_id` for deterministic
        /// id derivation.
        issue_key: String,
        /// Project key (the prefix of `issue_key`), stored separately
        /// so the rollup stage can group by project without having to
        /// re-parse the key.
        project_key: String,
        date: NaiveDate,
        /// Event ids that rolled up into this artefact, so evidence
        /// links stay traceable the same way `CommitSet` does.
        event_ids: Vec<Uuid>,
    },
    /// One Confluence page, aggregated across a single day's
    /// created/edited/commented events. The v0.2 Confluence walker
    /// (DAY-80) writes one of these per `(source_id, page_id, date)`
    /// tuple so a page that saw an authored event plus several
    /// autosave edits renders as one bullet. Added in DAY-73.
    ConfluencePage {
        /// The Confluence content id for the page (opaque numeric
        /// string in Cloud). Stable across renames.
        page_id: String,
        /// The space key (e.g. `"ENG"`) for grouping by space in the
        /// report.
        space_key: String,
        date: NaiveDate,
        event_ids: Vec<Uuid>,
    },
    /// One merge request — a GitLab MR or a GitHub PR — aggregated
    /// across a single day's lifecycle events (opened / reviewed /
    /// merged / closed / review-commented). The v0.4 GitLab
    /// rollup (DAY-97) and GitHub rollup (DAY-96) both produce
    /// this variant so the report engine's `## Merge requests`
    /// section renders them uniformly; `provider` tells the
    /// renderer which URL template to use (`gitlab://…/merge_requests/NN`
    /// vs `github://…/pull/NN`) and which verb family
    /// (`MrOpened` vs `GitHubPullRequestOpened`) the rolled-up
    /// events belong to.
    ///
    /// `number` is the upstream-assigned numeric id (GitLab's
    /// `iid`, GitHub's `.number`). `project_key` is the
    /// `group/project` or `owner/repo` path — stable across
    /// renames at the upstream, used as the grouping key for the
    /// `Artifact`'s `external_id` alongside `number`.
    ///
    /// Added in DAY-93. No production code emits this variant
    /// until DAY-96/97; the shape is landed here so the walker
    /// and rollup PRs don't have to amend core types mid-stream.
    MergeRequest {
        provider: MergeRequestProvider,
        /// Upstream-assigned numeric id (`iid` in GitLab,
        /// `number` in GitHub). Kept typed rather than stringly
        /// so the renderer can't accidentally string-compare
        /// `"10"` and `"9"`.
        number: i64,
        /// `owner/repo` (GitHub) or `group/project` (GitLab) —
        /// the full path segment the forge's URL template
        /// joins onto its base URL.
        project_key: String,
        title: String,
        /// Fully-qualified URL of the MR / PR on the upstream
        /// forge; pre-computed at rollup time so the renderer
        /// doesn't repeat per-provider URL construction.
        url: String,
        date: NaiveDate,
        event_ids: Vec<Uuid>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(uuid: &str) -> SourceId {
        Uuid::parse_str(uuid).unwrap()
    }

    #[test]
    fn artifact_id_is_stable_across_calls() {
        let a = ArtifactId::deterministic(
            &src("11111111-1111-1111-1111-111111111111"),
            ArtifactKind::CommitSet,
            "repo-a::2026-04-17",
        );
        let b = ArtifactId::deterministic(
            &src("11111111-1111-1111-1111-111111111111"),
            ArtifactKind::CommitSet,
            "repo-a::2026-04-17",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn artifact_id_differs_by_source() {
        let a = ArtifactId::deterministic(
            &src("11111111-1111-1111-1111-111111111111"),
            ArtifactKind::CommitSet,
            "repo-a::2026-04-17",
        );
        let b = ArtifactId::deterministic(
            &src("22222222-2222-2222-2222-222222222222"),
            ArtifactKind::CommitSet,
            "repo-a::2026-04-17",
        );
        assert_ne!(a, b);
    }

    #[test]
    fn artifact_id_differs_by_external_id() {
        let a = ArtifactId::deterministic(
            &src("11111111-1111-1111-1111-111111111111"),
            ArtifactKind::CommitSet,
            "repo-a::2026-04-17",
        );
        let b = ArtifactId::deterministic(
            &src("11111111-1111-1111-1111-111111111111"),
            ArtifactKind::CommitSet,
            "repo-a::2026-04-18",
        );
        assert_ne!(a, b);
    }

    #[test]
    fn artifact_id_display_matches_uuid() {
        let id = ArtifactId(Uuid::nil());
        assert_eq!(id.to_string(), Uuid::nil().to_string());
    }

    #[test]
    fn artifact_id_new_is_distinct() {
        assert_ne!(ArtifactId::new(), ArtifactId::new());
    }
}
