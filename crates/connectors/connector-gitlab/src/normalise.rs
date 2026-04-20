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

use std::collections::HashMap;

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
///
/// `project_paths` is the per-walk cache
/// ([`crate::project::fetch_project_path`]) mapping each seen
/// `project_id` to its `path_with_namespace` (or `None` if the
/// lookup was unsuccessful). The normaliser uses it to stamp a
/// `repo` [`EntityRef`] onto the event so the report rollup keys
/// bullets by repo rather than falling back to the `/` sentinel
/// and rendering `**/** — …`. A missing or `None` entry degrades
/// the `repo` entity's `external_id` to a synthetic `project-<id>`
/// token so the rollup still has a deterministic key; the render
/// layer detects the synthetic shape and drops the bolded prefix
/// (see `crates/dayseam-report/src/render.rs::commit_headline`).
pub fn normalise_event(
    source_id: SourceId,
    base_url: &str,
    event: &GitlabEvent,
    project_paths: &HashMap<i64, Option<String>>,
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
    let entities = compose_entities(event, project_paths);

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

/// Compose the evidence `Link` for a GitLab event.
///
/// Phase 3 CORR-02: earlier Phase 3 PRs emitted URLs shaped like
/// `{base}/-/api/v4/projects/{id}/merge_requests/{iid}`, mixing the
/// GitLab UI routing prefix (`/-/`) with the REST API path
/// (`/api/v4/`). That combination resolves on neither surface — every
/// click from a GitLab evidence popover 404'd. Phase 3.5 (DAY-68) fixes
/// this by emitting clean API URLs so the link at least resolves.
///
/// Known trade-off tracked as a v0.1.1 follow-up (DAY-69): the emitted
/// URLs return JSON, not HTML, when opened in a browser. The
/// user-friendly fix is to fetch each unique `project.web_url` during
/// the walk and compose UI paths
/// (`{web_url}/-/merge_requests/{iid}`, `/-/issues/{iid}`,
/// `/-/commit/{sha}`). That requires threading an extra GET per unique
/// project per day plus a project-id → web_url cache; it's real work
/// and belongs in its own PR.
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
            url: format!("{base}/api/v4/{project_slug}/merge_requests/{iid}"),
            label: Some(format!("!{iid}")),
        }],
        (ActivityKind::IssueOpened, Some(iid))
        | (ActivityKind::IssueClosed, Some(iid))
        | (ActivityKind::IssueComment, Some(iid)) => vec![Link {
            url: format!("{base}/api/v4/{project_slug}/issues/{iid}"),
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
                    url: format!("{base}/api/v4/{project_slug}/repository/commits/{sha}"),
                    label: Some(short_sha(sha).to_string()),
                });
            }
            links
        }
        _ => Vec::new(),
    }
}

