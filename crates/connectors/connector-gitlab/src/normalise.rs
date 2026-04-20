//! `GitlabEvent` → [`dayseam_core::ActivityEvent`] mapping.
//!
//! One match arm per [`dayseam_core::ActivityKind`] variant GitLab can
//! produce. Every arm computes `ActivityEvent::id` from
//! [`ActivityEvent::deterministic_id`] so a re-sync of the same day
//! regenerates byte-identical rows — which is the guarantee the
//! `INSERT OR IGNORE` path added in DAY-52 relies on.
//!
//! v0.1 routes push events to a *single* `CommitAuthored` summary per
//! push (one bullet per push to some ref). Per-commit enrichment —
//! turning one push with N SHAs into N separate `CommitAuthored`
//! events, then deduping against any local-git walk producing the
//! same SHAs — lands in Phase 3 Task 2.

use chrono::Utc;
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityRef, Link, Privacy, RawRef, SourceId,
};

use crate::events::{GitlabAction, GitlabAuthor, GitlabEvent, GitlabTargetType};

/// Upper bound on per-push commit enrichment. Task 1 does not enrich;
/// Task 2 will use this cap when fetching per-commit detail so a
/// 200-commit push can not spawn 200 bullets.
pub const MAX_COMMITS_PER_PUSH: usize = 50;

/// The result of normalising one event. `Some` when the event is of a
/// kind the connector surfaces; `None` when we intentionally drop it
/// (e.g. unknown target type, comment on an object kind we don't
/// render).
pub type NormalisedEvent = Option<ActivityEvent>;

/// Map one [`GitlabEvent`] to an [`ActivityEvent`]. `base_url` is the
/// project-facing GitLab host; we use it to compose deep-links on the
/// resulting bullet. `source_id` scopes the deterministic id so two
/// distinct sources cannot collide even if they share a GitLab
/// instance.
pub fn normalise_event(
    source_id: SourceId,
    base_url: &str,
    event: &GitlabEvent,
) -> NormalisedEvent {
    let action = GitlabAction::parse(&event.action_name);
    let kind = match (action, event.target_type) {
        (GitlabAction::Pushed, _) => ActivityKind::CommitAuthored,
        (GitlabAction::Opened, Some(GitlabTargetType::MergeRequest)) => ActivityKind::MrOpened,
        (GitlabAction::Merged, Some(GitlabTargetType::MergeRequest)) => ActivityKind::MrMerged,
        (GitlabAction::Closed, Some(GitlabTargetType::MergeRequest)) => ActivityKind::MrClosed,
        (GitlabAction::Approved, Some(GitlabTargetType::MergeRequest)) => ActivityKind::MrApproved,
        (GitlabAction::Opened, Some(GitlabTargetType::Issue)) => ActivityKind::IssueOpened,
        (GitlabAction::Closed, Some(GitlabTargetType::Issue)) => ActivityKind::IssueClosed,
        // v0.1 routes every note-like comment to MrReviewComment by
        // default because MR review notes dominate dev workflow
        // traffic; routing notes authored on issues to IssueComment
        // requires the `noteable_type` field which the events API
        // does not include on every row. Task 2 picks this up with
        // per-target enrichment.
        (
            GitlabAction::Commented,
            Some(
                GitlabTargetType::Note
                | GitlabTargetType::DiffNote
                | GitlabTargetType::DiscussionNote,
            ),
        ) => ActivityKind::MrReviewComment,
        (GitlabAction::Commented, Some(GitlabTargetType::Issue)) => ActivityKind::IssueComment,
        // Any other combination — an unknown action, a
        // known-action-on-unknown-target, a bare comment on a bare
        // Note target with no disambiguating context — is silently
        // dropped. "Unknown but forward-compatible" is the right
        // default for an events endpoint that adds variants
        // regularly; schema-drift surfacing happens at the event
        // decoding layer (`events.rs::GitlabTargetType::Unknown`).
        _ => return None,
    };

    let external_id = event_external_id(event, kind);
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token(kind));

    let (title, body) = title_and_body(event, kind);
    let actor = actor_from_event(event);
    let links = compose_links(base_url, event, kind);
    let entities = compose_entities(event);

    Some(ActivityEvent {
        id,
        source_id,
        external_id: external_id.clone(),
        kind,
        occurred_at: event.created_at.with_timezone(&Utc),
        actor,
        title,
        body,
        links,
        entities,
        parent_external_id: parent_external_id(event, kind),
        metadata: metadata(event, kind),
        raw_ref: RawRef {
            storage_key: format!("gitlab:event:{id}", id = event.id),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    })
}

