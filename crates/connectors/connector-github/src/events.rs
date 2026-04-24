//! Serde DTOs for the GitHub REST event + search shapes the walker
//! consumes.
//!
//! Two request surfaces, two payload shapes:
//!
//! 1. `GET /users/{login}/events` — returns an array of
//!    [`GithubEvent`] rows, each a thin envelope (`type`, `actor`,
//!    `repo`, `created_at`) around a `payload` whose shape depends
//!    on the event type. We decode the envelope strictly and fan
//!    `payload` into [`GithubEventPayload`] arms per `type`. Unknown
//!    or malformed types fall through to
//!    [`GithubEventPayload::Unknown`] so a single new event family
//!    GitHub rolls out after this code freezes does not kill the
//!    walk.
//! 2. `GET /search/issues` — returns a wrapped payload
//!    ([`GithubSearchPage`]) with an `items` array of
//!    [`GithubSearchIssue`]. Each search-issue represents either a
//!    PR (when `pull_request` is present) or an issue.
//!
//! All timestamps land as `DateTime<Utc>`. All numeric ids land as
//! `i64` — GitHub returns everything (user, issue, comment, review,
//! commit_id) as decimal scalars and `i64` is the workspace
//! convention for upstream actor ids ([`connector_gitlab::events`]).
//!
//! Fields we never read stay out of the DTOs deliberately; adding a
//! field later is additive (`serde` tolerates unknown JSON by
//! default), removing one is not.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One row of `GET /users/{login}/events`. The `type` tag drives the
/// variant in [`GithubEventPayload`]; the envelope fields are shared
/// across every event kind.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubEvent {
    /// GitHub's numeric event id serialised as a string in the event
    /// payload (confusingly — every other id is numeric). Kept as
    /// `String` because that's the over-the-wire shape.
    pub id: String,
    /// The GitHub event type discriminator: `PushEvent`,
    /// `PullRequestEvent`, `IssuesEvent`, `IssueCommentEvent`,
    /// `PullRequestReviewEvent`, `PullRequestReviewCommentEvent`, …
    #[serde(rename = "type")]
    pub event_type: String,
    pub actor: GithubActor,
    pub repo: GithubRepo,
    pub created_at: DateTime<Utc>,
    /// Strongly-typed payload. Decoded lazily via
    /// [`GithubEventPayload::from_raw`] so the outer envelope can
    /// deserialise even when a single row's payload has drifted.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// The event actor — the user (or, in rare cases, a bot) that
/// triggered the event. We only need `id` + `login`; `display_login`
/// and `gravatar_id` are surfaced by the GitHub UI but not by
/// Dayseam's EOD narrative.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubActor {
    pub id: i64,
    pub login: String,
    #[serde(default)]
    pub display_login: Option<String>,
}

/// The repository the event occurred in. `name` is in `owner/repo`
/// form, which [`connector_github::normalise`] uses to compose the
/// `EntityKind::GitHubRepo` external id.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubRepo {
    pub id: i64,
    pub name: String,
    /// The API URL for the repo — `https://api.github.com/repos/{owner}/{name}`.
    /// Kept as `String` because we never reach back through it.
    #[serde(default)]
    pub url: Option<String>,
}

