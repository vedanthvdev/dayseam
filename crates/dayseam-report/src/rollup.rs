//! Stage 1 of the pipeline: bundle events into artifact-shaped groups
//! before the template sees them.
//!
//! The rollup is the only place the engine walks the many-to-one
//! relationship between [`ActivityEvent`]s and [`Artifact`]s. It
//! produces [`RolledUpArtifact`] records keyed by the artifact (real
//! or synthetic) and sorted so downstream rendering is deterministic.
//!
//! Two invariants worth reading twice:
//!
//! 1. **Every event lands in exactly one group.** An event belongs to
//!    an [`Artifact`] iff that artifact's payload claims its id; an
//!    event claimed by zero artifacts lands in a *synthetic*
//!    [`Artifact::CommitSet`] keyed by `(source_id, repo_path, date)`
//!    — the same shape the connector would have produced had it
//!    emitted one. This keeps the template blind to whether the
//!    connector pre-grouped or not.
//! 2. **Sort order is total.** Groups are ordered by
//!    `(kind_token, external_id)`; events inside a group are ordered
//!    by `(occurred_at, external_id, id)`. No hash-map iteration
//!    survives into the render stage.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::NaiveDate;
use dayseam_core::{ActivityEvent, Artifact, ArtifactId, ArtifactKind, ArtifactPayload, SourceId};
use uuid::Uuid;

/// One artifact's worth of events, ready to feed the template.
///
/// `artifact` is always a real [`Artifact`] — either one produced by a
/// connector or a synthetic [`ArtifactKind::CommitSet`] the rollup
/// minted to hold orphan events. The `events` vec is sorted and
/// already filtered (see [`rollup`]).
#[derive(Debug, Clone)]
pub(crate) struct RolledUpArtifact {
    /// The real or synthetic artifact this group is built around.
    pub(crate) artifact: Artifact,
    /// The events that belong to `artifact`, sorted by
    /// `(occurred_at, external_id, id)`.
    pub(crate) events: Vec<ActivityEvent>,
}

/// Roll up `events` against `artifacts`.
///
/// * Events whose id appears in an [`ArtifactPayload::CommitSet::event_ids`]
///   list are attached to that artifact.
/// * Events not claimed by any artifact are grouped into synthetic
///   [`ArtifactKind::CommitSet`] artifacts keyed by
///   `(source_id, repo_path, occurred_at.date_naive())`. `repo_path`
///   for synthetic groups is derived from the event's first
///   `EntityRef` of kind `"repo"`; if there is no such entity the
///   path falls back to `/` so every event still lands somewhere
///   deterministic.
/// * The returned vec is sorted by `(kind_token, external_id)`. Ties
///   are broken by artifact id for the pathological case where two
///   real artifacts share a kind + external_id (they cannot in
///   practice because the deterministic id derivation collides, but
///   the engine still sorts so a malformed input renders stably
///   rather than nondeterministically).
pub(crate) fn roll_up(
    events: &[ActivityEvent],
    artifacts: &[Artifact],
    report_date: NaiveDate,
) -> Vec<RolledUpArtifact> {
    let mut event_by_id: BTreeMap<Uuid, &ActivityEvent> =
        events.iter().map(|e| (e.id, e)).collect();

    let mut groups: Vec<RolledUpArtifact> = Vec::new();

    for artifact in artifacts {
        let claimed_ids: Vec<Uuid> = match &artifact.payload {
            ArtifactPayload::CommitSet { event_ids, .. } => event_ids.clone(),
        };

        let mut claimed_events: Vec<ActivityEvent> = claimed_ids
            .iter()
            .filter_map(|id| event_by_id.remove(id).cloned())
            .collect();
        sort_events(&mut claimed_events);

        groups.push(RolledUpArtifact {
            artifact: artifact.clone(),
            events: claimed_events,
        });
    }

    let mut orphan_by_key: BTreeMap<(SourceId, PathBuf, NaiveDate), Vec<ActivityEvent>> =
        BTreeMap::new();
    for (_, event) in event_by_id {
        let repo_path = repo_path_from_event(event);
        let day = event.occurred_at.naive_local().date();
        orphan_by_key
            .entry((event.source_id, repo_path, day))
            .or_default()
            .push(event.clone());
    }

    for ((source_id, repo_path, day), mut orphan_events) in orphan_by_key {
        sort_events(&mut orphan_events);
        let external_id = synthetic_external_id(&repo_path, day);
        let synthetic_id =
            ArtifactId::deterministic(&source_id, ArtifactKind::CommitSet, &external_id);
        let commit_shas: Vec<String> = orphan_events
            .iter()
            .map(|e| e.external_id.clone())
            .collect();
        let event_ids: Vec<Uuid> = orphan_events.iter().map(|e| e.id).collect();

        let artifact = Artifact {
            id: synthetic_id,
            source_id,
            kind: ArtifactKind::CommitSet,
            external_id,
            payload: ArtifactPayload::CommitSet {
                repo_path,
                date: day,
                event_ids,
                commit_shas,
            },
            // Synthetic artifacts never reach disk; the report draft
            // only cares that this timestamp is deterministic. Using
            // `report_date` at midnight UTC keeps a fixed point on
            // the day in question without reaching for a clock.
            created_at: report_date
                .and_hms_opt(0, 0, 0)
                .unwrap_or_default()
                .and_utc(),
        };

        groups.push(RolledUpArtifact {
            artifact,
            events: orphan_events,
        });
    }

    sort_groups(&mut groups);
    groups
}