/// Convenience: lowercase-lookup token used by
/// [`ActivityEvent::deterministic_id`]. Kept in one place so the id
/// scheme cannot drift from the enum's name.
fn kind_token(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::CommitAuthored => "CommitAuthored",
        ActivityKind::MrOpened => "MrOpened",
        ActivityKind::MrMerged => "MrMerged",
        ActivityKind::MrClosed => "MrClosed",
        ActivityKind::MrApproved => "MrApproved",
        ActivityKind::MrReviewComment => "MrReviewComment",
        ActivityKind::IssueOpened => "IssueOpened",
        ActivityKind::IssueClosed => "IssueClosed",
        ActivityKind::IssueComment => "IssueComment",
    }
}

fn event_external_id(event: &GitlabEvent, kind: ActivityKind) -> String {
    match kind {
        // For pushes, anchor on the tip SHA (unique across repos) with
        // a fallback to the event id so a push without commit_to
        // surfaced (rare, but possible on forks without full
        // enrichment) is still addressable.
        ActivityKind::CommitAuthored => event
            .push_data
            .as_ref()
            .and_then(|p| p.commit_to.clone())
            .unwrap_or_else(|| format!("event:{}", event.id)),
        ActivityKind::MrOpened
        | ActivityKind::MrMerged
        | ActivityKind::MrClosed
        | ActivityKind::MrApproved => event
            .target_iid
            .map(|iid| format!("!{iid}"))
            .unwrap_or_else(|| format!("event:{}", event.id)),
        ActivityKind::IssueOpened | ActivityKind::IssueClosed => event
            .target_iid
            .map(|iid| format!("#{iid}"))
            .unwrap_or_else(|| format!("event:{}", event.id)),
        // Comments are uniquely addressable by the note's id; we
        // surface that directly so a comment on the same MR body has
        // a distinct external_id from the MR's open/merge events.
        ActivityKind::MrReviewComment | ActivityKind::IssueComment => event
            .target_id
            .map(|id| format!("note:{id}"))
            .unwrap_or_else(|| format!("event:{}", event.id)),
    }
}