/// Fan-out of the event `payload` field per `event.type`.
///
/// Each variant captures just the fields the normaliser needs — we
/// deliberately do not mirror the entire upstream payload, both to
/// keep the DTO surface narrow and because GitHub adds fields to
/// payloads without bumping `X-GitHub-Api-Version`. Unknown-shape
/// rows land in [`Self::Unknown`] carrying the raw JSON plus a
/// `reason` string so the walker can log + drop without a hard
/// failure.
#[derive(Clone)]
pub enum GithubEventPayload {
    /// `PullRequestEvent`. `action` is one of `opened`, `closed`,
    /// `reopened`, `edited`, `assigned`, …; only `opened` and
    /// `closed` make it to the normaliser. `merged` is only
    /// meaningful when `action == "closed"`.
    PullRequest {
        action: String,
        number: i64,
        pull_request: GithubPullRequest,
    },
    /// `PullRequestReviewEvent`. `action` is typically `submitted`
    /// or `dismissed`; only `submitted` drives an `ActivityEvent`.
    PullRequestReview {
        action: String,
        pull_request: GithubPullRequest,
        review: GithubReview,
    },
    /// `PullRequestReviewCommentEvent`. A comment on a single diff
    /// line inside a PR review thread. Distinct from
    /// `IssueCommentEvent` because issue comments render at the
    /// PR's conversation level, not inside a review.
    PullRequestReviewComment {
        action: String,
        pull_request: GithubPullRequest,
        comment: GithubComment,
    },
    /// `IssuesEvent`. `action` routes to
    /// `GitHubIssue{Opened,Closed,Assigned}`.
    Issues {
        action: String,
        issue: GithubIssue,
        #[doc = "Present only when `action == \"assigned\"` or `\"unassigned\"`."]
        assignee: Option<GithubUserRef>,
    },
    /// `IssueCommentEvent`. Splits into PR-commented vs
    /// issue-commented at the normaliser by inspecting
    /// `issue.pull_request`.
    IssueComment {
        action: String,
        issue: GithubIssue,
        comment: GithubComment,
    },
    /// `PushEvent`. We decode the envelope for completeness but
    /// deliberately do **not** emit an `ActivityEvent` for it —
    /// commits render through the local-git pipeline. See
    /// `docs/plan/2026-04-22-v0.4-github-connector.md` Task 5 for
    /// the non-goal rationale.
    Push,
    /// Anything we did not wire an arm for: `CreateEvent`,
    /// `DeleteEvent`, `ForkEvent`, `WatchEvent`, `GollumEvent`, etc.
    /// Also the landing pad for a wire-format drift in a payload
    /// whose `type` we recognise — the walker logs + drops.
    Unknown { reason: String },
}

