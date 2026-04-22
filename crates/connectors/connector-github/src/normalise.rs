//! `GithubEvent` + `GithubSearchIssue` → [`dayseam_core::ActivityEvent`] mapping.
//!
//! One match arm per [`dayseam_core::ActivityKind`] variant GitHub can
//! produce. Every arm computes `ActivityEvent::id` from
//! [`ActivityEvent::deterministic_id`] so a re-sync of the same day
//! regenerates byte-identical rows — the guarantee the
//! `INSERT OR IGNORE` upsert path relies on.
//!
//! ### Design choices
//!
//! * **`PushEvent` is not normalised.** GitHub push events surface
//!   bare SHAs without per-commit metadata; commits in a repo the
//!   user also has as a local-git source render through that pipe.
//!   `docs/plan/2026-04-22-v0.4-github-connector.md` Task 5 records
//!   this as a non-goal for v0.4.
//! * **Self-filter lives in the walker, not the normaliser.** Every
//!   arm in this module assumes the caller has already checked
//!   `event.actor.id == self.id`; the normaliser trusts its input.
//! * **Jira ticket-key enrichment** — we scan the PR / issue title
//!   for `[A-Z][A-Z0-9]+-\d+` tokens and emit one
//!   `EntityKind::JiraIssue` per hit. The report layer (DAY-97)
//!   uses the tokens to cross-link a Jira transition with the
//!   triggering PR.

use chrono::Utc;
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityKind, EntityRef, Link, Privacy, RawRef, SourceId,
};

use crate::events::{
    GithubEvent, GithubEventPayload, GithubIssue, GithubPullRequest, GithubReview, GithubUserRef,
};

/// Max Jira-style ticket keys we'll emit per event. A PR title with
/// thirty references is pathological; cap the enrichment so one
/// run-away row can't dwarf the rest of the event's entities vec.
pub const MAX_TICKET_KEYS_PER_EVENT: usize = 8;

/// The result of normalising one event: `Some` when the event is of
/// a kind the connector surfaces, `None` when we intentionally drop
/// it (unknown payload type, push event, bot review, etc.).
pub type NormalisedEvent = Option<ActivityEvent>;

/// Entry point. Decodes the event's lazy payload, routes on
/// `(payload_type, action)`, and emits zero-or-one `ActivityEvent`.
pub fn normalise_event(source_id: SourceId, event: &GithubEvent) -> NormalisedEvent {
    let payload = GithubEventPayload::from_raw(&event.event_type, &event.payload);
    match payload {
        GithubEventPayload::PullRequest {
            action,
            number: _,
            pull_request,
        } => normalise_pull_request(source_id, event, &action, &pull_request),
        GithubEventPayload::PullRequestReview {
            action,
            pull_request,
            review,
        } => normalise_pull_request_review(source_id, event, &action, &pull_request, &review),
        GithubEventPayload::PullRequestReviewComment {
            action,
            pull_request,
            comment: _,
        } => normalise_pull_request_review_comment(source_id, event, &action, &pull_request),
        GithubEventPayload::Issues {
            action,
            issue,
            assignee,
        } => normalise_issues_event(source_id, event, &action, &issue, assignee.as_ref()),
        GithubEventPayload::IssueComment {
            action,
            issue,
            comment: _,
        } => normalise_issue_comment(source_id, event, &action, &issue),
        GithubEventPayload::Push | GithubEventPayload::Unknown { .. } => None,
    }
}

fn normalise_pull_request(
    source_id: SourceId,
    event: &GithubEvent,
    action: &str,
    pr: &GithubPullRequest,
) -> NormalisedEvent {
    let kind = match action {
        "opened" => ActivityKind::GitHubPullRequestOpened,
        "closed" if pr.merged => ActivityKind::GitHubPullRequestMerged,
        "closed" => ActivityKind::GitHubPullRequestClosed,
        _ => return None,
    };
    Some(compose_pr_event(source_id, event, kind, pr, None))
}

fn normalise_pull_request_review(
    source_id: SourceId,
    event: &GithubEvent,
    action: &str,
    pr: &GithubPullRequest,
    review: &GithubReview,
) -> NormalisedEvent {
    if action != "submitted" {
        return None;
    }
    let kind = ActivityKind::GitHubPullRequestReviewed;
    let metadata = serde_json::json!({
        "github_event_id": event.id,
        "pr_number": pr.number,
        "repo": event.repo.name,
        "review_id": review.id,
        "review_state": review.state,
        "review_count": 1,
    });
    Some(compose_pr_event(source_id, event, kind, pr, Some(metadata)))
}