fn title_and_body(event: &GitlabEvent, kind: ActivityKind) -> (String, Option<String>) {
    let title = match kind {
        ActivityKind::CommitAuthored => push_title(event),
        ActivityKind::MrOpened => format!(
            "Opened MR: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
        ActivityKind::MrMerged => format!(
            "Merged MR: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
        ActivityKind::MrClosed => format!(
            "Closed MR: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
        ActivityKind::MrApproved => format!(
            "Approved MR: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
        ActivityKind::MrReviewComment => format!(
            "Commented on MR: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
        ActivityKind::IssueOpened => format!(
            "Opened issue: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
        ActivityKind::IssueClosed => format!(
            "Closed issue: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
        ActivityKind::IssueComment => format!(
            "Commented on issue: {}",
            event.target_title.as_deref().unwrap_or("(no title)")
        ),
    };
    let body = event
        .note
        .as_ref()
        .and_then(|n| n.body.clone())
        .filter(|s| !s.trim().is_empty());
    (title, body)
}

fn push_title(event: &GitlabEvent) -> String {
    let (count, git_ref) = event
        .push_data
        .as_ref()
        .map(|p| (p.commit_count.unwrap_or(1), p.git_ref.clone()))
        .unwrap_or((1, None));

    let ref_label = git_ref
        .as_deref()
        .and_then(|r| r.strip_prefix("refs/heads/").or(Some(r)))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "a branch".to_string());

    let noun = if count == 1 { "commit" } else { "commits" };
    format!("Pushed {count} {noun} to {ref_label}")
}

fn actor_from_event(event: &GitlabEvent) -> Actor {
    match event.author.as_ref() {
        Some(GitlabAuthor {
            id, username, name, ..
        }) => Actor {
            display_name: name.clone().unwrap_or_else(|| username.clone()),
            email: None,
            external_id: Some(id.to_string()),
        },
        None => Actor {
            display_name: format!("user:{}", event.author_id),
            email: None,
            external_id: Some(event.author_id.to_string()),
        },
    }
}

fn compose_links(base_url: &str, event: &GitlabEvent, kind: ActivityKind) -> Vec<Link> {
    let base = base_url.trim_end_matches('/');
    let project_slug = event
        .project_id
        .map(|id| format!("projects/{id}"))
        .unwrap_or_else(|| "projects/unknown".to_string());

    match (kind, event.target_iid) {
        (ActivityKind::MrOpened, Some(iid))
        | (ActivityKind::MrMerged, Some(iid))
        | (ActivityKind::MrClosed, Some(iid))
        | (ActivityKind::MrApproved, Some(iid))
        | (ActivityKind::MrReviewComment, Some(iid)) => vec![Link {
            url: format!("{base}/-/api/v4/{project_slug}/merge_requests/{iid}"),
            label: Some(format!("!{iid}")),
        }],
        (ActivityKind::IssueOpened, Some(iid))
        | (ActivityKind::IssueClosed, Some(iid))
        | (ActivityKind::IssueComment, Some(iid)) => vec![Link {
            url: format!("{base}/-/api/v4/{project_slug}/issues/{iid}"),
            label: Some(format!("#{iid}")),
        }],
        (ActivityKind::CommitAuthored, _) => {
            let sha = event
                .push_data
                .as_ref()
                .and_then(|p| p.commit_to.as_deref())
                .unwrap_or("");
            let mut links = vec![];
            if !sha.is_empty() {
                links.push(Link {
                    url: format!("{base}/-/api/v4/{project_slug}/repository/commits/{sha}"),
                    label: Some(short_sha(sha).to_string()),
                });
            }
            links
        }
        _ => Vec::new(),
    }
}

fn compose_entities(event: &GitlabEvent) -> Vec<EntityRef> {
    let mut entities = Vec::new();
    if let Some(pid) = event.project_id {
        entities.push(EntityRef {
            kind: "project".to_string(),
            external_id: pid.to_string(),
            label: None,
        });
    }
    if let Some(iid) = event.target_iid {
        let label = match event.target_type {
            Some(GitlabTargetType::MergeRequest) => Some(format!("!{iid}")),
            Some(GitlabTargetType::Issue) => Some(format!("#{iid}")),
            _ => None,
        };
        entities.push(EntityRef {
            kind: "target".to_string(),
            external_id: iid.to_string(),
            label,
        });
    }
    entities
}

fn parent_external_id(event: &GitlabEvent, kind: ActivityKind) -> Option<String> {
    match kind {
        ActivityKind::MrReviewComment => event.target_iid.map(|iid| format!("!{iid}")),
        ActivityKind::IssueComment => event.target_iid.map(|iid| format!("#{iid}")),
        _ => None,
    }
}

fn metadata(event: &GitlabEvent, kind: ActivityKind) -> serde_json::Value {
    match (kind, event.push_data.as_ref()) {
        (ActivityKind::CommitAuthored, Some(p)) => serde_json::json!({
            "gitlab_event_id": event.id,
            "commit_count": p.commit_count,
            "ref": p.git_ref,
            "commit_from": p.commit_from,
            "commit_to": p.commit_to,
        }),
        _ => serde_json::json!({ "gitlab_event_id": event.id }),
    }
}

fn short_sha(sha: &str) -> &str {
    let len = sha.len().min(8);
    &sha[..len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{GitlabAuthor, GitlabNote, GitlabPushData};
    use chrono::TimeZone;
    use uuid::Uuid;

    fn source() -> SourceId {
        Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
    }

    fn author() -> GitlabAuthor {
        GitlabAuthor {
            id: 17,
            username: "vedanth".into(),
            name: Some("Vedanth".into()),
            web_url: None,
        }
    }

    fn push_event() -> GitlabEvent {
        GitlabEvent {
            id: 1001,
            action_name: "pushed to".into(),
            target_type: None,
            target_iid: None,
            target_id: None,
            target_title: None,
            project_id: Some(42),
            created_at: Utc.with_ymd_and_hms(2026, 4, 19, 12, 0, 0).unwrap(),
            author_id: 17,
            author: Some(author()),
            note: None,
            push_data: Some(GitlabPushData {
                git_ref: Some("refs/heads/main".into()),
                commit_count: Some(3),
                commit_to: Some("abcdef1234567890".into()),
                commit_from: Some("0000000000000000".into()),
            }),
        }
    }

    fn mr_opened_event() -> GitlabEvent {
        GitlabEvent {
            id: 1002,
            action_name: "opened".into(),
            target_type: Some(GitlabTargetType::MergeRequest),
            target_iid: Some(11),
            target_id: Some(2001),
            target_title: Some("Add payments slice".into()),
            project_id: Some(42),
            created_at: Utc.with_ymd_and_hms(2026, 4, 19, 12, 5, 0).unwrap(),
            author_id: 17,
            author: Some(author()),
            note: None,
            push_data: None,
        }
    }

    #[test]
    fn push_event_becomes_commit_authored_with_sha_external_id() {
        let e = normalise_event(source(), "https://gitlab.example", &push_event()).unwrap();
        assert_eq!(e.kind, ActivityKind::CommitAuthored);
        assert_eq!(e.external_id, "abcdef1234567890");
        assert_eq!(e.title, "Pushed 3 commits to main");
        assert_eq!(e.actor.external_id.as_deref(), Some("17"));
    }

    #[test]
    fn mr_opened_becomes_mr_opened_kind_and_bang_iid_external() {
        let e = normalise_event(source(), "https://gitlab.example", &mr_opened_event()).unwrap();
        assert_eq!(e.kind, ActivityKind::MrOpened);
        assert_eq!(e.external_id, "!11");
        assert!(e.title.starts_with("Opened MR: "));
    }

    #[test]
    fn mr_merged_closed_approved_route_correctly() {
        for (action, expected) in [
            ("merged", ActivityKind::MrMerged),
            ("closed", ActivityKind::MrClosed),
            ("approved", ActivityKind::MrApproved),
        ] {
            let mut ev = mr_opened_event();
            ev.action_name = action.into();
            let normalised = normalise_event(source(), "https://gitlab.example", &ev)
                .unwrap_or_else(|| panic!("expected normalisation to succeed for action={action}"));
            assert_eq!(normalised.kind, expected, "action={action}");
        }
    }

    #[test]
    fn issue_opened_and_closed_route_correctly() {
        let mut ev = mr_opened_event();
        ev.target_type = Some(GitlabTargetType::Issue);
        ev.target_iid = Some(7);
        let opened = normalise_event(source(), "https://gitlab.example", &ev).unwrap();
        assert_eq!(opened.kind, ActivityKind::IssueOpened);
        assert_eq!(opened.external_id, "#7");

        ev.action_name = "closed".into();
        let closed = normalise_event(source(), "https://gitlab.example", &ev).unwrap();
        assert_eq!(closed.kind, ActivityKind::IssueClosed);
    }

    #[test]
    fn comment_on_note_target_routes_to_mr_review_comment_with_parent() {
        let ev = GitlabEvent {
            id: 1003,
            action_name: "commented on".into(),
            target_type: Some(GitlabTargetType::Note),
            target_iid: Some(11),
            target_id: Some(555),
            target_title: Some("Add payments slice".into()),
            project_id: Some(42),
            created_at: Utc.with_ymd_and_hms(2026, 4, 19, 12, 10, 0).unwrap(),
            author_id: 17,
            author: Some(author()),
            note: Some(GitlabNote {
                body: Some("LGTM".into()),
            }),
            push_data: None,
        };
        let normalised = normalise_event(source(), "https://gitlab.example", &ev).unwrap();
        assert_eq!(normalised.kind, ActivityKind::MrReviewComment);
        assert_eq!(normalised.parent_external_id.as_deref(), Some("!11"));
        assert_eq!(normalised.body.as_deref(), Some("LGTM"));
    }

    #[test]
    fn unknown_action_and_target_returns_none_instead_of_panic() {
        let mut ev = mr_opened_event();
        ev.action_name = "exotic".into();
        ev.target_type = Some(GitlabTargetType::Unknown);
        assert!(normalise_event(source(), "https://gitlab.example", &ev).is_none());
    }

    /// Plan Task 1 invariant 2 — same input normalises byte-identically
    /// on two independent calls, which is what the
    /// [`ActivityEvent::deterministic_id`] contract guarantees.
    #[test]
    fn normalisation_is_deterministic() {
        let a = normalise_event(source(), "https://gitlab.example", &push_event()).unwrap();
        let b = normalise_event(source(), "https://gitlab.example", &push_event()).unwrap();
        assert_eq!(a, b);
    }
}
