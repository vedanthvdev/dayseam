//! Jira JQL issue → [`dayseam_core::ActivityEvent`] mapping.
//!
//! The walker pulls one issue (with an expanded `changelog`) per row
//! and hands it here. This module walks the `fields` + `changelog` +
//! `fields.comment.comments` shape and emits zero-or-more
//! [`ActivityEvent`]s per issue — one arm per v0.2 `ActivityKind`
//! variant the plan reserves:
//!
//! | Arm | Source | Kind | Filter |
//! |---|---|---|---|
//! | status change | `changelog.histories[].items[] where field == "status"` | `JiraIssueTransitioned` | `author.accountId == self` |
//! | assignee change | `changelog.histories[].items[] where field == "assignee"` | `JiraIssueAssigned` | `items[].to == self.accountId` |
//! | comment | `fields.comment.comments[]` | `JiraIssueCommented` | `author.accountId == self` |
//! | issue created | issue itself | `JiraIssueCreated` | `fields.reporter.accountId == self AND fields.created in window` |
//!
//! Every emitted event carries `occurred_at` in UTC and is day-window
//! filtered by the walker. A changelog item whose `field` we do not
//! recognise is silently dropped with a `LogLevel::Debug` — custom
//! fields (`cf[10019]`, etc.) are legion and none of them belong in
//! the v0.2 event vocabulary.

use chrono::{DateTime, Utc};
use connector_atlassian_common::{adf_to_plain, AtlassianError, Product};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, DayseamError, EntityRef, Link, LogLevel, Privacy, RawRef,
    SourceId,
};
use dayseam_events::LogSender;
use serde_json::{json, Value};
use url::Url;

use crate::rollup::{collapse_rapid_transitions, CollapsedTransition, StatusTransition};

/// The set of events produced from one JQL issue row, plus a small
/// count of per-issue drops the walker aggregates into its `dropped_by_shape`
/// counter.
#[derive(Debug, Default)]
pub struct IssueEvents {
    pub events: Vec<ActivityEvent>,
    pub dropped_unknown_changelog: u64,
}

/// Window bounds (UTC). Anything outside `[start, end)` is dropped.
#[derive(Debug, Clone, Copy)]
pub struct DayWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl DayWindow {
    fn contains(&self, ts: DateTime<Utc>) -> bool {
        ts >= self.start && ts < self.end
    }
}