fn normalise_pull_request_review_comment(
    source_id: SourceId,
    event: &GithubEvent,
    action: &str,
    pr: &GithubPullRequest,
) -> NormalisedEvent {
    if action != "created" {
        return None;
    }
    let kind = ActivityKind::GitHubPullRequestCommented;
    Some(compose_pr_event(source_id, event, kind, pr, None))
}

fn normalise_issues_event(
    source_id: SourceId,
    event: &GithubEvent,
    action: &str,
    issue: &GithubIssue,
    assignee: Option<&GithubUserRef>,
) -> NormalisedEvent {
    // `IssuesEvent` on a PR (issue.pull_request.is_some()) is rare
    // in practice because PR actions come through PullRequestEvent;
    // when it does surface (e.g. an "assigned" event on a PR) we
    // treat it like an issue — assignment-to-a-PR is still worth
    // surfacing under the PR thread.
    let kind = match action {
        "opened" if !issue.is_pull_request() => ActivityKind::GitHubIssueOpened,
        "closed" if !issue.is_pull_request() => ActivityKind::GitHubIssueClosed,
        "assigned" => {
            let actor_login = event.actor.login.as_str();
            let assigned_to = assignee.map(|a| a.login.as_str()).unwrap_or("");
            // Only surface the event when the user was assigned to
            // themselves or to someone else's thread they were
            // actioning. The walker already confirmed
            // `event.actor == self`; we additionally require the
            // assignee to be self so "I assigned CAR-5117 to a
            // teammate" doesn't clutter the user's EOD.
            if assigned_to.is_empty() || assigned_to != actor_login {
                return None;
            }
            ActivityKind::GitHubIssueAssigned
        }
        _ => return None,
    };
    Some(compose_issue_event(source_id, event, kind, issue))
}

fn normalise_issue_comment(
    source_id: SourceId,
    event: &GithubEvent,
    action: &str,
    issue: &GithubIssue,
) -> NormalisedEvent {
    if action != "created" {
        return None;
    }
    let kind = if issue.is_pull_request() {
        ActivityKind::GitHubPullRequestCommented
    } else {
        ActivityKind::GitHubIssueCommented
    };
    Some(compose_issue_event(source_id, event, kind, issue))
}