impl GithubEventPayload {
    /// Decode the payload JSON using the event type tag.
    ///
    /// Unknown-type events return `Ok(Unknown)` rather than `Err` so
    /// the walker can keep making forward progress. Shape-drift on a
    /// known type (e.g. `PullRequestEvent` without a `pull_request`
    /// key) also lands in `Unknown { reason }` — we never error the
    /// whole walk for a single malformed row.
    pub fn from_raw(event_type: &str, raw: &serde_json::Value) -> Self {
        match event_type {
            "PullRequestEvent" => match serde_json::from_value::<PullRequestPayload>(raw.clone()) {
                Ok(p) => GithubEventPayload::PullRequest {
                    action: p.action,
                    number: p.number,
                    pull_request: p.pull_request,
                },
                Err(e) => GithubEventPayload::Unknown {
                    reason: format!("PullRequestEvent payload shape changed: {e}"),
                },
            },
            "PullRequestReviewEvent" => {
                match serde_json::from_value::<PullRequestReviewPayload>(raw.clone()) {
                    Ok(p) => GithubEventPayload::PullRequestReview {
                        action: p.action,
                        pull_request: p.pull_request,
                        review: p.review,
                    },
                    Err(e) => GithubEventPayload::Unknown {
                        reason: format!("PullRequestReviewEvent payload shape changed: {e}"),
                    },
                }
            }
            "PullRequestReviewCommentEvent" => {
                match serde_json::from_value::<PullRequestReviewCommentPayload>(raw.clone()) {
                    Ok(p) => GithubEventPayload::PullRequestReviewComment {
                        action: p.action,
                        pull_request: p.pull_request,
                        comment: p.comment,
                    },
                    Err(e) => GithubEventPayload::Unknown {
                        reason: format!("PullRequestReviewCommentEvent payload shape changed: {e}"),
                    },
                }
            }
            "IssuesEvent" => match serde_json::from_value::<IssuesPayload>(raw.clone()) {
                Ok(p) => GithubEventPayload::Issues {
                    action: p.action,
                    issue: p.issue,
                    assignee: p.assignee,
                },
                Err(e) => GithubEventPayload::Unknown {
                    reason: format!("IssuesEvent payload shape changed: {e}"),
                },
            },
            "IssueCommentEvent" => match serde_json::from_value::<IssueCommentPayload>(raw.clone())
            {
                Ok(p) => GithubEventPayload::IssueComment {
                    action: p.action,
                    issue: p.issue,
                    comment: p.comment,
                },
                Err(e) => GithubEventPayload::Unknown {
                    reason: format!("IssueCommentEvent payload shape changed: {e}"),
                },
            },
            "PushEvent" => GithubEventPayload::Push,
            other => GithubEventPayload::Unknown {
                reason: format!("unhandled GitHub event type: {other}"),
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestPayload {
    action: String,
    number: i64,
    pull_request: GithubPullRequest,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestReviewPayload {
    action: String,
    pull_request: GithubPullRequest,
    review: GithubReview,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestReviewCommentPayload {
    action: String,
    pull_request: GithubPullRequest,
    comment: GithubComment,
}

#[derive(Debug, Clone, Deserialize)]
struct IssuesPayload {
    action: String,
    issue: GithubIssue,
    #[serde(default)]
    assignee: Option<GithubUserRef>,
}

#[derive(Debug, Clone, Deserialize)]
struct IssueCommentPayload {
    action: String,
    issue: GithubIssue,
    comment: GithubComment,
}

/// The PR object embedded in `PullRequestEvent.payload.pull_request`
/// and in `PullRequestReviewEvent.payload.pull_request`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubPullRequest {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    #[serde(default)]
    pub user: Option<GithubUserRef>,
    /// `true` iff the PR has been merged. Populated on
    /// `PullRequestEvent.action: "closed"` so the normaliser can
    /// route `closed + merged=true` → `GitHubPullRequestMerged`
    /// and `closed + merged=false` → `GitHubPullRequestClosed`.
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub draft: bool,
}

/// The issue object embedded in `IssuesEvent.payload.issue` and in
/// `IssueCommentEvent.payload.issue`. When `pull_request` is
/// present, the "issue" is actually a PR — GitHub serves PRs through
/// the issues API for historical reasons.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubIssue {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    #[serde(default)]
    pub user: Option<GithubUserRef>,
    /// Presence indicates this "issue" is a PR. The nested object
    /// carries `html_url`/`url`/`merged_at`; for the normaliser we
    /// only care whether the field exists at all.
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
}

impl GithubIssue {
    /// Whether this row is actually a PR served through the issues
    /// API. Used by the normaliser to route `IssueCommentEvent` on a
    /// PR to `GitHubPullRequestCommented` and the same on a true
    /// issue to `GitHubIssueCommented`.
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }
}

/// A PR review payload (`payload.review`) — we only read `id`,
/// `user`, `state`, `body`, `submitted_at`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubReview {
    pub id: i64,
    /// `approved`, `changes_requested`, `commented`, `dismissed`, …
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub user: Option<GithubUserRef>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub submitted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub html_url: Option<String>,
}

/// A comment payload — PR review comments, PR conversation
/// comments, issue comments. The GitHub API shape is shared across
/// all three.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubComment {
    pub id: i64,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub user: Option<GithubUserRef>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

/// Nested user reference — every `user` / `assignee` / `author`
/// payload we touch has the same shape. A thin subset of the full
/// GitHub User object; more fields stay off-DTO so renames upstream
/// don't cascade here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubUserRef {
    pub id: i64,
    pub login: String,
    #[serde(default)]
    pub html_url: Option<String>,
}

/// Shape of `GET /search/issues`. `items` is the only field we
/// consume, but `incomplete_results` is captured so the caller can
/// downgrade confidence when GitHub couldn't search the full corpus
/// inside its SLA.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubSearchPage {
    #[serde(default)]
    pub total_count: i64,
    #[serde(default)]
    pub incomplete_results: bool,
    #[serde(default)]
    pub items: Vec<GithubSearchIssue>,
}

