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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
}

/// Short string used as the `artifacts.kind` column value and as the
/// discriminator when deriving deterministic ids. Kept in one place so
/// the DB helper and the id derivation can never drift.
pub(crate) const fn artifact_kind_token(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::CommitSet => "CommitSet",
    }
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