/// Normalise one JQL-returned issue into zero-or-more activity events.
///
/// * `source_id` scopes the deterministic-id namespace.
/// * `workspace_url` is the canonical Cloud URL (with trailing slash)
///   used to compose browser deep-links.
/// * `self_account_id` is the Atlassian `accountId` we filter for.
/// * `window` is the UTC day-window the walker is fetching.
/// * `logs` receives debug/warn events — debug for skipped custom-field
///   changelogs, warn for shape surprises that still survive (e.g. a
///   comment missing its body).
///
/// Returns `Err(DayseamError::UpstreamChanged { code: jira.walk.upstream_shape_changed, … })`
/// if a mandatory field is missing from the issue envelope.
pub fn normalise_issue(
    source_id: SourceId,
    workspace_url: &Url,
    self_account_id: &str,
    window: DayWindow,
    issue: &Value,
    logs: Option<&LogSender>,
) -> Result<IssueEvents, DayseamError> {
    let key = issue
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| shape_error("issue.key missing"))?;
    let fields = issue
        .get("fields")
        .ok_or_else(|| shape_error("issue.fields missing"))?;

    let issue_summary = fields
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let project = fields
        .get("project")
        .ok_or_else(|| shape_error("issue.fields.project missing"))?;
    let project_key = project
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| shape_error("issue.fields.project.key missing"))?
        .to_string();
    let project_name = project
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);

    let link = issue_browse_link(workspace_url, key);

    let mut out = IssueEvents::default();

    // ---- Created -----------------------------------------------------
    if let Some(reporter) = fields.get("reporter") {
        let is_self = reporter
            .get("accountId")
            .and_then(Value::as_str)
            .is_some_and(|id| id == self_account_id);
        if is_self {
            if let Some(created_raw) = fields.get("created").and_then(Value::as_str) {
                if let Some(created_at) = parse_jira_datetime(created_raw) {
                    if window.contains(created_at) {
                        out.events.push(build_created_event(
                            source_id,
                            &project_key,
                            project_name.as_deref(),
                            key,
                            &issue_summary,
                            &link,
                            reporter,
                            created_at,
                        ));
                    }
                }
            }
        }
    }

    // ---- Changelog: transitions + assignments ------------------------
    let mut self_status_transitions: Vec<StatusTransition> = Vec::new();
    if let Some(histories) = issue
        .get("changelog")
        .and_then(|c| c.get("histories"))
        .and_then(Value::as_array)
    {
        for history in histories {
            let Some(author) = history.get("author") else {
                continue;
            };
            let Some(author_account) = author.get("accountId").and_then(Value::as_str) else {
                continue;
            };
            if author_account != self_account_id {
                continue;
            }
            let Some(created_raw) = history.get("created").and_then(Value::as_str) else {
                continue;
            };
            let Some(created_at) = parse_jira_datetime(created_raw) else {
                continue;
            };
            if !window.contains(created_at) {
                continue;
            }
            let Some(items) = history.get("items").and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                let field_name = item.get("field").and_then(Value::as_str).unwrap_or("");
                match field_name {
                    "status" => {
                        let to_status = item
                            .get("toString")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let from_status = item
                            .get("fromString")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let status_category = fields
                            .get("status")
                            .and_then(|s| s.get("statusCategory"))
                            .and_then(|c| c.get("key"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_default();
                        self_status_transitions.push(StatusTransition {
                            created_at,
                            from_status,
                            to_status,
                            status_category,
                        });
                    }
                    "assignee" => {
                        // The Jira changelog identifies assignees by
                        // `to` (the accountId) rather than `toString`
                        // (the displayName) — displayName is a
                        // rendering concern the upstream may not even
                        // populate consistently.
                        let to_account = item.get("to").and_then(Value::as_str).unwrap_or("");
                        let from_account = item.get("from").and_then(Value::as_str).unwrap_or("");
                        if to_account == self_account_id {
                            // Pre-DAY-88 semantics: emit only when the
                            // user was newly assigned the ticket.
                            out.events.push(build_assigned_event(
                                source_id,
                                &project_key,
                                project_name.as_deref(),
                                key,
                                &issue_summary,
                                &link,
                                author,
                                created_at,
                            ));
                        } else if from_account == self_account_id {
                            // DAY-88 / CORR-v0.2-07. The user was
                            // unassigned. This covers both true
                            // unassignments (`to == ""`) and handoffs
                            // to a different teammate (`to == other`).
                            // Symmetric with the assigned arm above:
                            // "I handed off CAR-5117" is as much a
                            // calendar event for the user's EOD as
                            // "I picked up CAR-5117".
                            out.events.push(build_unassigned_event(
                                source_id,
                                &project_key,
                                project_name.as_deref(),
                                key,
                                &issue_summary,
                                &link,
                                author,
                                created_at,
                                to_account,
                            ));
                        }
                    }
                    _ => {
                        // Custom fields (`cf[10019]`) and shape
                        // additions drop silently — the debug log
                        // keeps the observability surface intact
                        // without failing the walk.
                        out.dropped_unknown_changelog =
                            out.dropped_unknown_changelog.saturating_add(1);
                        if let Some(tx) = logs {
                            tx.send(
                                LogLevel::Debug,
                                None,
                                "jira: ignoring unknown changelog item field".to_string(),
                                json!({
                                    "issue_key": key,
                                    "field": field_name,
                                }),
                            );
                        }
                    }
                }
            }
        }
    }

    // Collapse rapid cascades before emitting.
    self_status_transitions.sort_by_key(|t| t.created_at);
    let collapsed = collapse_rapid_transitions(&self_status_transitions);
    for transition in collapsed {
        out.events.push(build_transition_event(
            source_id,
            &project_key,
            project_name.as_deref(),
            key,
            &issue_summary,
            &link,
            self_account_id,
            &transition,
        ));
    }

    // ---- Comments ----------------------------------------------------
    if let Some(comments) = fields
        .get("comment")
        .and_then(|c| c.get("comments"))
        .and_then(Value::as_array)
    {
        for comment in comments {
            let Some(author) = comment.get("author") else {
                continue;
            };
            let Some(author_account) = author.get("accountId").and_then(Value::as_str) else {
                continue;
            };
            if author_account != self_account_id {
                continue;
            }
            let Some(created_raw) = comment.get("created").and_then(Value::as_str) else {
                continue;
            };
            let Some(created_at) = parse_jira_datetime(created_raw) else {
                continue;
            };
            if !window.contains(created_at) {
                continue;
            }
            let comment_id = comment
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let body_adf = comment.get("body").cloned().unwrap_or(Value::Null);
            let body_plain = adf_to_plain(&body_adf, logs);
            out.events.push(build_commented_event(
                source_id,
                &project_key,
                project_name.as_deref(),
                key,
                &issue_summary,
                &link,
                author,
                created_at,
                &comment_id,
                body_plain,
            ));
        }
    }

    Ok(out)
}

