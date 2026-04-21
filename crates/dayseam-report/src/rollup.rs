//! Stage 1 of the pipeline: bundle events into artifact-shaped groups
//! before the template sees them.
//!
//! The rollup is the only place the engine walks the many-to-one
//! relationship between [`ActivityEvent`]s and [`Artifact`]s. It
//! produces [`RolledUpArtifact`] records keyed by the artifact (real
//! or synthetic) and sorted so downstream rendering is deterministic.
//!
//! Three invariants worth reading twice:
//!
//! 1. **Every event lands in exactly one group.** An event belongs to
//!    an [`Artifact`] iff that artifact's payload claims its id; an
//!    event claimed by zero artifacts lands in a *synthetic*
//!    artifact keyed by the event kind:
//!    * repo-shaped events → `Artifact::CommitSet { repo_path, date }`
//!    * Jira-shaped events → `Artifact::JiraIssue { issue_key, project_key, date }`
//!    * Confluence-shaped events → `Artifact::ConfluencePage { page_id, space_key, date }`
//!
//!    This keeps the template blind to whether the connector
//!    pre-grouped or not.
//! 2. **CommitSet groups are deduplicated by `(repo_path, date)`.**
//!    Two configured sources that happen to scan the same repo each
//!    emit their own `CommitSet` artifact. Without this merge step
//!    the report would show every commit twice. Events are unioned
//!    across the colliding groups and deduplicated by commit SHA so
//!    the count on the output side is honest. (Jira and Confluence
//!    never duplicate-collide because their synthetic artifacts key
//!    off an upstream-assigned id — issue keys and page ids are
//!    globally unique within a workspace.)
//! 3. **Sort order is total.** Groups are ordered by
//!    `(kind_token, external_id)`; events inside a group are ordered
//!    by `(occurred_at, external_id, id)`. No hash-map iteration
//!    survives into the render stage.
//!
//! The DAY-78 refactor replaced the v0.1 `repo_path_from_event`
//! primitive with [`crate::group_key::group_key_from_event`], which
//! dispatches on [`dayseam_core::ActivityKind`] so Jira and
//! Confluence events no longer silently bucket into the repo-only
//! `/` fallback.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::NaiveDate;
use dayseam_core::{ActivityEvent, Artifact, ArtifactId, ArtifactKind, ArtifactPayload, SourceId};
use uuid::Uuid;

use crate::group_key::{group_key_from_event, GroupKind};