fn compose_pr_event(
    source_id: SourceId,
    event: &GithubEvent,
    kind: ActivityKind,
    pr: &GithubPullRequest,
    metadata_override: Option<serde_json::Value>,
) -> ActivityEvent {
    let external_id = format!("{}#{}", event.repo.name, pr.number);
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token(kind));

    let title = pr_title(kind, pr);
    let body = None;
    let actor = actor_from_event(event);
    let links = vec![Link {
        url: pr.html_url.clone(),
        label: Some(format!("#{}", pr.number)),
    }];
    let mut entities = base_entities(event);
    entities.push(EntityRef {
        kind: EntityKind::GitHubPullRequest,
        external_id: external_id.clone(),
        label: Some(format!("#{}", pr.number)),
    });
    extend_with_ticket_keys(&mut entities, &pr.title);

    ActivityEvent {
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
        parent_external_id: Some(external_id),
        metadata: metadata_override.unwrap_or_else(|| {
            serde_json::json!({
                "github_event_id": event.id,
                "pr_number": pr.number,
                "repo": event.repo.name,
            })
        }),
        raw_ref: RawRef {
            storage_key: format!("github:event:{}", event.id),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

fn compose_issue_event(
    source_id: SourceId,
    event: &GithubEvent,
    kind: ActivityKind,
    issue: &GithubIssue,
) -> ActivityEvent {
    let external_id = format!("{}#{}", event.repo.name, issue.number);
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token(kind));

    let title = issue_title(kind, issue);
    let body = None;
    let actor = actor_from_event(event);
    let links = vec![Link {
        url: issue.html_url.clone(),
        label: Some(format!("#{}", issue.number)),
    }];
    let mut entities = base_entities(event);
    let entity_kind = if issue.is_pull_request() {
        EntityKind::GitHubPullRequest
    } else {
        EntityKind::GitHubIssue
    };
    entities.push(EntityRef {
        kind: entity_kind,
        external_id: external_id.clone(),
        label: Some(format!("#{}", issue.number)),
    });
    extend_with_ticket_keys(&mut entities, &issue.title);

    ActivityEvent {
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
        parent_external_id: Some(external_id),
        metadata: serde_json::json!({
            "github_event_id": event.id,
            "issue_number": issue.number,
            "repo": event.repo.name,
        }),
        raw_ref: RawRef {
            storage_key: format!("github:event:{}", event.id),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

fn base_entities(event: &GithubEvent) -> Vec<EntityRef> {
    let repo_label = event.repo.name.rsplit('/').next().map(|s| s.to_string());
    vec![EntityRef {
        kind: EntityKind::GitHubRepo,
        external_id: event.repo.name.clone(),
        label: repo_label,
    }]
}

fn extend_with_ticket_keys(entities: &mut Vec<EntityRef>, title: &str) {
    for key in ticket_keys(title)
        .into_iter()
        .take(MAX_TICKET_KEYS_PER_EVENT)
    {
        entities.push(EntityRef {
            kind: EntityKind::JiraIssue,
            external_id: key.clone(),
            label: Some(key),
        });
    }
}

/// Extract Jira-style ticket keys from a PR / issue title.
///
/// A key matches the shape `[A-Z][A-Z0-9]+-\d+` surrounded by
/// word-boundary characters — the same shape GitLab / Jira / GitHub
/// all accept in cross-linking. Extraction is left-to-right and each
/// unique key is emitted at most once.
///
/// We hand-roll the scan rather than pull in the `regex` crate —
/// this is the only pattern the connector needs and `regex`'s
/// compile-time footprint is heavyweight for a single anchored
/// shape (no other connector in the workspace uses `regex` today).
pub fn ticket_keys(title: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out: Vec<String> = Vec::new();

    let bytes = title.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Anchor on an uppercase ASCII letter that starts at a
        // non-word-character boundary (beginning of string or an
        // ASCII non-word char before it).
        let is_boundary = i == 0 || {
            let prev = bytes[i - 1];
            !is_ascii_word_char(prev)
        };
        if !is_boundary || !bytes[i].is_ascii_uppercase() {
            i += 1;
            continue;
        }

        // Consume the letters-or-digits prefix (must have at least
        // two uppercase/digit chars total: `[A-Z][A-Z0-9]+`).
        let prefix_start = i;
        let mut j = i + 1;
        while j < bytes.len() && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit()) {
            j += 1;
        }
        if j - prefix_start < 2 || j >= bytes.len() || bytes[j] != b'-' {
            i = j.max(i + 1);
            continue;
        }

        // Expect the hyphen then one-or-more digits.
        let digits_start = j + 1;
        let mut k = digits_start;
        while k < bytes.len() && bytes[k].is_ascii_digit() {
            k += 1;
        }
        if k == digits_start {
            i = j + 1;
            continue;
        }

        // Require a trailing non-word boundary so `ABC-12A` doesn't
        // match `ABC-12`.
        let trails_cleanly = k == bytes.len() || !is_ascii_word_char(bytes[k]);
        if !trails_cleanly {
            i = k;
            continue;
        }

        // Also require the prefix to start with a letter: we've
        // already checked `bytes[i]` is uppercase, but the `[A-Z][A-Z0-9]+`
        // shape means the prefix can't be *all* digits. Reject
        // ambiguous all-caps-digit prefixes like `Z9-1` only if the
        // first non-anchor char is also a digit — the anchor already
        // enforces a leading letter, so no extra work here.
        let key = std::str::from_utf8(&bytes[prefix_start..k])
            .expect("ASCII slice stays UTF-8")
            .to_string();
        if seen.insert(key.clone()) {
            out.push(key);
        }
        i = k;
    }

    out
}

fn is_ascii_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn actor_from_event(event: &GithubEvent) -> Actor {
    Actor {
        display_name: event.actor.login.clone(),
        email: None,
        external_id: Some(event.actor.id.to_string()),
    }
}

fn pr_title(kind: ActivityKind, pr: &GithubPullRequest) -> String {
    match kind {
        ActivityKind::GitHubPullRequestOpened => format!("Opened PR: {}", pr.title),
        ActivityKind::GitHubPullRequestMerged => format!("Merged PR: {}", pr.title),
        ActivityKind::GitHubPullRequestClosed => format!("Closed PR: {}", pr.title),
        ActivityKind::GitHubPullRequestReviewed => format!("Reviewed PR: {}", pr.title),
        ActivityKind::GitHubPullRequestCommented => format!("Commented on PR: {}", pr.title),
        _ => pr.title.clone(),
    }
}

fn issue_title(kind: ActivityKind, issue: &GithubIssue) -> String {
    match kind {
        ActivityKind::GitHubIssueOpened => format!("Opened issue: {}", issue.title),
        ActivityKind::GitHubIssueClosed => format!("Closed issue: {}", issue.title),
        ActivityKind::GitHubIssueCommented => format!("Commented on issue: {}", issue.title),
        ActivityKind::GitHubIssueAssigned => format!("Assigned issue: {}", issue.title),
        ActivityKind::GitHubPullRequestCommented => format!("Commented on PR: {}", issue.title),
        _ => issue.title.clone(),
    }
}

/// Convenience: string token used by
/// [`ActivityEvent::deterministic_id`]. Kept in one place so the id
/// scheme cannot drift from the enum's name.
pub(crate) fn kind_token(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::GitHubPullRequestOpened => "GitHubPullRequestOpened",
        ActivityKind::GitHubPullRequestMerged => "GitHubPullRequestMerged",
        ActivityKind::GitHubPullRequestClosed => "GitHubPullRequestClosed",
        ActivityKind::GitHubPullRequestReviewed => "GitHubPullRequestReviewed",
        ActivityKind::GitHubPullRequestCommented => "GitHubPullRequestCommented",
        ActivityKind::GitHubIssueOpened => "GitHubIssueOpened",
        ActivityKind::GitHubIssueClosed => "GitHubIssueClosed",
        ActivityKind::GitHubIssueCommented => "GitHubIssueCommented",
        ActivityKind::GitHubIssueAssigned => "GitHubIssueAssigned",
        // Non-GitHub kinds cannot reach this normaliser: every emit
        // path above produces a GitHub variant. A stray
        // non-GitHub kind reaching here would be a programmer bug,
        // not user data, so panicking is the right signal.
        ActivityKind::CommitAuthored
        | ActivityKind::MrOpened
        | ActivityKind::MrMerged
        | ActivityKind::MrClosed
        | ActivityKind::MrReviewComment
        | ActivityKind::MrApproved
        | ActivityKind::IssueOpened
        | ActivityKind::IssueClosed
        | ActivityKind::IssueComment
        | ActivityKind::JiraIssueTransitioned
        | ActivityKind::JiraIssueCommented
        | ActivityKind::JiraIssueAssigned
        | ActivityKind::JiraIssueUnassigned
        | ActivityKind::JiraIssueCreated
        | ActivityKind::ConfluencePageCreated
        | ActivityKind::ConfluencePageEdited
        | ActivityKind::ConfluenceComment => unreachable!(
            "GitHub normaliser saw non-GitHub ActivityKind {kind:?}: kind production is local \
             to this module",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use uuid::Uuid;

    use crate::events::{GithubActor, GithubRepo};

    fn source() -> SourceId {
        Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
    }

    fn actor() -> GithubActor {
        GithubActor {
            id: 17,
            login: "vedanth".into(),
            display_login: Some("vedanth".into()),
        }
    }

    fn repo() -> GithubRepo {
        GithubRepo {
            id: 99,
            name: "modulr/foo".into(),
            url: None,
        }
    }

    fn base_event(ev_type: &str, payload: serde_json::Value) -> GithubEvent {
        GithubEvent {
            id: "evt-1".into(),
            event_type: ev_type.into(),
            actor: actor(),
            repo: repo(),
            created_at: Utc.with_ymd_and_hms(2026, 4, 20, 12, 0, 0).unwrap(),
            payload,
        }
    }

    fn pr_payload(action: &str, merged: bool, title: &str, number: i64) -> serde_json::Value {
        serde_json::json!({
            "action": action,
            "number": number,
            "pull_request": {
                "id": 7000 + number,
                "number": number,
                "title": title,
                "html_url": format!("https://github.com/modulr/foo/pull/{number}"),
                "state": if merged { "closed" } else { "open" },
                "user": { "id": 17, "login": "vedanth" },
                "merged": merged,
                "draft": false
            }
        })
    }

    #[test]
    fn pr_opened_becomes_github_pull_request_opened() {
        let ev = base_event(
            "PullRequestEvent",
            pr_payload("opened", false, "Add payments slice", 42),
        );
        let out = normalise_event(source(), &ev).unwrap();
        assert_eq!(out.kind, ActivityKind::GitHubPullRequestOpened);
        assert_eq!(out.title, "Opened PR: Add payments slice");
        assert_eq!(out.external_id, "modulr/foo#42");
        assert_eq!(out.links[0].url, "https://github.com/modulr/foo/pull/42");
        let repo = out
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::GitHubRepo)
            .unwrap();
        assert_eq!(repo.external_id, "modulr/foo");
        assert_eq!(repo.label.as_deref(), Some("foo"));
    }

    #[test]
    fn pr_closed_with_merged_true_routes_to_merged_kind() {
        let ev = base_event(
            "PullRequestEvent",
            pr_payload("closed", true, "Ship payments slice", 42),
        );
        let out = normalise_event(source(), &ev).unwrap();
        assert_eq!(out.kind, ActivityKind::GitHubPullRequestMerged);
    }

    #[test]
    fn pr_closed_without_merged_routes_to_closed_kind() {
        let ev = base_event(
            "PullRequestEvent",
            pr_payload("closed", false, "Abandon experiment", 42),
        );
        let out = normalise_event(source(), &ev).unwrap();
        assert_eq!(out.kind, ActivityKind::GitHubPullRequestClosed);
    }

    #[test]
    fn pr_review_submitted_routes_to_reviewed_and_carries_state_in_metadata() {
        let payload = serde_json::json!({
            "action": "submitted",
            "pull_request": {
                "id": 7000,
                "number": 1,
                "title": "Review target",
                "html_url": "https://github.com/modulr/foo/pull/1",
                "state": "open",
                "merged": false,
                "draft": false
            },
            "review": {
                "id": 555,
                "state": "approved",
                "user": { "id": 17, "login": "vedanth" },
                "body": "LGTM",
                "submitted_at": "2026-04-20T12:00:00Z"
            }
        });
        let ev = base_event("PullRequestReviewEvent", payload);
        let out = normalise_event(source(), &ev).unwrap();
        assert_eq!(out.kind, ActivityKind::GitHubPullRequestReviewed);
        assert_eq!(
            out.metadata.get("review_state").and_then(|v| v.as_str()),
            Some("approved")
        );
        assert_eq!(
            out.metadata.get("review_count").and_then(|v| v.as_i64()),
            Some(1)
        );
    }

    #[test]
    fn issue_comment_on_pr_routes_to_pull_request_commented() {
        let payload = serde_json::json!({
            "action": "created",
            "issue": {
                "id": 11,
                "number": 42,
                "title": "Add payments",
                "html_url": "https://github.com/modulr/foo/pull/42",
                "state": "open",
                "pull_request": { "url": "..." }
            },
            "comment": {
                "id": 321,
                "body": "LGTM",
                "user": { "id": 17, "login": "vedanth" }
            }
        });
        let ev = base_event("IssueCommentEvent", payload);
        let out = normalise_event(source(), &ev).unwrap();
        assert_eq!(out.kind, ActivityKind::GitHubPullRequestCommented);
        // The nested entity must be a GitHubPullRequest, not GitHubIssue.
        assert!(out
            .entities
            .iter()
            .any(|e| e.kind == EntityKind::GitHubPullRequest));
    }

    #[test]
    fn issue_comment_on_plain_issue_routes_to_issue_commented() {
        let payload = serde_json::json!({
            "action": "created",
            "issue": {
                "id": 11,
                "number": 7,
                "title": "Paginator 404s on empty inputs",
                "html_url": "https://github.com/modulr/foo/issues/7",
                "state": "open"
            },
            "comment": {
                "id": 321,
                "body": "Seeing this on ...",
                "user": { "id": 17, "login": "vedanth" }
            }
        });
        let ev = base_event("IssueCommentEvent", payload);
        let out = normalise_event(source(), &ev).unwrap();
        assert_eq!(out.kind, ActivityKind::GitHubIssueCommented);
        assert!(out
            .entities
            .iter()
            .any(|e| e.kind == EntityKind::GitHubIssue));
    }

    #[test]
    fn issues_event_opened_closed_route_correctly() {
        for (action, expected) in [
            ("opened", ActivityKind::GitHubIssueOpened),
            ("closed", ActivityKind::GitHubIssueClosed),
        ] {
            let payload = serde_json::json!({
                "action": action,
                "issue": {
                    "id": 11,
                    "number": 7,
                    "title": "A bug",
                    "html_url": "https://github.com/modulr/foo/issues/7",
                    "state": if action == "opened" { "open" } else { "closed" }
                }
            });
            let ev = base_event("IssuesEvent", payload);
            let out = normalise_event(source(), &ev).unwrap();
            assert_eq!(out.kind, expected);
        }
    }

    /// The "assigned to self" self-filter must hold — an issue
    /// assigned to a teammate by the user is not surfaced.
    #[test]
    fn issues_event_assigned_only_surfaces_when_assignee_is_self() {
        let self_assigned = serde_json::json!({
            "action": "assigned",
            "issue": {
                "id": 11,
                "number": 7,
                "title": "A bug",
                "html_url": "https://github.com/modulr/foo/issues/7",
                "state": "open"
            },
            "assignee": { "id": 17, "login": "vedanth" }
        });
        let ev = base_event("IssuesEvent", self_assigned);
        let out = normalise_event(source(), &ev).unwrap();
        assert_eq!(out.kind, ActivityKind::GitHubIssueAssigned);

        let teammate_assigned = serde_json::json!({
            "action": "assigned",
            "issue": {
                "id": 11,
                "number": 7,
                "title": "A bug",
                "html_url": "https://github.com/modulr/foo/issues/7",
                "state": "open"
            },
            "assignee": { "id": 42, "login": "someone-else" }
        });
        let ev = base_event("IssuesEvent", teammate_assigned);
        assert!(normalise_event(source(), &ev).is_none());
    }

    /// Push events are deliberately dropped (v0.4 non-goal —
    /// commits surface through the local-git pipeline).
    #[test]
    fn push_event_is_deliberately_dropped() {
        let ev = base_event("PushEvent", serde_json::json!({}));
        assert!(normalise_event(source(), &ev).is_none());
    }

    /// Unknown event types drop silently — the walker keeps going.
    #[test]
    fn unknown_event_type_is_dropped_not_errored() {
        let ev = base_event("ForkEvent", serde_json::json!({}));
        assert!(normalise_event(source(), &ev).is_none());
    }

    /// PR title carrying a Jira-style ticket key seeds an
    /// `EntityKind::JiraIssue` — the enrichment hook DAY-97 reads
    /// on the report side. Tokens are extracted in left-to-right
    /// order, deduplicated.
    #[test]
    fn pr_title_with_jira_ticket_key_enriches_entity_list() {
        let ev = base_event(
            "PullRequestEvent",
            pr_payload("opened", false, "CAR-5117: plumb charge reasons", 42),
        );
        let out = normalise_event(source(), &ev).unwrap();
        let jira = out
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::JiraIssue)
            .expect("jira ticket key must seed JiraIssue entity");
        assert_eq!(jira.external_id, "CAR-5117");
    }

    #[test]
    fn ticket_keys_extracts_unique_tokens_in_order() {
        let keys = ticket_keys("CAR-5117 bugfix; see also PROJ-12 and CAR-5117 again");
        assert_eq!(keys, vec!["CAR-5117", "PROJ-12"]);
    }

    #[test]
    fn ticket_keys_ignores_lowercase_or_kebab_branches() {
        // A branch name like `feat-15` is not a ticket key; neither
        // is `foo-bar-5`.
        let keys = ticket_keys("feat-15 and foo-bar-5 but not real keys");
        assert!(keys.is_empty());
    }

    /// Normalisation is deterministic — same input, same
    /// `ActivityEvent` (byte-identical).
    #[test]
    fn normalisation_is_deterministic() {
        let ev = base_event("PullRequestEvent", pr_payload("opened", false, "Title", 42));
        let a = normalise_event(source(), &ev).unwrap();
        let b = normalise_event(source(), &ev).unwrap();
        assert_eq!(a, b);
    }
}