fn shape_error(message: impl Into<String>) -> DayseamError {
    AtlassianError::WalkShapeChanged {
        product: Product::Jira,
        message: message.into(),
    }
    .into()
}

fn parse_jira_datetime(raw: &str) -> Option<DateTime<Utc>> {
    // Jira uses `2026-04-20T15:30:00.000+0000` (no colon in the
    // offset). `DateTime::parse_from_rfc3339` tolerates that shape
    // via `%z`; we fall through to `%Y-%m-%dT%H:%M:%S%.3f%z` just in
    // case.
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .or_else(|| DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.3f%z").ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn issue_browse_link(workspace_url: &Url, key: &str) -> Link {
    let url = workspace_url
        .join(&format!("browse/{key}"))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("{workspace_url}browse/{key}"));
    Link {
        url,
        label: Some(key.to_string()),
    }
}

fn project_entity(project_key: &str, project_name: Option<&str>) -> EntityRef {
    EntityRef {
        kind: "jira_project".to_string(),
        external_id: project_key.to_string(),
        label: project_name.map(str::to_string),
    }
}

fn issue_entity(issue_key: &str, summary: &str) -> EntityRef {
    EntityRef {
        kind: "jira_issue".to_string(),
        external_id: issue_key.to_string(),
        label: if summary.is_empty() {
            None
        } else {
            Some(summary.to_string())
        },
    }
}