/// One artifact's worth of events, ready to feed the template.
///
/// `artifact` is always a real [`Artifact`] — either one produced by a
/// connector or a synthetic artefact the rollup minted to hold
/// orphan events. The `events` vec is sorted and already filtered
/// (see [`roll_up`]).
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
/// * Events whose id appears in an `ArtifactPayload::*::event_ids`
///   list are attached to that artifact.
/// * Events not claimed by any artifact are grouped into synthetic
///   artifacts per `[SyntheticBucket]`. Repo events become
///   `ArtifactKind::CommitSet`; Jira events become
///   `ArtifactKind::JiraIssue`; Confluence events become
///   `ArtifactKind::ConfluencePage`.
/// * The returned vec is sorted by `(kind_token, external_id)`.
pub(crate) fn roll_up(
    events: &[ActivityEvent],
    artifacts: &[Artifact],
    report_date: NaiveDate,
) -> Vec<RolledUpArtifact> {
    let mut event_by_id: BTreeMap<Uuid, &ActivityEvent> =
        events.iter().map(|e| (e.id, e)).collect();

    let mut groups: Vec<RolledUpArtifact> = Vec::new();

    for artifact in artifacts {
        // Every `ArtifactPayload` variant carries a flat `event_ids`
        // list in its current shape; collecting them here lets the
        // rollup behave uniformly without knowing which connector
        // produced the artefact. DAY-73 added the Atlassian
        // variants; DAY-77 / DAY-80 fill them with real data.
        let claimed_ids: Vec<Uuid> = match &artifact.payload {
            ArtifactPayload::CommitSet { event_ids, .. }
            | ArtifactPayload::JiraIssue { event_ids, .. }
            | ArtifactPayload::ConfluencePage { event_ids, .. } => event_ids.clone(),
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

    let mut orphan_by_key: BTreeMap<OrphanKey, Vec<ActivityEvent>> = BTreeMap::new();
    for (_, event) in event_by_id {
        let key = orphan_key(event);
        orphan_by_key.entry(key).or_default().push(event.clone());
    }

    for (key, mut orphan_events) in orphan_by_key {
        sort_events(&mut orphan_events);
        let artifact = synthesize_artifact(&key, &orphan_events, report_date);
        groups.push(RolledUpArtifact {
            artifact,
            events: orphan_events,
        });
    }

    let groups = merge_duplicate_commit_sets(groups);
    let mut groups = groups;
    sort_groups(&mut groups);
    groups
}

/// The bucket key for an orphan event: kind + per-kind id + date,
/// scoped to the source.
///
/// For Repo events the id is the repo path; for Jira events it is
/// the issue key; for Confluence events it is the page id. Keying
/// at this level (not at the group level — project / space) means
/// each issue / page gets its own artifact with its own
/// `event_ids`, so the evidence popover in the UI can map a bullet
/// back to exactly the events that produced it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum OrphanKey {
    /// `(source_id, repo_path, date)` — v0.1 shape.
    CommitSet(SourceId, PathBuf, NaiveDate),
    /// `(source_id, issue_key, project_key, date)` — DAY-78.
    JiraIssue(SourceId, String, String, NaiveDate),
    /// `(source_id, page_id, space_key, date)` — DAY-78.
    ConfluencePage(SourceId, String, String, NaiveDate),
}

fn orphan_key(event: &ActivityEvent) -> OrphanKey {
    use dayseam_core::ActivityKind::*;
    let day = event.occurred_at.naive_local().date();
    match event.kind {
        JiraIssueTransitioned | JiraIssueCommented | JiraIssueAssigned | JiraIssueCreated => {
            let gk = group_key_from_event(event);
            // `jira_issue` entity carries the per-issue id the
            // synthetic `JiraIssue` artifact is keyed on.
            let issue_key = event
                .entities
                .iter()
                .find(|e| e.kind == "jira_issue")
                .map(|e| e.external_id.clone())
                .unwrap_or_else(|| "UNKNOWN".to_string());
            OrphanKey::JiraIssue(event.source_id, issue_key, gk.value, day)
        }
        ConfluencePageCreated | ConfluencePageEdited | ConfluenceComment => {
            let gk = group_key_from_event(event);
            let page_id = event
                .entities
                .iter()
                .find(|e| e.kind == "confluence_page")
                .map(|e| e.external_id.clone())
                .unwrap_or_else(|| "UNKNOWN".to_string());
            OrphanKey::ConfluencePage(event.source_id, page_id, gk.value, day)
        }
        _ => {
            let repo_path = PathBuf::from(group_key_from_event(event).value);
            OrphanKey::CommitSet(event.source_id, repo_path, day)
        }
    }
}

fn synthesize_artifact(
    key: &OrphanKey,
    events: &[ActivityEvent],
    report_date: NaiveDate,
) -> Artifact {
    let event_ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
    // Synthetic artifacts never reach disk; the report draft only
    // cares that this timestamp is deterministic. Using
    // `report_date` at midnight UTC keeps a fixed point on the day
    // in question without reaching for a clock.
    let created_at = report_date
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc();

    match key {
        OrphanKey::CommitSet(source_id, repo_path, day) => {
            let external_id = synthetic_commit_set_external_id(repo_path, *day);
            let id = ArtifactId::deterministic(source_id, ArtifactKind::CommitSet, &external_id);
            let commit_shas: Vec<String> = events.iter().map(|e| e.external_id.clone()).collect();
            Artifact {
                id,
                source_id: *source_id,
                kind: ArtifactKind::CommitSet,
                external_id,
                payload: ArtifactPayload::CommitSet {
                    repo_path: repo_path.clone(),
                    date: *day,
                    event_ids,
                    commit_shas,
                },
                created_at,
            }
        }
        OrphanKey::JiraIssue(source_id, issue_key, project_key, day) => {
            let external_id = format!("{issue_key}::{day}");
            let id = ArtifactId::deterministic(source_id, ArtifactKind::JiraIssue, &external_id);
            Artifact {
                id,
                source_id: *source_id,
                kind: ArtifactKind::JiraIssue,
                external_id,
                payload: ArtifactPayload::JiraIssue {
                    issue_key: issue_key.clone(),
                    project_key: project_key.clone(),
                    date: *day,
                    event_ids,
                },
                created_at,
            }
        }
        OrphanKey::ConfluencePage(source_id, page_id, space_key, day) => {
            let external_id = format!("{page_id}::{day}");
            let id =
                ArtifactId::deterministic(source_id, ArtifactKind::ConfluencePage, &external_id);
            Artifact {
                id,
                source_id: *source_id,
                kind: ArtifactKind::ConfluencePage,
                external_id,
                payload: ArtifactPayload::ConfluencePage {
                    page_id: page_id.clone(),
                    space_key: space_key.clone(),
                    date: *day,
                    event_ids,
                },
                created_at,
            }
        }
    }
}

/// Merge `CommitSet` groups that share a `(repo_path, date)` key.
///
/// See the DAY-52 rationale on the original primitive; the DAY-78
/// refactor keeps the same guarantee and adds a passthrough for
/// `JiraIssue` and `ConfluencePage` groups, whose synthetic keys are
/// already globally unique (`issue_key` / `page_id`) so no merging
/// is needed.
fn merge_duplicate_commit_sets(groups: Vec<RolledUpArtifact>) -> Vec<RolledUpArtifact> {
    use std::collections::HashSet;

    let mut by_key: BTreeMap<(PathBuf, NaiveDate), RolledUpArtifact> = BTreeMap::new();
    // Preserve first-seen order so the final sort has a
    // deterministic tie-breaker when two merged groups sort equal.
    let mut order: Vec<(PathBuf, NaiveDate)> = Vec::with_capacity(groups.len());

    for group in groups {
        let key = commit_set_key(&group);
        match key {
            Some(k) => {
                if let Some(existing) = by_key.get_mut(&k) {
                    let mut seen: HashSet<String> = existing
                        .events
                        .iter()
                        .map(|e| e.external_id.clone())
                        .collect();
                    for event in group.events {
                        if seen.insert(event.external_id.clone()) {
                            existing.events.push(event);
                        }
                    }
                    sort_events(&mut existing.events);
                } else {
                    order.push(k.clone());
                    by_key.insert(k, group);
                }
            }
            None => {
                // Non-CommitSet kinds (JiraIssue / ConfluencePage)
                // pass through untouched. The order vec is keyed by
                // `(repo_path, date)` so we use a sentinel key.
                // Sentinels collide only if two groups share the
                // same artifact id, which implies the same source +
                // kind + external_id — a true duplicate that the
                // final sort-and-dedup should flatten anyway.
                let sentinel = (
                    PathBuf::from(format!("__non_commit_set__::{}", group.artifact.id)),
                    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default(),
                );
                order.push(sentinel.clone());
                by_key.insert(sentinel, group);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

fn commit_set_key(group: &RolledUpArtifact) -> Option<(PathBuf, NaiveDate)> {
    match &group.artifact.payload {
        ArtifactPayload::CommitSet {
            repo_path, date, ..
        } => Some((repo_path.clone(), *date)),
        ArtifactPayload::JiraIssue { .. } | ArtifactPayload::ConfluencePage { .. } => None,
    }
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
        ArtifactKind::JiraIssue => "JiraIssue",
        ArtifactKind::ConfluencePage => "ConfluencePage",
    }
}

fn synthetic_commit_set_external_id(repo_path: &std::path::Path, day: NaiveDate) -> String {
    format!("{}::{}::synthetic", repo_path.display(), day)
}

/// Returns the render-stage group kind for a rolled-up artifact.
///
/// This is the inverse of the [`orphan_key`] dispatch — the rollup
/// already chose which kind to synthesise from the event stream; the
/// renderer just needs to know whether to prefix `**<repo>** —` vs
/// `**<project_name>** (<project_key>) —` vs
/// `**<space_name>** (<space_key>) —`.
pub(crate) fn group_kind_for_payload(payload: &ArtifactPayload) -> GroupKind {
    match payload {
        ArtifactPayload::CommitSet { .. } => GroupKind::Repo,
        ArtifactPayload::JiraIssue { .. } => GroupKind::Project,
        ArtifactPayload::ConfluencePage { .. } => GroupKind::Space,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{
        ActivityKind, Actor, ArtifactKind, ArtifactPayload, EntityRef, Privacy, RawRef, SourceId,
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

    fn jira_event(id: u128, source: SourceId, issue_key: &str, project_key: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::from_u128(id),
            source_id: source,
            external_id: format!("{issue_key}::transition::{id}"),
            kind: ActivityKind::JiraIssueTransitioned,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 18, 10, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Self".into(),
                email: None,
                external_id: Some("acct-1".into()),
            },
            title: format!("{issue_key}: Status transition"),
            body: None,
            links: Vec::new(),
            entities: vec![
                EntityRef {
                    kind: "jira_project".into(),
                    external_id: project_key.into(),
                    label: Some(format!("{project_key} Project")),
                },
                EntityRef {
                    kind: "jira_issue".into(),
                    external_id: issue_key.into(),
                    label: None,
                },
            ],
            parent_external_id: Some(issue_key.into()),
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: format!("jira:{issue_key}"),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
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
                ArtifactPayload::JiraIssue { .. } | ArtifactPayload::ConfluencePage { .. } => {
                    unreachable!("this test only produces CommitSet artefacts")
                }
            })
            .collect();
        repos.sort();
        assert_eq!(repos, vec!["/repo/a", "/repo/b"]);
    }

    /// DAY-78: Jira orphans bucket by issue key (not project key) so
    /// each issue carries its own evidence-link artefact. Two issues
    /// in the same project → two synthetic `JiraIssue` artefacts.
    #[test]
    fn jira_orphans_bucket_by_issue_key() {
        let src = source();
        let events = vec![
            jira_event(10, src, "CAR-5117", "CAR"),
            jira_event(11, src, "CAR-5117", "CAR"),
            jira_event(12, src, "CAR-6001", "CAR"),
            jira_event(13, src, "KTON-4550", "KTON"),
        ];
        let groups = roll_up(&events, &[], NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());

        assert_eq!(groups.len(), 3, "one synthetic JiraIssue per issue key");
        let issues: Vec<String> = groups
            .iter()
            .map(|g| match &g.artifact.payload {
                ArtifactPayload::JiraIssue { issue_key, .. } => issue_key.clone(),
                _ => unreachable!(),
            })
            .collect();
        // Groups sort by external_id, which is `<issue>::<date>`.
        assert_eq!(issues, vec!["CAR-5117", "CAR-6001", "KTON-4550"]);

        // The two events on CAR-5117 both landed in the same group.
        let car_5117 = &groups[0];
        assert_eq!(car_5117.events.len(), 2);
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

    /// DAY-52 regression: two configured sources scanning the same
    /// repository each produce their own `CommitSet` artifact for
    /// the same day. The rollup merges them by `(repo_path, date)`
    /// so the downstream render sees one group with every unique
    /// commit, not two groups with the same commits.
    #[test]
    fn duplicate_commit_sets_are_merged_across_sources() {
        let src_a = Uuid::from_u128(0x2222);
        let src_b = Uuid::from_u128(0x3333);

        let e_a = event(1, src_a, 9, "/work/dayseam");
        let e_b = event(1, src_b, 9, "/work/dayseam");
        let e_a_only = event(2, src_a, 10, "/work/dayseam");

        let day = NaiveDate::from_ymd_opt(2026, 4, 18).unwrap();
        let external_id = format!("/work/dayseam::{day}");
        let art_a = Artifact {
            id: ArtifactId::deterministic(&src_a, ArtifactKind::CommitSet, &external_id),
            source_id: src_a,
            kind: ArtifactKind::CommitSet,
            external_id: external_id.clone(),
            payload: ArtifactPayload::CommitSet {
                repo_path: PathBuf::from("/work/dayseam"),
                date: day,
                event_ids: vec![e_a.id, e_a_only.id],
                commit_shas: vec![e_a.external_id.clone(), e_a_only.external_id.clone()],
            },
            created_at: Utc.with_ymd_and_hms(2026, 4, 18, 0, 0, 0).unwrap(),
        };
        let art_b = Artifact {
            id: ArtifactId::deterministic(&src_b, ArtifactKind::CommitSet, &external_id),
            source_id: src_b,
            kind: ArtifactKind::CommitSet,
            external_id,
            payload: ArtifactPayload::CommitSet {
                repo_path: PathBuf::from("/work/dayseam"),
                date: day,
                event_ids: vec![e_b.id],
                commit_shas: vec![e_b.external_id.clone()],
            },
            created_at: Utc.with_ymd_and_hms(2026, 4, 18, 0, 0, 0).unwrap(),
        };

        let groups = roll_up(
            &[e_a.clone(), e_a_only.clone(), e_b],
            &[art_a.clone(), art_b],
            day,
        );

        assert_eq!(
            groups.len(),
            1,
            "two sources sharing a repo should merge into one group"
        );
        let shas: Vec<&str> = groups[0]
            .events
            .iter()
            .map(|e| e.external_id.as_str())
            .collect();
        assert_eq!(
            shas,
            vec!["sha1", "sha2"],
            "events unioned and deduplicated by SHA, sorted by (occurred_at, id)"
        );
        assert_eq!(
            groups[0].artifact.id, art_a.id,
            "first-seen group wins the artifact id so bullet_id stays stable"
        );
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