fn sort_events(events: &mut [ActivityEvent]) {
    events.sort_by(|a, b| {
        a.occurred_at
            .cmp(&b.occurred_at)
            .then_with(|| a.external_id.cmp(&b.external_id))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn sort_groups(groups: &mut [RolledUpArtifact]) {
    groups.sort_by(|a, b| {
        kind_token(a.artifact.kind)
            .cmp(kind_token(b.artifact.kind))
            .then_with(|| a.artifact.external_id.cmp(&b.artifact.external_id))
            .then_with(|| a.artifact.id.as_uuid().cmp(&b.artifact.id.as_uuid()))
    });
}

const fn kind_token(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::CommitSet => "CommitSet",
    }
}

fn repo_path_from_event(event: &ActivityEvent) -> PathBuf {
    event
        .entities
        .iter()
        .find(|e| e.kind == "repo")
        .map(|e| PathBuf::from(&e.external_id))
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn synthetic_external_id(repo_path: &std::path::Path, day: NaiveDate) -> String {
    format!("{}::{}::synthetic", repo_path.display(), day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{
        Actor, ArtifactKind, ArtifactPayload, EntityRef, Privacy, RawRef, SourceId,
    };

    fn event(id: u128, source: SourceId, occurred_at_hour: u32, repo: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::from_u128(id),
            source_id: source,
            external_id: format!("sha{id}"),
            kind: dayseam_core::ActivityKind::CommitAuthored,
            occurred_at: Utc
                .with_ymd_and_hms(2026, 4, 18, occurred_at_hour, 0, 0)
                .unwrap(),
            actor: Actor {
                display_name: "Test".into(),
                email: Some("test@example.com".into()),
                external_id: None,
            },
            title: format!("commit {id}"),
            body: None,
            links: Vec::new(),
            entities: vec![EntityRef {
                kind: "repo".into(),
                external_id: repo.into(),
                label: None,
            }],
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: "k".into(),
                content_type: "application/x-git".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn source() -> SourceId {
        Uuid::from_u128(0x1111)
    }

    #[test]
    fn orphan_events_become_one_synthetic_commitset_per_repo_day() {
        let src = source();
        let events = vec![
            event(1, src, 9, "/repo/a"),
            event(2, src, 10, "/repo/a"),
            event(3, src, 11, "/repo/b"),
        ];
        let groups = roll_up(&events, &[], NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());

        assert_eq!(groups.len(), 2, "one group per repo-day");
        let mut repos: Vec<String> = groups
            .iter()
            .map(|g| match &g.artifact.payload {
                ArtifactPayload::CommitSet { repo_path, .. } => {
                    repo_path.to_string_lossy().to_string()
                }
            })
            .collect();
        repos.sort();
        assert_eq!(repos, vec!["/repo/a", "/repo/b"]);
    }

    #[test]
    fn real_artifacts_claim_their_events() {
        let src = source();
        let e1 = event(1, src, 9, "/repo/a");
        let e2 = event(2, src, 10, "/repo/a");

        let artifact = Artifact {
            id: ArtifactId::deterministic(&src, ArtifactKind::CommitSet, "/repo/a::2026-04-18"),
            source_id: src,
            kind: ArtifactKind::CommitSet,
            external_id: "/repo/a::2026-04-18".into(),
            payload: ArtifactPayload::CommitSet {
                repo_path: PathBuf::from("/repo/a"),
                date: NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(),
                event_ids: vec![e1.id, e2.id],
                commit_shas: vec!["sha1".into(), "sha2".into()],
            },
            created_at: Utc.with_ymd_and_hms(2026, 4, 18, 0, 0, 0).unwrap(),
        };

        let groups = roll_up(
            &[e1.clone(), e2.clone()],
            &[artifact.clone()],
            NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(),
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].artifact.id, artifact.id);
        assert_eq!(groups[0].events.len(), 2);
        assert_eq!(groups[0].events[0].id, e1.id);
        assert_eq!(groups[0].events[1].id, e2.id);
    }

    #[test]
    fn rollup_is_deterministic_across_permutations() {
        let src = source();
        let events_a = vec![
            event(3, src, 11, "/repo/b"),
            event(1, src, 9, "/repo/a"),
            event(2, src, 10, "/repo/a"),
        ];
        let events_b = vec![
            event(1, src, 9, "/repo/a"),
            event(3, src, 11, "/repo/b"),
            event(2, src, 10, "/repo/a"),
        ];
        let day = NaiveDate::from_ymd_opt(2026, 4, 18).unwrap();

        let out_a = roll_up(&events_a, &[], day);
        let out_b = roll_up(&events_b, &[], day);

        let ids_a: Vec<_> = out_a.iter().map(|g| g.artifact.id).collect();
        let ids_b: Vec<_> = out_b.iter().map(|g| g.artifact.id).collect();
        assert_eq!(ids_a, ids_b);
    }
}