fn actor_from_author(author: &Value) -> Actor {
    Actor {
        display_name: author
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        email: author
            .get("emailAddress")
            .and_then(Value::as_str)
            .map(str::to_string),
        external_id: author
            .get("accountId")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_transition_event(
    source_id: SourceId,
    project_key: &str,
    project_name: Option<&str>,
    issue_key: &str,
    summary: &str,
    link: &Link,
    self_account_id: &str,
    t: &CollapsedTransition,
) -> ActivityEvent {
    let kind_token = "JiraIssueTransitioned";
    let external_id = format!("{issue_key}:transition:{}", t.created_at.timestamp_millis());
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);
    let title = if t.transition_count > 1 {
        format!(
            "{issue_key} {} → {} (rolled up from {} transitions)",
            t.from_status, t.to_status, t.transition_count
        )
    } else {
        format!("{issue_key} {} → {}", t.from_status, t.to_status)
    };
    ActivityEvent {
        id,
        source_id,
        external_id,
        kind: ActivityKind::JiraIssueTransitioned,
        occurred_at: t.created_at,
        actor: Actor {
            display_name: "".into(),
            email: None,
            external_id: Some(self_account_id.to_string()),
        },
        title,
        body: None,
        links: vec![link.clone()],
        entities: vec![
            project_entity(project_key, project_name),
            issue_entity(issue_key, summary),
        ],
        parent_external_id: Some(issue_key.to_string()),
        metadata: json!({
            "from_status": t.from_status,
            "to_status": t.to_status,
            "status_category": t.status_category,
            "transition_count": t.transition_count,
            // DAY-88 / CORR-v0.2-04. Intermediate hops a cascade passed
            // through, in chronological order. Empty for a non-collapsed
            // singleton; renderers can ignore the field.
            "via": t.via,
        }),
        raw_ref: RawRef {
            storage_key: format!("jira:issue:{issue_key}:transition:{}", t.created_at),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_assigned_event(
    source_id: SourceId,
    project_key: &str,
    project_name: Option<&str>,
    issue_key: &str,
    summary: &str,
    link: &Link,
    author: &Value,
    created_at: DateTime<Utc>,
) -> ActivityEvent {
    let kind_token = "JiraIssueAssigned";
    let external_id = format!("{issue_key}:assigned:{}", created_at.timestamp_millis());
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);
    ActivityEvent {
        id,
        source_id,
        external_id,
        kind: ActivityKind::JiraIssueAssigned,
        occurred_at: created_at,
        actor: actor_from_author(author),
        title: format!("{issue_key} assigned"),
        body: None,
        links: vec![link.clone()],
        entities: vec![
            project_entity(project_key, project_name),
            issue_entity(issue_key, summary),
        ],
        parent_external_id: Some(issue_key.to_string()),
        metadata: json!({}),
        raw_ref: RawRef {
            storage_key: format!("jira:issue:{issue_key}:assigned:{created_at}"),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

/// DAY-88 / CORR-v0.2-07. Symmetric counterpart to
/// [`build_assigned_event`] for the `from == self` case.
///
/// `to_account` is the accountId the ticket moved to — empty string
/// for a true unassignment, a different accountId for a handoff. It
/// is recorded on `metadata.reassigned_to_account_id` when non-empty
/// so a future renderer can distinguish the two cases without
/// reparsing the raw payload.
#[allow(clippy::too_many_arguments)]
fn build_unassigned_event(
    source_id: SourceId,
    project_key: &str,
    project_name: Option<&str>,
    issue_key: &str,
    summary: &str,
    link: &Link,
    author: &Value,
    created_at: DateTime<Utc>,
    to_account: &str,
) -> ActivityEvent {
    let kind_token = "JiraIssueUnassigned";
    let external_id = format!("{issue_key}:unassigned:{}", created_at.timestamp_millis());
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);
    let metadata = if to_account.is_empty() {
        json!({})
    } else {
        json!({ "reassigned_to_account_id": to_account })
    };
    ActivityEvent {
        id,
        source_id,
        external_id,
        kind: ActivityKind::JiraIssueUnassigned,
        occurred_at: created_at,
        actor: actor_from_author(author),
        title: format!("{issue_key} unassigned"),
        body: None,
        links: vec![link.clone()],
        entities: vec![
            project_entity(project_key, project_name),
            issue_entity(issue_key, summary),
        ],
        parent_external_id: Some(issue_key.to_string()),
        metadata,
        raw_ref: RawRef {
            storage_key: format!("jira:issue:{issue_key}:unassigned:{created_at}"),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_created_event(
    source_id: SourceId,
    project_key: &str,
    project_name: Option<&str>,
    issue_key: &str,
    summary: &str,
    link: &Link,
    reporter: &Value,
    created_at: DateTime<Utc>,
) -> ActivityEvent {
    let kind_token = "JiraIssueCreated";
    let external_id = format!("{issue_key}:created");
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);
    ActivityEvent {
        id,
        source_id,
        external_id,
        kind: ActivityKind::JiraIssueCreated,
        occurred_at: created_at,
        actor: actor_from_author(reporter),
        title: if summary.is_empty() {
            format!("{issue_key} created")
        } else {
            format!("{issue_key}: {summary}")
        },
        body: None,
        links: vec![link.clone()],
        entities: vec![
            project_entity(project_key, project_name),
            issue_entity(issue_key, summary),
        ],
        parent_external_id: Some(issue_key.to_string()),
        metadata: json!({}),
        raw_ref: RawRef {
            storage_key: format!("jira:issue:{issue_key}:created"),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_commented_event(
    source_id: SourceId,
    project_key: &str,
    project_name: Option<&str>,
    issue_key: &str,
    summary: &str,
    link: &Link,
    author: &Value,
    created_at: DateTime<Utc>,
    comment_id: &str,
    body_plain: String,
) -> ActivityEvent {
    let kind_token = "JiraIssueCommented";
    let external_id = format!("{issue_key}:comment:{comment_id}");
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);
    ActivityEvent {
        id,
        source_id,
        external_id,
        kind: ActivityKind::JiraIssueCommented,
        occurred_at: created_at,
        actor: actor_from_author(author),
        title: format!("{issue_key} comment"),
        body: if body_plain.is_empty() {
            None
        } else {
            Some(body_plain)
        },
        links: vec![link.clone()],
        entities: vec![
            project_entity(project_key, project_name),
            issue_entity(issue_key, summary),
        ],
        parent_external_id: Some(issue_key.to_string()),
        metadata: json!({
            "comment_id": comment_id,
        }),
        raw_ref: RawRef {
            storage_key: format!("jira:issue:{issue_key}:comment:{comment_id}"),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

// Keep the shape-change helper public to the crate so walk.rs can
// also reach for it when it detects a top-level envelope surprise
// (e.g. the `issues` array missing entirely).
pub(crate) fn shape_changed(message: impl Into<String>) -> DayseamError {
    shape_error(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use dayseam_core::error_codes;
    use uuid::Uuid;

    const SELF_ID: &str = "5d53f3cbc6b9320d9ea5bdc2";
    const SHAPE_CHANGED_CODE: &str = error_codes::JIRA_WALK_UPSTREAM_SHAPE_CHANGED;

    fn workspace() -> Url {
        Url::parse("https://acme.atlassian.net/").unwrap()
    }

    fn window() -> DayWindow {
        DayWindow {
            start: Utc.with_ymd_and_hms(2026, 4, 20, 0, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).unwrap(),
        }
    }

    fn issue_skeleton() -> Value {
        json!({
            "id": "10001",
            "key": "CAR-5117",
            "fields": {
                "summary": "Fix review findings",
                "status": {
                    "name": "In Progress",
                    "statusCategory": {"key": "indeterminate", "name": "In Progress"}
                },
                "project": {"id": "10", "key": "CAR", "name": "Car"},
                "priority": {"name": "Medium"},
                "labels": [],
                "updated": "2026-04-20T10:00:00.000+0000"
            }
        })
    }

    #[test]
    fn status_transition_by_self_emits_one_transition_event() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "status", "fromString": "To Do", "toString": "In Progress",
                         "from": "1", "to": "3"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        let ev = &out.events[0];
        assert_eq!(ev.kind, ActivityKind::JiraIssueTransitioned);
        assert!(ev.title.contains("CAR-5117"));
        assert!(ev.title.contains("To Do"));
        assert!(ev.title.contains("In Progress"));
        assert_eq!(ev.metadata["transition_count"], json!(1));
    }

    #[test]
    fn transition_authored_by_other_user_is_dropped() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": "other-user", "displayName": "Other"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "status", "fromString": "To Do", "toString": "In Progress"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert!(out.events.is_empty());
    }

    #[test]
    fn transition_outside_window_is_dropped() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-21T10:00:00.000+0000",
                    "items": [
                        {"field": "status", "fromString": "To Do", "toString": "In Progress"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert!(out.events.is_empty());
    }

    #[test]
    fn unknown_changelog_field_increments_dropped_counter_and_emits_nothing() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "cf[10019]", "fromString": null, "toString": "whatever"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert!(out.events.is_empty());
        assert_eq!(out.dropped_unknown_changelog, 1);
    }

    #[test]
    fn self_assignee_change_emits_jira_issue_assigned() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "assignee", "to": SELF_ID, "toString": "Me"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].kind, ActivityKind::JiraIssueAssigned);
    }

    #[test]
    fn assignee_change_to_someone_else_does_not_emit() {
        // Unrelated assignment — neither `from` nor `to` is self — must
        // not emit. CORR-v0.2-07 only adds a `from == self` arm; it
        // must not introduce false positives when the user is neither
        // the losing assignee nor the gaining one.
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "assignee",
                         "from": "someone-else", "fromString": "Them",
                         "to": "other-user", "toString": "Other"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert!(out.events.is_empty());
    }

    /// DAY-88 / CORR-v0.2-07. A true unassignment (`from == self`,
    /// `to == ""`) must emit a `JiraIssueUnassigned` bullet. Pre-fix
    /// this was silently dropped because the walker only looked at the
    /// `to == self` branch.
    #[test]
    fn assignee_change_from_self_to_empty_emits_jira_issue_unassigned() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "assignee", "from": SELF_ID, "fromString": "Me",
                         "to": "", "toString": ""}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        let ev = &out.events[0];
        assert_eq!(ev.kind, ActivityKind::JiraIssueUnassigned);
        assert_eq!(ev.title, "CAR-5117 unassigned");
        assert_eq!(
            ev.metadata,
            json!({}),
            "empty `to` must not record a reassignment target"
        );
    }

    /// DAY-88 / CORR-v0.2-07. A handoff (`from == self`, `to == other`)
    /// also emits a `JiraIssueUnassigned` and records the new assignee
    /// in `metadata.reassigned_to_account_id` so downstream renderers
    /// can distinguish handoffs from true unassignments.
    #[test]
    fn assignee_change_from_self_to_other_emits_unassigned_with_handoff_metadata() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "assignee", "from": SELF_ID, "fromString": "Me",
                         "to": "teammate-account", "toString": "Teammate"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        let ev = &out.events[0];
        assert_eq!(ev.kind, ActivityKind::JiraIssueUnassigned);
        assert_eq!(
            ev.metadata["reassigned_to_account_id"],
            json!("teammate-account")
        );
    }

    /// DAY-88 / CORR-v0.2-04. A three-transition cascade must surface
    /// every intermediate status in `metadata.via` on the emitted
    /// transition event. Pre-fix, only the earliest `from` and the
    /// latest `to` survived; intermediates were overwritten.
    #[test]
    fn transition_event_metadata_includes_via_on_collapsed_cascade() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "status", "fromString": "Todo", "toString": "In Progress"}
                    ]
                },
                {
                    "id": "2",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:05.000+0000",
                    "items": [
                        {"field": "status", "fromString": "In Progress", "toString": "Review"}
                    ]
                },
                {
                    "id": "3",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:10.000+0000",
                    "items": [
                        {"field": "status", "fromString": "Review", "toString": "Done"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(
            out.events.len(),
            1,
            "the three transitions must collapse into one event"
        );
        let ev = &out.events[0];
        assert_eq!(ev.kind, ActivityKind::JiraIssueTransitioned);
        assert_eq!(ev.metadata["transition_count"], json!(3));
        assert_eq!(ev.metadata["from_status"], json!("Todo"));
        assert_eq!(ev.metadata["to_status"], json!("Done"));
        assert_eq!(ev.metadata["via"], json!(["In Progress", "Review"]));
    }

    /// Non-collapsed singleton transitions still carry an empty `via`
    /// array so downstream consumers can rely on the key existing
    /// (serde would omit it if the field were `Option<Vec<String>>`,
    /// which would force an awkward `get().is_some_or_...` check).
    #[test]
    fn transition_event_metadata_via_is_empty_array_for_singleton() {
        let mut issue = issue_skeleton();
        issue["changelog"] = json!({
            "histories": [
                {
                    "id": "1",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "items": [
                        {"field": "status", "fromString": "To Do", "toString": "In Progress"}
                    ]
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(out.events[0].metadata["via"], json!([]));
    }

    #[test]
    fn issue_created_by_self_in_window_emits_issue_created() {
        let mut issue = issue_skeleton();
        issue["fields"]["created"] = json!("2026-04-20T09:00:00.000+0000");
        issue["fields"]["reporter"] = json!({"accountId": SELF_ID, "displayName": "Me"});
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].kind, ActivityKind::JiraIssueCreated);
        assert!(out.events[0].title.contains("CAR-5117"));
        assert!(out.events[0].title.contains("Fix review findings"));
    }

    #[test]
    fn self_comment_emits_jira_issue_commented_with_adf_rendering() {
        let mut issue = issue_skeleton();
        issue["fields"]["comment"] = json!({
            "comments": [
                {
                    "id": "500",
                    "author": {"accountId": SELF_ID, "displayName": "Me"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "body": {
                        "type": "doc",
                        "content": [
                            {
                                "type": "paragraph",
                                "content": [
                                    {"type": "text", "text": "Hello "},
                                    {"type": "mention",
                                     "attrs": {"id": "secret", "text": "@Saravanan"}},
                                    {"type": "text", "text": " bye"}
                                ]
                            }
                        ]
                    }
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        let ev = &out.events[0];
        assert_eq!(ev.kind, ActivityKind::JiraIssueCommented);
        let body = ev.body.as_ref().expect("comment body rendered");
        assert!(body.contains("Hello"));
        // Privacy: mention must render @Saravanan, never the accountId.
        assert!(body.contains("@Saravanan"));
        assert!(!body.contains("secret"));
    }

    #[test]
    fn comment_by_other_user_is_not_emitted() {
        let mut issue = issue_skeleton();
        issue["fields"]["comment"] = json!({
            "comments": [
                {
                    "id": "501",
                    "author": {"accountId": "someone-else", "displayName": "Other"},
                    "created": "2026-04-20T10:00:00.000+0000",
                    "body": {"type": "doc", "content": []}
                }
            ]
        });
        let out = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap();
        assert!(out.events.is_empty());
    }

    #[test]
    fn issue_envelope_missing_key_returns_shape_changed() {
        let issue = json!({"fields": {}});
        let err = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code(), SHAPE_CHANGED_CODE);
    }

    #[test]
    fn issue_missing_project_returns_shape_changed() {
        let issue = json!({"key": "CAR-1", "fields": {}});
        let err = normalise_issue(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &issue,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code(), SHAPE_CHANGED_CODE);
    }
}
