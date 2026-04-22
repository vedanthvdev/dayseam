//! `GroupKey` — the unified "which section does this event belong to"
//! abstraction shared between rollup and render.
//!
//! v0.1 only grouped commits by `repo_path`, so the primitive was
//! `repo_path_from_event(event) -> PathBuf`. v0.2 (DAY-77 / DAY-80)
//! emits Jira and Confluence events that need to group by project
//! and space respectively, and the existing repo-only primitive
//! would silently bucket every Jira event into a single `/` orphan
//! group — one section header for the whole day's Jira activity,
//! which is worse than useless.
//!
//! [`group_key_from_event`] dispatches on [`ActivityKind`] and reads
//! the canonical grouping entity off the event:
//!
//! * Repo-shaped kinds (commits, GitLab MRs, GitLab issues) → `repo`
//!   entity → [`GroupKind::Repo`].
//! * Jira kinds → `jira_project` entity → [`GroupKind::Project`].
//! * Confluence kinds → `confluence_space` entity →
//!   [`GroupKind::Space`].
//!
//! Events that are missing the canonical grouping entity (a shape
//! bug in the connector) degrade to a synthetic `/` value so the
//! render stage still lands the bullet somewhere visible rather
//! than panicking. The rollup / render paths log nothing on the
//! fallback — the upstream connector owns the observability for its
//! own shape errors.
//!
//! # Display
//!
//! [`GroupKey::display`] returns the human label when one is
//! present (Jira project `"Cardtronics"` / Confluence space
//! `"Engineering"`) and falls back to the stable `value`
//! (`"CAR"` / `"ENG"`). The rollup uses `value` for bucket keys so
//! a label churn (Jira admin renames a project mid-day) never
//! splits an issue's day into two sections.

use dayseam_core::{ActivityEvent, ActivityKind};

/// Which kind of section this event renders under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum GroupKind {
    /// A local-git or GitLab repository. Bucketed by repo path.
    Repo,
    /// A Jira project. Bucketed by project key (e.g. `"CAR"`).
    Project,
    /// A Confluence space. Bucketed by space key (e.g. `"ENG"`).
    Space,
}

/// The "which section does this event belong to" key.
///
/// `value` is the stable identifier used for rollup bucketing and
/// for deterministic sort order; `label` is the optional
/// human-friendly name used at render time only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct GroupKey {
    pub(crate) kind: GroupKind,
    pub(crate) value: String,
    pub(crate) label: Option<String>,
}

impl GroupKey {
    /// Human-facing display string: label if present, else value.
    ///
    /// Used by the render stage to build the bullet prefix. The
    /// rollup never calls this; bucket keys must stay stable under
    /// label churn.
    pub(crate) fn display(&self) -> &str {
        self.label.as_deref().unwrap_or(self.value.as_str())
    }
}

/// Compute the [`GroupKey`] for an [`ActivityEvent`].
///
/// Never panics. Returns a synthetic `/` value when the canonical
/// grouping entity is absent — the fallback matches the v0.1
/// `repo_path_from_event` behaviour so existing goldens stay green
/// under the rename.
pub(crate) fn group_key_from_event(event: &ActivityEvent) -> GroupKey {
    match event.kind {
        ActivityKind::JiraIssueTransitioned
        | ActivityKind::JiraIssueCommented
        | ActivityKind::JiraIssueAssigned
        | ActivityKind::JiraIssueUnassigned
        | ActivityKind::JiraIssueCreated => entity_group(event, "jira_project", GroupKind::Project),
        ActivityKind::ConfluencePageCreated
        | ActivityKind::ConfluencePageEdited
        | ActivityKind::ConfluenceComment => {
            entity_group(event, "confluence_space", GroupKind::Space)
        }
        // Commits, GitLab MRs / issues, anything else repo-shaped.
        // Matches v0.1's `repo_path_from_event` behaviour.
        _ => entity_group(event, "repo", GroupKind::Repo),
    }
}

fn entity_group(event: &ActivityEvent, entity_kind: &str, group_kind: GroupKind) -> GroupKey {
    event
        .entities
        .iter()
        .find(|e| e.kind == entity_kind)
        .map(|e| GroupKey {
            kind: group_kind,
            value: if e.external_id.is_empty() {
                "/".to_string()
            } else {
                e.external_id.clone()
            },
            label: e.label.clone(),
        })
        .unwrap_or(GroupKey {
            kind: group_kind,
            value: "/".to_string(),
            label: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{Actor, EntityRef, Privacy, RawRef, SourceId};
    use uuid::Uuid;

    fn event_with(kind: ActivityKind, entities: Vec<EntityRef>) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::nil(),
            source_id: SourceId::nil(),
            external_id: "x".into(),
            kind,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Self".into(),
                email: None,
                external_id: None,
            },
            title: "t".into(),
            body: None,
            links: Vec::new(),
            entities,
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: "k".into(),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn ent(kind: &str, external_id: &str, label: Option<&str>) -> EntityRef {
        EntityRef {
            kind: kind.into(),
            external_id: external_id.into(),
            label: label.map(str::to_string),
        }
    }

    #[test]
    fn commit_event_groups_by_repo() {
        let e = event_with(
            ActivityKind::CommitAuthored,
            vec![ent("repo", "/work/dayseam", None)],
        );
        let gk = group_key_from_event(&e);
        assert_eq!(gk.kind, GroupKind::Repo);
        assert_eq!(gk.value, "/work/dayseam");
    }

    #[test]
    fn jira_transitioned_groups_by_project() {
        let e = event_with(
            ActivityKind::JiraIssueTransitioned,
            vec![
                ent("jira_project", "CAR", Some("Cardtronics")),
                ent("jira_issue", "CAR-5117", None),
            ],
        );
        let gk = group_key_from_event(&e);
        assert_eq!(gk.kind, GroupKind::Project);
        assert_eq!(gk.value, "CAR");
        assert_eq!(gk.label.as_deref(), Some("Cardtronics"));
        assert_eq!(gk.display(), "Cardtronics");
    }

    #[test]
    fn confluence_groups_by_space() {
        let e = event_with(
            ActivityKind::ConfluencePageEdited,
            vec![ent("confluence_space", "ENG", Some("Engineering"))],
        );
        let gk = group_key_from_event(&e);
        assert_eq!(gk.kind, GroupKind::Space);
        assert_eq!(gk.value, "ENG");
        assert_eq!(gk.display(), "Engineering");
    }

    #[test]
    fn missing_entity_degrades_to_slash() {
        let e = event_with(ActivityKind::CommitAuthored, vec![]);
        let gk = group_key_from_event(&e);
        assert_eq!(gk.kind, GroupKind::Repo);
        assert_eq!(gk.value, "/");
        assert_eq!(gk.label, None);
    }

    #[test]
    fn display_falls_back_to_value_when_label_missing() {
        let gk = GroupKey {
            kind: GroupKind::Project,
            value: "CAR".into(),
            label: None,
        };
        assert_eq!(gk.display(), "CAR");
    }
}
