//! The pure-function preprocessing pipeline that runs between the
//! orchestrator's fan-out and the report engine's [`crate::render`].
//!
//! # Why one function?
//!
//! Every caller — the orchestrator's `generate_report`, the future
//! CLI, the test harnesses — needs the same four passes in the same
//! order:
//!
//! 1. **Dedup** cross-source `CommitAuthored` collisions. A push +
//!    local commit for the same SHA must not render twice.
//! 2. **Extract ticket keys.** Scan titles + bodies for
//!    `[A-Z]{2,10}-\d+` and attach `jira_issue` [`EntityRef`]s as
//!    targets. Runs on deduped events so we only scan each SHA
//!    once.
//! 3. **Annotate Jira transitions with MRs.** Now that MRs carry
//!    `jira_issue` targets, we can link each Jira transition to its
//!    triggering MR. Runs after extraction so the index it builds
//!    includes every MR.
//! 4. **Annotate rolled-into-MR.** Set `parent_external_id` on
//!    `CommitAuthored` events whose SHA belongs to an MR's commit
//!    list. DAY-72 PERF-addendum-06 index, unchanged.
//!
//! Downstream rollup sees the enriched events and groups them into
//! per-issue / per-repo artefacts correctly on the first pass.
//!
//! # Determinism
//!
//! Every pass is a pure function of its input, and their
//! composition is too. Running [`pipeline`] twice on the same input
//! produces the same output (the individual `is_idempotent` tests
//! combine to prove this; the pipeline-level test asserts the
//! combined guarantee).

use dayseam_core::ActivityEvent;

use crate::dedup::dedup_commit_authored;
use crate::enrich::{annotate_transition_with_mr, extract_ticket_keys};
use crate::rollup_mr::{annotate_rolled_into_mr, MergeRequestArtifact};

/// Run dedup → enrich → annotate-into-MR on a day's events.
///
/// Consumes the input `events` by value (the orchestrator already
/// owns the vec; copying would be wasted work) and returns the
/// enriched stream. `mrs` is the MR index the caller gets from
/// [`crate::rollup_mr::MergeRequestArtifact`]; passing an empty
/// slice is fine and skips the rolled-into-MR pass.
///
/// Note that [`annotate_transition_with_mr`] keys off MR events
/// inside `events` (not `mrs`) because it follows the
/// extract-then-link pattern: MRs in `events` carry `jira_issue`
/// targets after [`extract_ticket_keys`] runs, and that's the
/// index the transition annotator needs.
#[must_use]
pub fn pipeline(events: Vec<ActivityEvent>, mrs: &[MergeRequestArtifact]) -> Vec<ActivityEvent> {
    let mut events = dedup_commit_authored(events);
    extract_ticket_keys(&mut events);
    annotate_transition_with_mr(&mut events);
    annotate_rolled_into_mr(&mut events, mrs);
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{
        ActivityEvent, ActivityKind, Actor, EntityKind, EntityRef, Privacy, RawRef, SourceId,
    };
    use uuid::Uuid;

    fn src() -> SourceId {
        Uuid::from_u128(0x1111)
    }

    fn commit(sha: &str, title: &str) -> ActivityEvent {
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
            title: title.into(),
            body: None,
            links: Vec::new(),
            entities: vec![EntityRef {
                kind: EntityKind::Repo,
                external_id: "/work/dayseam".into(),
                label: None,
            }],
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: format!("k:{sha}"),
                content_type: "application/x-git-commit".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn mr_opened(iid: &str, title: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::new_v5(&Uuid::NAMESPACE_OID, iid.as_bytes()),
            source_id: src(),
            external_id: iid.into(),
            kind: ActivityKind::MrOpened,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Self".into(),
                email: None,
                external_id: Some("17".into()),
            },
            title: title.into(),
            body: None,
            links: Vec::new(),
            entities: Vec::new(),
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: format!("mr:{iid}"),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn jira_transition(issue_key: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::new_v5(&Uuid::NAMESPACE_OID, issue_key.as_bytes()),
            source_id: src(),
            external_id: format!("{issue_key}::transition"),
            kind: ActivityKind::JiraIssueTransitioned,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 11, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Self".into(),
                email: None,
                external_id: Some("acct-1".into()),
            },
            title: format!("{issue_key}: In Progress → Done"),
            body: None,
            links: Vec::new(),
            entities: vec![
                EntityRef {
                    kind: EntityKind::JiraProject,
                    external_id: "CAR".into(),
                    label: Some("Cardtronics".into()),
                },
                EntityRef {
                    kind: EntityKind::JiraIssue,
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

    /// Plan invariant 9: dedup → enrich → annotate all compose into
    /// a single stable output.
    #[test]
    fn pipeline_runs_dedup_enrich_rollup_in_order() {
        let c_a = commit("sha1", "CAR-5117: trim JSON");
        let c_b = {
            let mut c = commit("sha1", "CAR-5117: trim JSON");
            c.source_id = Uuid::from_u128(0x2222);
            c
        };
        let mr = mr_opened("!321", "CAR-5117: Rename commands");
        let transition = jira_transition("CAR-5117");

        let events = vec![c_a, c_b, mr, transition];
        let mrs = vec![MergeRequestArtifact {
            external_id: "!321".into(),
            commit_shas: vec!["sha1".into()],
        }];
        let out = pipeline(events, &mrs);

        // 1. Dedup collapsed the duplicate sha1 commits.
        let commit_rows: Vec<&ActivityEvent> = out
            .iter()
            .filter(|e| e.kind == ActivityKind::CommitAuthored)
            .collect();
        assert_eq!(commit_rows.len(), 1, "dedup merged sha1 rows");

        // 2. Extract ticket keys attached a jira_issue target to
        //    the surviving commit + the MR.
        for e in &out {
            if matches!(
                e.kind,
                ActivityKind::CommitAuthored | ActivityKind::MrOpened
            ) {
                assert!(
                    e.entities
                        .iter()
                        .any(|ent| ent.kind == EntityKind::JiraIssue
                            && ent.external_id == "CAR-5117"),
                    "extract_ticket_keys missed {:?}",
                    e.kind
                );
            }
        }

        // 3. annotate_transition_with_mr stamped the MR iid on the
        //    transition.
        let transition = out
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(transition.parent_external_id.as_deref(), Some("!321"));

        // 4. annotate_rolled_into_mr stamped the MR iid on the
        //    deduped commit.
        let commit = commit_rows[0];
        assert_eq!(commit.parent_external_id.as_deref(), Some("!321"));
    }

    #[test]
    fn pipeline_is_idempotent() {
        let c = commit("sha1", "CAR-5117: trim JSON");
        let mr = mr_opened("!321", "CAR-5117: Rename commands");
        let transition = jira_transition("CAR-5117");

        let events = vec![c, mr, transition];
        let mrs = vec![MergeRequestArtifact {
            external_id: "!321".into(),
            commit_shas: vec!["sha1".into()],
        }];
        let first = pipeline(events.clone(), &mrs);
        let second = pipeline(first.clone(), &mrs);
        assert_eq!(second, first, "pipeline is a pure function");
    }
}