/// One row of `GET /search/issues`. Shape-wise an amplified
/// [`GithubIssue`]: it carries the same `html_url`, `state`, `user`,
/// `number`, `pull_request` markers, plus `updated_at` we key dedup
/// off. The walker converts each search-hit into a synthetic
/// [`GithubEvent`]-equivalent and feeds it to the normaliser — see
/// [`crate::walk`] for the conversion.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GithubSearchIssue {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    #[serde(default)]
    pub user: Option<GithubUserRef>,
    #[serde(default)]
    pub assignees: Vec<GithubUserRef>,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    /// Only populated on the PR-shaped search hits. We never read
    /// this field; its presence is a marker the walker uses to route
    /// the hit through the PR-state inference path.
    #[serde(default)]
    pub repository_url: Option<String>,
}

impl GithubSearchIssue {
    /// Whether this row describes a PR. Mirrors
    /// [`GithubIssue::is_pull_request`].
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }

    /// `owner/repo` extracted from the `repository_url` form
    /// `https://api.github.com/repos/{owner}/{repo}`. Returns
    /// `None` when the URL is missing or not of the expected shape;
    /// callers degrade to a synthetic `repo-<id>` key in that case.
    pub fn repo_full_name(&self) -> Option<String> {
        let url = self.repository_url.as_deref()?;
        let marker = "/repos/";
        let idx = url.find(marker)?;
        let tail = &url[idx + marker.len()..];
        let tail = tail.trim_end_matches('/');
        if tail.is_empty() || !tail.contains('/') {
            return None;
        }
        Some(tail.to_string())
    }
}