fn compose_entities(
    event: &GitlabEvent,
    project_paths: &HashMap<i64, Option<String>>,
) -> Vec<EntityRef> {
    let mut entities = Vec::new();
    if let Some(pid) = event.project_id {
        entities.push(EntityRef {
            kind: "project".to_string(),
            external_id: pid.to_string(),
            label: None,
        });

        // The `repo` entity is what the report rollup
        // (`crates/dayseam-report/src/rollup.rs::repo_path_from_event`)
        // groups bullets by and what the render layer surfaces as the
        // bolded prefix. Emitting it here — with `path_with_namespace`
        // when we could resolve it, and a synthetic `project-<id>`
        // token otherwise — means every GitLab event lands on a
        // stable, non-`/` key. Missing the entity was DAY-71's
        // "**/**" rendering bug.
        let repo_external_id = match project_paths.get(&pid).and_then(|p| p.clone()) {
            Some(path) => path,
            None => format!("project-{pid}"),
        };
        let label = repo_external_id.rsplit('/').next().map(|s| s.to_string());
        entities.push(EntityRef {
            kind: "repo".to_string(),
            external_id: repo_external_id,
            label,
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

    fn empty_paths() -> HashMap<i64, Option<String>> {
        HashMap::new()
    }

    fn paths_with(pid: i64, path: &str) -> HashMap<i64, Option<String>> {
        let mut m = HashMap::new();
        m.insert(pid, Some(path.to_string()));
        m
    }

    #[test]
    fn push_event_becomes_commit_authored_with_sha_external_id() {
        let e = normalise_event(
            source(),
            "https://gitlab.example",
            &push_event(),
            &empty_paths(),
        )
        .unwrap();
        assert_eq!(e.kind, ActivityKind::CommitAuthored);
        assert_eq!(e.external_id, "abcdef1234567890");
        assert_eq!(e.title, "Pushed 3 commits to main");
        assert_eq!(e.actor.external_id.as_deref(), Some("17"));
    }

    #[test]
    fn mr_opened_becomes_mr_opened_kind_and_bang_iid_external() {
        let e = normalise_event(
            source(),
            "https://gitlab.example",
            &mr_opened_event(),
            &empty_paths(),
        )
        .unwrap();
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
            let normalised =
                normalise_event(source(), "https://gitlab.example", &ev, &empty_paths())
                    .unwrap_or_else(|| {
                        panic!("expected normalisation to succeed for action={action}")
                    });
            assert_eq!(normalised.kind, expected, "action={action}");
        }
    }

    #[test]
    fn issue_opened_and_closed_route_correctly() {
        let mut ev = mr_opened_event();
        ev.target_type = Some(GitlabTargetType::Issue);
        ev.target_iid = Some(7);
        let opened =
            normalise_event(source(), "https://gitlab.example", &ev, &empty_paths()).unwrap();
        assert_eq!(opened.kind, ActivityKind::IssueOpened);
        assert_eq!(opened.external_id, "#7");

        ev.action_name = "closed".into();
        let closed =
            normalise_event(source(), "https://gitlab.example", &ev, &empty_paths()).unwrap();
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
        let normalised =
            normalise_event(source(), "https://gitlab.example", &ev, &empty_paths()).unwrap();
        assert_eq!(normalised.kind, ActivityKind::MrReviewComment);
        assert_eq!(normalised.parent_external_id.as_deref(), Some("!11"));
        assert_eq!(normalised.body.as_deref(), Some("LGTM"));
    }

    #[test]
    fn unknown_action_and_target_returns_none_instead_of_panic() {
        let mut ev = mr_opened_event();
        ev.action_name = "exotic".into();
        ev.target_type = Some(GitlabTargetType::Unknown);
        assert!(normalise_event(source(), "https://gitlab.example", &ev, &empty_paths()).is_none());
    }

    /// Plan Task 1 invariant 2 — same input normalises byte-identically
    /// on two independent calls, which is what the
    /// [`ActivityEvent::deterministic_id`] contract guarantees.
    #[test]
    fn normalisation_is_deterministic() {
        let a = normalise_event(
            source(),
            "https://gitlab.example",
            &push_event(),
            &empty_paths(),
        )
        .unwrap();
        let b = normalise_event(
            source(),
            "https://gitlab.example",
            &push_event(),
            &empty_paths(),
        )
        .unwrap();
        assert_eq!(a, b);
    }

    /// DAY-71 regression: when the walker successfully resolved the
    /// project's `path_with_namespace`, the event must carry a
    /// `repo` [`EntityRef`] so the report rollup keys bullets by
    /// repo rather than falling back to the `/` sentinel.
    #[test]
    fn push_event_emits_repo_entity_when_path_known() {
        let paths = paths_with(42, "modulr/modulo-local-infra");
        let e = normalise_event(source(), "https://gitlab.example", &push_event(), &paths).unwrap();

        let repo_entity = e
            .entities
            .iter()
            .find(|r| r.kind == "repo")
            .expect("normalised event must carry a repo entity when the lookup succeeded");
        assert_eq!(repo_entity.external_id, "modulr/modulo-local-infra");
        assert_eq!(repo_entity.label.as_deref(), Some("modulo-local-infra"));
    }

    /// DAY-71 regression: when the walker could not resolve
    /// `path_with_namespace` (404/403/missing field), the event still
    /// carries a `repo` entity so the rollup has a deterministic key,
    /// but the external_id is the synthetic `project-<id>` token the
    /// render layer recognises to drop its bolded prefix.
    #[test]
    fn push_event_emits_synthetic_repo_entity_when_path_missing() {
        let mut paths: HashMap<i64, Option<String>> = HashMap::new();
        paths.insert(42, None);
        let e = normalise_event(source(), "https://gitlab.example", &push_event(), &paths).unwrap();

        let repo_entity = e
            .entities
            .iter()
            .find(|r| r.kind == "repo")
            .expect("repo entity must always be present when project_id is known");
        assert_eq!(repo_entity.external_id, "project-42");
    }

    /// Phase 3 CORR-02 regression.
    ///
    /// Evidence links for MRs, issues, and commits must point at paths
    /// that actually resolve on a real GitLab host. Before the fix they
    /// were shaped `{base}/-/api/v4/projects/{id}/merge_requests/{iid}`
    /// — `/-/` is the GitLab UI routing prefix and `api/v4/` is the
    /// REST API prefix, and no GitLab endpoint answers a request that
    /// mixes the two. The fix drops `/-/` so the URL becomes a clean
    /// REST-API path that at least resolves.
    #[test]
    fn compose_links_emit_clean_api_paths_without_ui_prefix() {
        let base = "https://gitlab.example";

        let mr = normalise_event(source(), base, &mr_opened_event(), &empty_paths()).unwrap();
        let mr_url = &mr.links[0].url;
        assert_eq!(
            mr_url, "https://gitlab.example/api/v4/projects/42/merge_requests/11",
            "MR link must not carry the `/-/` UI prefix"
        );
        assert!(
            !mr_url.contains("/-/"),
            "MR link must not contain `/-/`; got {mr_url}"
        );

        let mut issue_ev = mr_opened_event();
        issue_ev.target_type = Some(GitlabTargetType::Issue);
        issue_ev.target_iid = Some(7);
        let issue = normalise_event(source(), base, &issue_ev, &empty_paths()).unwrap();
        let issue_url = &issue.links[0].url;
        assert_eq!(
            issue_url, "https://gitlab.example/api/v4/projects/42/issues/7",
            "Issue link must not carry the `/-/` UI prefix"
        );
        assert!(
            !issue_url.contains("/-/"),
            "Issue link must not contain `/-/`; got {issue_url}"
        );

        let commit = normalise_event(source(), base, &push_event(), &empty_paths()).unwrap();
        let commit_url = &commit.links[0].url;
        assert_eq!(
            commit_url,
            "https://gitlab.example/api/v4/projects/42/repository/commits/abcdef1234567890",
            "Commit link must not carry the `/-/` UI prefix"
        );
        assert!(
            !commit_url.contains("/-/"),
            "Commit link must not contain `/-/`; got {commit_url}"
        );
    }
}