/// Fmt helper so tests can `panic!("got {other:?}")` on
/// [`GithubEventPayload`] without derive-expansion worries.
impl core::fmt::Debug for GithubEventPayload {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GithubEventPayload::PullRequest {
                action,
                number,
                pull_request,
            } => f
                .debug_struct("PullRequest")
                .field("action", action)
                .field("number", number)
                .field("pull_request.title", &pull_request.title)
                .finish(),
            GithubEventPayload::PullRequestReview { action, review, .. } => f
                .debug_struct("PullRequestReview")
                .field("action", action)
                .field("review.state", &review.state)
                .finish(),
            GithubEventPayload::PullRequestReviewComment { action, .. } => f
                .debug_struct("PullRequestReviewComment")
                .field("action", action)
                .finish(),
            GithubEventPayload::Issues {
                action,
                issue,
                assignee,
            } => f
                .debug_struct("Issues")
                .field("action", action)
                .field("issue.number", &issue.number)
                .field(
                    "assignee.login",
                    &assignee.as_ref().map(|a| a.login.as_str()),
                )
                .finish(),
            GithubEventPayload::IssueComment { action, issue, .. } => f
                .debug_struct("IssueComment")
                .field("action", action)
                .field("issue.number", &issue.number)
                .field("issue.is_pr", &issue.is_pull_request())
                .finish(),
            GithubEventPayload::Push => f.write_str("Push"),
            GithubEventPayload::Unknown { reason } => {
                f.debug_struct("Unknown").field("reason", reason).finish()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `PullRequestEvent` decodes envelope + payload both
    /// strictly (outer) and lazily (payload).
    #[test]
    fn pull_request_event_round_trips() {
        let raw = serde_json::json!({
            "id": "12345",
            "type": "PullRequestEvent",
            "actor": { "id": 17, "login": "vedanth" },
            "repo": { "id": 99, "name": "company/foo" },
            "created_at": "2026-04-20T10:00:00Z",
            "payload": {
                "action": "opened",
                "number": 42,
                "pull_request": {
                    "id": 777,
                    "number": 42,
                    "title": "Add payments slice",
                    "html_url": "https://github.com/company/foo/pull/42",
                    "state": "open",
                    "user": { "id": 17, "login": "vedanth" },
                    "merged": false,
                    "draft": false
                }
            }
        });
        let ev: GithubEvent = serde_json::from_value(raw).unwrap();
        assert_eq!(ev.event_type, "PullRequestEvent");
        assert_eq!(ev.actor.login, "vedanth");
        assert_eq!(ev.repo.name, "company/foo");

        let payload = GithubEventPayload::from_raw(&ev.event_type, &ev.payload);
        match payload {
            GithubEventPayload::PullRequest {
                action,
                number,
                pull_request,
            } => {
                assert_eq!(action, "opened");
                assert_eq!(number, 42);
                assert_eq!(pull_request.title, "Add payments slice");
                assert!(!pull_request.merged);
            }
            other => panic!("expected PullRequest payload, got {other:?}"),
        }
    }

    /// An unknown event `type` does not fail the decode; the walker
    /// logs + drops.
    #[test]
    fn unknown_event_type_decodes_to_unknown_variant() {
        let raw = serde_json::json!({});
        let payload = GithubEventPayload::from_raw("ForkEvent", &raw);
        match payload {
            GithubEventPayload::Unknown { reason } => {
                assert!(reason.contains("ForkEvent"));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// A known `type` whose payload drifted (e.g. missing
    /// `pull_request` key) also lands in `Unknown` rather than
    /// erroring the whole walk.
    #[test]
    fn malformed_known_payload_lands_in_unknown_not_error() {
        let raw = serde_json::json!({
            "action": "opened",
            "number": 1
        });
        let payload = GithubEventPayload::from_raw("PullRequestEvent", &raw);
        match payload {
            GithubEventPayload::Unknown { reason } => {
                assert!(
                    reason.contains("PullRequestEvent payload shape changed"),
                    "reason={reason}"
                );
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// An issue with a `pull_request` marker is_pull_request()==true;
    /// the normaliser routes `IssueCommentEvent` with this issue
    /// shape to `GitHubPullRequestCommented`.
    #[test]
    fn issue_with_pull_request_field_is_recognised_as_pr() {
        let issue: GithubIssue = serde_json::from_value(serde_json::json!({
            "id": 1,
            "number": 42,
            "title": "A PR-flavoured issue",
            "html_url": "https://github.com/foo/bar/pull/42",
            "state": "open",
            "pull_request": { "url": "..." }
        }))
        .unwrap();
        assert!(issue.is_pull_request());

        let issue: GithubIssue = serde_json::from_value(serde_json::json!({
            "id": 1,
            "number": 7,
            "title": "Real issue",
            "html_url": "https://github.com/foo/bar/issues/7",
            "state": "open"
        }))
        .unwrap();
        assert!(!issue.is_pull_request());
    }

    #[test]
    fn search_issue_extracts_owner_repo_from_repository_url() {
        let hit = GithubSearchIssue {
            id: 1,
            number: 1,
            title: "t".into(),
            html_url: "https://github.com/foo/bar/issues/1".into(),
            state: "open".into(),
            user: None,
            assignees: vec![],
            pull_request: None,
            created_at: None,
            updated_at: None,
            closed_at: None,
            repository_url: Some("https://api.github.com/repos/foo/bar".into()),
        };
        assert_eq!(hit.repo_full_name().as_deref(), Some("foo/bar"));

        let hit_bad = GithubSearchIssue {
            repository_url: Some("https://other.example/no-repos-marker".into()),
            ..hit.clone()
        };
        assert!(hit_bad.repo_full_name().is_none());

        let hit_none = GithubSearchIssue {
            repository_url: None,
            ..hit
        };
        assert!(hit_none.repo_full_name().is_none());
    }
}
