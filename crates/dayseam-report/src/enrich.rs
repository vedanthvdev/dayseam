//! Cross-source enrichment pipeline.
//!
//! Three passes, all pure and idempotent:
//!
//! 1. [`extract_ticket_keys`] scans every event's `title` + `body`
//!    for Jira-shaped ticket keys (`/\b[A-Z]{2,10}-\d+\b/`) and
//!    attaches a `jira_issue` [`EntityRef`] as a `target` on the
//!    event. A GitLab MR titled `"CAR-5117: Fix review findings"`
//!    gets a `jira_issue` entity pointing at `CAR-5117` with **zero
//!    Jira API calls**. Downstream passes use this to cross-link MRs
//!    to Jira transitions.
//! 2. [`extract_github_pr_urls`] scans GitLab MR events' `title` +
//!    `body` for `https://github.com/<owner>/<repo>/pull/<N>` URLs
//!    and attaches a `github_pull_request` [`EntityRef`] whose
//!    `external_id` matches the GitHub connector's
//!    `"{repo}#{number}"` shape, so a single day's events
//!    cross-reference GitLab MRs and the GitHub PRs they mention.
//!    DAY-97.
//! 3. [`annotate_transition_with_mr`] walks `JiraIssueTransitioned`
//!    events and, for each one, looks up whether the day also has a
//!    matching MR-like event — a GitLab `MrOpened` / `MrMerged` or
//!    a GitHub `GitHubPullRequestOpened` / `GitHubPullRequestMerged`
//!    / `GitHubPullRequestClosed` — with a `jira_issue` target
//!    pointing at the same issue key. When it does, the transition's
//!    `parent_external_id` is set to the MR/PR's `external_id`, so
//!    the verbose-mode render can show `(triggered by !321)` or
//!    `(triggered by #42)` next to a status change. DAY-97
//!    generalises the DAY-78 GitLab-only pipeline and lands the
//!    previously-documented-but-unimplemented 24h temporal guard:
//!    the triggering MR/PR must *precede* the transition and be
//!    within 24 hours of it. A PR opened *after* a transition can't
//!    have triggered it, and an MR merged a week earlier is too far
//!    out to credibly claim authorship of today's status change.
//!
//! # Why regex-free
//!
//! The ticket-key pattern is simple enough that a hand-rolled
//! ASCII scanner avoids pulling `regex` (and its 300 kLoC `regex-automata`
//! transitive) into the pure-function report crate. The report engine
//! is a hot path (the UI re-renders on every filter toggle) so a
//! dependency with a 2 MB binary footprint would be a net loss even
//! if we reused it elsewhere — which we don't: no other crate in the
//! workspace uses `regex`.
//!
//! # Noise bail
//!
//! Commit titles like `"Fix GH-123 by bumping LOG4J-2 from 2.17.0 to
//! 2.17.2"` contain tokens that syntactically match the pattern
//! (`LOG4J-2`) but semantically aren't Jira tickets. We can't
//! distinguish the two from the string alone, so
//! [`extract_ticket_keys`] bails when a single event surfaces more
//! than [`MAX_TICKET_KEYS_PER_EVENT`] candidates — the commit
//! probably references many tickets in a non-structured way and
//! we'd rather attach nothing than the wrong thing.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use dayseam_core::{ActivityEvent, ActivityKind, EntityKind, EntityRef};
use uuid::Uuid;

/// Bail threshold for [`extract_ticket_keys`]. See the module docs.
pub(crate) const MAX_TICKET_KEYS_PER_EVENT: usize = 3;

/// Maximum lookback for [`annotate_transition_with_mr`].
///
/// A triggering MR/PR must precede the transition by at most this
/// window. The cap is the same 24h threshold the v0.2 dogfood notes
/// called out as the "obviously too loose" band — a transition that
/// happens more than a day after the MR is more likely organic
/// triage than MR-driven. DAY-97 makes this a concrete guard
/// instead of the prose promise it was pre-v0.4.
pub(crate) const MR_TRIGGER_WINDOW: Duration = Duration::hours(24);

/// Attach a `jira_issue` [`EntityRef`] for every ticket key found in
/// each event's `title` and `body`.
///
/// Idempotent: a second call produces no new entities, because the
/// function checks for an existing `jira_issue` entity with a
/// matching `external_id` before pushing. Events that already carry
/// a `jira_issue` target (e.g. the Jira connector's own emissions)
/// are untouched.
///
/// Events with more than [`MAX_TICKET_KEYS_PER_EVENT`] unique
/// candidates are treated as noise and attached no entity — see the
/// module docs for the rationale.
pub fn extract_ticket_keys(events: &mut [ActivityEvent]) {
    for event in events.iter_mut() {
        let mut keys: Vec<String> = Vec::new();
        scan_ticket_keys(&event.title, &mut keys);
        if let Some(body) = &event.body {
            scan_ticket_keys(body, &mut keys);
        }
        keys.sort();
        keys.dedup();
        if keys.is_empty() || keys.len() > MAX_TICKET_KEYS_PER_EVENT {
            continue;
        }
        for key in keys {
            let already = event
                .entities
                .iter()
                .any(|e| e.kind == EntityKind::JiraIssue && e.external_id == key);
            if !already {
                event.entities.push(EntityRef {
                    kind: EntityKind::JiraIssue,
                    external_id: key,
                    label: None,
                });
            }
        }
    }
}

/// Annotate `JiraIssueTransitioned` events with the GitLab MR or
/// GitHub PR that (probably) triggered them.
///
/// Uses the `jira_issue` [`EntityRef`] that [`extract_ticket_keys`]
/// attaches to MRs and PRs: an `MrOpened` / `MrMerged` / GitHub PR
/// event whose title carried the ticket key `CAR-5117` exposes a
/// `jira_issue` entity with `external_id = "CAR-5117"`. A
/// `JiraIssueTransitioned` event for `CAR-5117` then finds the
/// triggering MR/PR and stamps `parent_external_id =
/// Some(<mr_or_pr_external_id>)`.
///
/// # Selection
///
/// For each transition, the candidate MR/PR set is the MRs and PRs
/// on the same Jira issue whose `occurred_at` falls in
/// `[transition - MR_TRIGGER_WINDOW, transition]` — i.e. the MR/PR
/// must **precede** the transition by at most 24 hours. This is the
/// DAY-88 temporal guard generalised to cross-source: a PR opened
/// *after* a transition can't have triggered it, and an MR merged
/// a week earlier is too stale to credibly claim authorship of
/// today's status change. The chosen MR/PR within that window is
/// the earliest one, tie-broken by `ActivityEvent::id` (UUIDv5 from
/// `(source_id, external_id, kind)`) so swapping walker output
/// order never flips the pick.
///
/// # Provider-agnostic
///
/// "MR-like" is a closed set of activity kinds:
///
/// * GitLab: [`ActivityKind::MrOpened`], [`ActivityKind::MrMerged`].
/// * GitHub (DAY-97): [`ActivityKind::GitHubPullRequestOpened`],
///   [`ActivityKind::GitHubPullRequestMerged`],
///   [`ActivityKind::GitHubPullRequestClosed`]. Review and comment
///   PR events are deliberately excluded — reviewing a PR doesn't
///   imply authorship of the code that triggered the transition.
///
/// # Behaviour
///
/// Overwrites any existing `parent_external_id` on the transition
/// when a trigger is found. DAY-77's Jira connector populates
/// `parent_external_id` with the issue key for routing purposes,
/// but the issue key is also in the event's `entities` list — the
/// field is free to repurpose here.
///
/// No-op on events that aren't `JiraIssueTransitioned`.
/// No-op on transitions whose issue key has no matching MR/PR in
/// the window.
pub fn annotate_transition_with_mr(events: &mut [ActivityEvent]) {
    let candidates = build_issue_to_mr_candidates(events);
    if candidates.is_empty() {
        return;
    }
    for event in events.iter_mut() {
        if event.kind != ActivityKind::JiraIssueTransitioned {
            continue;
        }
        let Some(issue_key) = event
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::JiraIssue)
            .map(|e| e.external_id.clone())
        else {
            continue;
        };
        let Some(pool) = candidates.get(issue_key.as_str()) else {
            continue;
        };
        let transition_at = event.occurred_at;
        let window_start = transition_at - MR_TRIGGER_WINDOW;
        // Pick the earliest MR/PR in `[window_start, transition_at]`
        // on `(occurred_at, id)` order. Running the filter per
        // transition (rather than once at index-build time) is what
        // lets a single MR cleanly trigger same-day *and* next-day
        // transitions while a far-back MR stops being credited on
        // day two.
        let winner = pool
            .iter()
            .filter(|c| c.occurred_at >= window_start && c.occurred_at <= transition_at)
            .min_by_key(|c| (c.occurred_at, c.id));
        if let Some(c) = winner {
            event.parent_external_id = Some(c.external_id.clone());
        }
    }
}

/// One MR/PR candidate for `annotate_transition_with_mr`'s
/// per-transition window search.
///
/// Owned strings so the caller can freely `iter_mut()` the event
/// vec after we return — tying `&str` references to the `events`
/// borrow would conflict with the subsequent mutable walk.
#[derive(Debug, Clone)]
struct MrCandidate {
    occurred_at: DateTime<Utc>,
    id: Uuid,
    external_id: String,
}

/// Build an issue-key → candidate-MRs/PRs index.
///
/// Every MR/PR is a candidate for every same-issue transition; the
/// per-transition window filter inside
/// [`annotate_transition_with_mr`] picks the winner.
fn build_issue_to_mr_candidates(events: &[ActivityEvent]) -> HashMap<String, Vec<MrCandidate>> {
    let mut out: HashMap<String, Vec<MrCandidate>> = HashMap::new();
    for event in events {
        if !is_mr_like(event.kind) {
            continue;
        }
        for ent in &event.entities {
            if ent.kind != EntityKind::JiraIssue {
                continue;
            }
            out.entry(ent.external_id.clone())
                .or_default()
                .push(MrCandidate {
                    occurred_at: event.occurred_at,
                    id: event.id,
                    external_id: event.external_id.clone(),
                });
        }
    }
    out
}

/// Kinds treated as "MR-like" trigger candidates by
/// [`annotate_transition_with_mr`].
///
/// Kept local so the list stays next to its usage and adding a new
/// provider's MR-opening kind is a one-line change. Review and
/// comment events are excluded — reviewing a PR doesn't imply
/// authorship of the code that triggered the transition.
fn is_mr_like(kind: ActivityKind) -> bool {
    matches!(
        kind,
        ActivityKind::MrOpened
            | ActivityKind::MrMerged
            | ActivityKind::GitHubPullRequestOpened
            | ActivityKind::GitHubPullRequestMerged
            | ActivityKind::GitHubPullRequestClosed
    )
}

/// Scan GitLab MR events for cross-linked GitHub PR URLs and
/// attach `EntityKind::GitHubPullRequest` entities pointing at
/// them.
///
/// An MR description like
/// `"Mirrors https://github.com/vedanthvdev/dayseam/pull/42"` produces
/// a `github_pull_request` entity with
/// `external_id = "dayseam#42"` — the same shape the GitHub
/// connector emits in [`crate::group_key`] and on its own PR events.
/// Downstream the render layer treats the cross-linked PR the same
/// as a native one, and a future "PR↔MR diff" view can key off
/// the entity to show the two artifacts side-by-side.
///
/// # Scope
///
/// Only scans GitLab MR-shaped events today
/// ([`ActivityKind::MrOpened`] / [`ActivityKind::MrMerged`]).
/// GitHub PR events don't need this pass because their entities
/// are already populated by the GitHub connector itself. Scanning
/// commits would be noisy — commit messages routinely link to
/// upstream PRs that weren't authored by the user.
///
/// # Idempotent
///
/// A second call produces no new entities because the helper
/// checks for an existing `github_pull_request` entity with a
/// matching `external_id` before pushing.
///
/// # Regex-free
///
/// Hand-rolled byte scanner for the same reason
/// [`scan_ticket_keys`] avoids the `regex` crate — keeping this
/// crate dependency-light because every UI filter toggle
/// re-renders through it. See the module docs.
pub fn extract_github_pr_urls(events: &mut [ActivityEvent]) {
    for event in events.iter_mut() {
        if !matches!(event.kind, ActivityKind::MrOpened | ActivityKind::MrMerged) {
            continue;
        }
        let mut found: Vec<GithubPrRef> = Vec::new();
        scan_github_pr_urls(&event.title, &mut found);
        if let Some(body) = &event.body {
            scan_github_pr_urls(body, &mut found);
        }
        if found.is_empty() {
            continue;
        }
        found.sort_by(|a, b| a.external_id.cmp(&b.external_id));
        found.dedup_by(|a, b| a.external_id == b.external_id);
        for pr in found {
            let already = event.entities.iter().any(|e| {
                e.kind == EntityKind::GitHubPullRequest && e.external_id == pr.external_id
            });
            if already {
                continue;
            }
            event.entities.push(EntityRef {
                kind: EntityKind::GitHubPullRequest,
                external_id: pr.external_id,
                label: Some(pr.label),
            });
        }
    }
}

/// One `github.com` PR URL match.
///
/// `external_id` is the normalised `"{repo}#{number}"` shape the
/// GitHub connector uses; `label` is the `#{number}` short form the
/// Jira/Confluence render layer displays in the verbose suffix.
struct GithubPrRef {
    external_id: String,
    label: String,
}

/// Scan `text` for `https://github.com/<owner>/<repo>/pull/<N>`
/// URLs and append matches to `out`.
///
/// Case-insensitive on the scheme + host. Trailing path segments
/// (`/files`, `/commits`, `#issuecomment-…`) are tolerated — the
/// scan consumes characters after the PR number until it hits a
/// non-URL-safe character or end of input.
fn scan_github_pr_urls(text: &str, out: &mut Vec<GithubPrRef>) {
    // Marker-based scan: we're looking for a `github.com/<owner>/<repo>/pull/<N>`
    // fragment. Finding the literal `/pull/` first narrows the search — then we
    // walk *backwards* to recover the owner/repo and *forwards* to collect the
    // PR number.
    let bytes = text.as_bytes();
    let needle = b"/pull/";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if !starts_with(&bytes[i..], needle) {
            i += 1;
            continue;
        }
        // Walk back to recover `<host>/<owner>/<repo>` by
        // finding the four slash boundaries surrounding them.
        // For `https://github.com/<owner>/<repo>/pull/<N>` those
        // are, from the match site walking backwards: the slash
        // before `pull`, the slash before `<repo>`, the slash
        // before `<owner>`, and the second slash of `://` sitting
        // before `<host>`.
        let slash_before_pull = i;
        let slash_before_repo = match rfind_byte(bytes, slash_before_pull, b'/') {
            Some(j) => j,
            None => {
                i += needle.len();
                continue;
            }
        };
        let slash_before_owner = match rfind_byte(bytes, slash_before_repo, b'/') {
            Some(j) => j,
            None => {
                i += needle.len();
                continue;
            }
        };
        let slash_before_host = match rfind_byte(bytes, slash_before_owner, b'/') {
            Some(j) => j,
            None => {
                i += needle.len();
                continue;
            }
        };
        let host = &bytes[slash_before_host + 1..slash_before_owner];
        if !eq_ignore_ascii_case(host, b"github.com") {
            i += needle.len();
            continue;
        }
        let owner = &bytes[slash_before_owner + 1..slash_before_repo];
        let repo = &bytes[slash_before_repo + 1..slash_before_pull];
        if owner.is_empty()
            || repo.is_empty()
            || !owner.iter().all(|&b| is_url_ident(b))
            || !repo.iter().all(|&b| is_url_ident(b))
        {
            i += needle.len();
            continue;
        }
        // Collect digits after `/pull/`.
        let digits_start = i + needle.len();
        let mut j = digits_start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == digits_start {
            i += needle.len();
            continue;
        }
        let number = match std::str::from_utf8(&bytes[digits_start..j]) {
            Ok(s) => s.to_string(),
            Err(_) => {
                i = j;
                continue;
            }
        };
        let repo_str = match std::str::from_utf8(repo) {
            Ok(s) => s.to_string(),
            Err(_) => {
                i = j;
                continue;
            }
        };
        out.push(GithubPrRef {
            external_id: format!("{repo_str}#{number}"),
            label: format!("#{number}"),
        });
        i = j;
    }
}

fn starts_with(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && &hay[..needle.len()] == needle
}

fn rfind_byte(hay: &[u8], end_exclusive: usize, target: u8) -> Option<usize> {
    if end_exclusive == 0 {
        return None;
    }
    let mut k = end_exclusive;
    while k > 0 {
        k -= 1;
        if hay[k] == target {
            return Some(k);
        }
    }
    None
}

fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Owner / repo segment character set: letters, digits, dot, dash,
/// underscore. GitHub allows these in repo names and the scan is
/// anchored inside `/pull/` so adjacency to URL punctuation
/// (`(parenthesised)`, `[bracketed]`, trailing `.`) is already
/// excluded by the `starts_with` check one level up.
const fn is_url_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'
}

/// Scan `text` for `[A-Z]{2,10}-\d+` tokens and push matches onto
/// `out` (with dedup at the caller, not here, because the caller
/// concatenates title + body before dedup).
///
/// Respects word boundaries: a leading alphanumeric or trailing
/// alphanumeric disqualifies the match, so `LOG4J-2` (the trailing
/// letter kills the word-boundary) and `FOO-42a` (trailing letter on
/// the digits) are rejected, while `[CAR-5117]` and
/// `"Merged CAR-5117:"` match.
fn scan_ticket_keys(text: &str, out: &mut Vec<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find the start of a potential key: an uppercase ASCII
        // letter with no alphanumeric char immediately before it.
        if !is_ascii_upper(bytes[i]) || (i > 0 && is_ascii_alnum(bytes[i - 1])) {
            i += 1;
            continue;
        }
        // Collect the uppercase prefix (letters only).
        let prefix_start = i;
        while i < bytes.len() && is_ascii_upper(bytes[i]) {
            i += 1;
        }
        let prefix_len = i - prefix_start;
        if !(2..=10).contains(&prefix_len) || i >= bytes.len() || bytes[i] != b'-' {
            continue;
        }
        // Skip the hyphen, collect digits.
        let hyphen = i;
        i += 1;
        let digits_start = i;
        while i < bytes.len() && is_ascii_digit(bytes[i]) {
            i += 1;
        }
        let digits_len = i - digits_start;
        if digits_len == 0 {
            // Rewind to just after the hyphen so the next scan can
            // re-evaluate from here.
            i = hyphen + 1;
            continue;
        }
        // Trailing-alnum boundary: if the byte after the digits is
        // alphanumeric, reject (e.g. `CAR-5117a`).
        if i < bytes.len() && is_ascii_alnum(bytes[i]) {
            continue;
        }
        if let Ok(token) = std::str::from_utf8(&bytes[prefix_start..i]) {
            out.push(token.to_string());
        }
    }
}

const fn is_ascii_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}

const fn is_ascii_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

const fn is_ascii_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use dayseam_core::{Actor, EntityKind, EntityRef, Privacy, RawRef, SourceId};
    use uuid::Uuid;

    fn src() -> SourceId {
        Uuid::from_u128(0x1111)
    }

    fn event(kind: ActivityKind, external_id: &str, title: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::new_v5(&Uuid::NAMESPACE_OID, external_id.as_bytes()),
            source_id: src(),
            external_id: external_id.into(),
            kind,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Self".into(),
                email: Some("self@example.com".into()),
                external_id: None,
            },
            title: title.into(),
            body: None,
            links: Vec::new(),
            entities: Vec::new(),
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: format!("k:{external_id}"),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn jira_transition(issue_key: &str) -> ActivityEvent {
        let mut e = event(
            ActivityKind::JiraIssueTransitioned,
            &format!("{issue_key}::transition"),
            &format!("{issue_key}: In Progress → Done"),
        );
        e.entities.push(EntityRef {
            kind: EntityKind::JiraIssue,
            external_id: issue_key.into(),
            label: None,
        });
        e
    }

    #[test]
    fn scan_simple_key() {
        let mut out = Vec::new();
        scan_ticket_keys("CAR-5117: Fix things", &mut out);
        assert_eq!(out, vec!["CAR-5117"]);
    }

    #[test]
    fn scan_key_inside_punctuation() {
        let mut out = Vec::new();
        scan_ticket_keys("Merged [CAR-5117] into main", &mut out);
        assert_eq!(out, vec!["CAR-5117"]);
    }

    #[test]
    fn scan_rejects_trailing_letter() {
        let mut out = Vec::new();
        // `LOG4J-2a` — the `a` after the digits kills the match.
        scan_ticket_keys("Bumping LOG4J-2a from 2.17", &mut out);
        assert!(out.is_empty(), "trailing alphanumeric rejects match");
    }

    #[test]
    fn scan_rejects_leading_letter() {
        let mut out = Vec::new();
        // `xCAR-1` — leading letter kills the match.
        scan_ticket_keys("xCAR-1", &mut out);
        assert!(out.is_empty(), "leading alphanumeric rejects match");
    }

    #[test]
    fn scan_rejects_too_short_or_long_prefix() {
        let mut out = Vec::new();
        // `A-1` — prefix below 2 chars.
        scan_ticket_keys("A-1 short", &mut out);
        assert!(out.is_empty());
        // 11-char prefix — above 10.
        scan_ticket_keys("ABCDEFGHIJK-1 longer than allowed", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn commit_titled_with_ticket_gains_jira_target_entity() {
        // Plan invariant 4.
        let mut events = vec![event(
            ActivityKind::CommitAuthored,
            "sha1",
            "CAR-5117: Fix review findings",
        )];
        extract_ticket_keys(&mut events);
        let targets: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::JiraIssue)
            .collect();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].external_id, "CAR-5117");
    }

    #[test]
    fn extract_ticket_keys_is_idempotent() {
        // Plan invariant 5.
        let mut events = vec![event(
            ActivityKind::CommitAuthored,
            "sha1",
            "CAR-5117: Fix review findings",
        )];
        extract_ticket_keys(&mut events);
        let first = events[0].entities.clone();
        extract_ticket_keys(&mut events);
        assert_eq!(events[0].entities, first, "second call must be a no-op");
    }

    #[test]
    fn extract_ticket_keys_bails_on_noisy_titles() {
        // Plan invariant 6. The title references four keys — we bail.
        let mut events = vec![event(
            ActivityKind::CommitAuthored,
            "sha1",
            "Fix GH-123 and FOO-4 and BAR-9 and BAZ-11 by bumping deps",
        )];
        extract_ticket_keys(&mut events);
        let targets: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::JiraIssue)
            .collect();
        assert!(
            targets.is_empty(),
            "event referencing >3 candidates attaches none"
        );
    }

    #[test]
    fn extract_ticket_keys_preserves_existing_jira_issue_targets() {
        let mut e = event(
            ActivityKind::CommitAuthored,
            "sha1",
            "CAR-5117: Fix review findings",
        );
        e.entities.push(EntityRef {
            kind: EntityKind::JiraIssue,
            external_id: "CAR-5117".into(),
            label: Some("Pre-existing".into()),
        });
        let mut events = vec![e];
        extract_ticket_keys(&mut events);
        let targets: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::JiraIssue)
            .collect();
        assert_eq!(targets.len(), 1, "existing jira_issue target wins");
        assert_eq!(targets[0].label.as_deref(), Some("Pre-existing"));
    }

    #[test]
    fn extract_scans_body_in_addition_to_title() {
        let mut e = event(ActivityKind::CommitAuthored, "sha1", "chore: bump deps");
        e.body = Some("Closes CAR-5117 per the release plan.".into());
        let mut events = vec![e];
        extract_ticket_keys(&mut events);
        assert!(events[0]
            .entities
            .iter()
            .any(|ent| ent.kind == EntityKind::JiraIssue && ent.external_id == "CAR-5117"));
    }

    #[test]
    fn jira_transition_annotated_with_mr_that_triggered_it() {
        // Plan invariant 7.
        let mr = {
            let mut e = event(ActivityKind::MrOpened, "!321", "CAR-5117: Rename commands");
            // Simulate what `extract_ticket_keys` already attached.
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mut events = vec![mr, jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events);
        let transition = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(transition.parent_external_id.as_deref(), Some("!321"));
    }

    #[test]
    fn annotate_transition_is_idempotent() {
        // Plan invariant 8.
        let mr = {
            let mut e = event(ActivityKind::MrOpened, "!321", "CAR-5117: Rename commands");
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mut events = vec![mr, jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events);
        let first = events.clone();
        annotate_transition_with_mr(&mut events);
        assert_eq!(events, first, "second call produces identical events");
    }

    #[test]
    fn annotate_no_op_when_mr_missing() {
        let mut events = vec![jira_transition("CAR-9999")];
        annotate_transition_with_mr(&mut events);
        assert_eq!(
            events[0].parent_external_id, None,
            "transition with no matching MR keeps its pre-existing parent (None)"
        );
    }

    /// DAY-88 / CORR-v0.2-05. Pre-fix, the winner was "first in the
    /// vec", which was walker-insertion dependent. Now it is
    /// "earliest `occurred_at`". This test vets the new rule by
    /// placing the earlier-in-time MR *second* in the vec — so
    /// any code that still relies on vec order would pick the wrong
    /// MR and fail the assertion.
    #[test]
    fn annotate_prefers_earliest_mr_by_occurred_at() {
        let later_in_time_but_first_in_vec = {
            let mut e = event(ActivityKind::MrOpened, "!100", "CAR-5117: later-in-time");
            e.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 14, 0, 0).unwrap();
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let earlier_in_time_but_second_in_vec = {
            let mut e = event(ActivityKind::MrMerged, "!200", "CAR-5117: earlier-in-time");
            e.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap();
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mut events = vec![
            later_in_time_but_first_in_vec,
            earlier_in_time_but_second_in_vec,
            jira_transition("CAR-5117"),
        ];
        annotate_transition_with_mr(&mut events);
        let transition = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(
            transition.parent_external_id.as_deref(),
            Some("!200"),
            "the MR that occurred earlier in time must win even when it appears later in the input vec"
        );
    }

    /// DAY-88 / CORR-v0.2-05. When two MRs share an `occurred_at`,
    /// pairing falls through to `ActivityEvent::id` — which is a
    /// UUIDv5 from `(source_id, external_id, kind)`. Because that's
    /// content-addressable, the tie-break is reproducible across
    /// runs and across walker orderings.
    #[test]
    fn annotate_tie_breaks_mrs_with_same_occurred_at_by_deterministic_id() {
        let shared_time = Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap();
        let mr_a = {
            let mut e = event(ActivityKind::MrOpened, "!100", "CAR-5117: a");
            e.occurred_at = shared_time;
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mr_b = {
            let mut e = event(ActivityKind::MrOpened, "!200", "CAR-5117: b");
            e.occurred_at = shared_time;
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        // The expected winner is whichever of !100 / !200 has the
        // smaller UUIDv5. Recompute deterministically rather than
        // hard-code, so a seed change in `event()`'s test helper
        // doesn't flip the assertion silently.
        let winning_id = if mr_a.id < mr_b.id { "!100" } else { "!200" };

        // Run once with one vec ordering ...
        let mut events_ab = vec![mr_a.clone(), mr_b.clone(), jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events_ab);
        let transition_ab = events_ab
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();

        // ... and again with the MRs swapped. Both must yield the
        // same winner because the tie-break is deterministic.
        let mut events_ba = vec![mr_b, mr_a, jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events_ba);
        let transition_ba = events_ba
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();

        assert_eq!(
            transition_ab.parent_external_id.as_deref(),
            Some(winning_id)
        );
        assert_eq!(
            transition_ba.parent_external_id.as_deref(),
            Some(winning_id)
        );
    }

    // ----- DAY-97: GitHub PR → Jira transition enrichment -----------------

    /// Build a GitHub-shaped PR event. Mirrors the shape
    /// `connector_github::normalise::compose_pr_event` produces:
    /// `external_id` is `"{repo}#{number}"` (no owner prefix —
    /// GitHub's `/users/{login}/events` stream uses the short form);
    /// the title carries the Jira ticket key the same way a GitLab
    /// MR title would; the `jira_issue` entity is populated by
    /// `extract_ticket_keys` upstream in the pipeline and is
    /// simulated here so the unit test stays focused on the
    /// annotate pass.
    fn github_pr_event(
        kind: ActivityKind,
        repo: &str,
        number: u32,
        occurred_at: DateTime<Utc>,
        ticket_key: &str,
    ) -> ActivityEvent {
        let external_id = format!("{repo}#{number}");
        let mut e = event(
            kind,
            &external_id,
            &format!("{ticket_key}: Rename commands"),
        );
        e.occurred_at = occurred_at;
        e.entities.push(EntityRef {
            kind: EntityKind::GitHubPullRequest,
            external_id: external_id.clone(),
            label: Some(format!("#{number}")),
        });
        e.entities.push(EntityRef {
            kind: EntityKind::JiraIssue,
            external_id: ticket_key.into(),
            label: None,
        });
        e
    }

    /// Plan v0.4 Task 6 invariant 1: a GitHub PR opened shortly
    /// before a transition on the same Jira key stamps
    /// `parent_external_id` with the PR's `external_id`.
    #[test]
    fn jira_transition_annotates_triggering_github_pr() {
        let pr_time = Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap();
        let pr = github_pr_event(
            ActivityKind::GitHubPullRequestOpened,
            "dayseam",
            42,
            pr_time,
            "CAR-5117",
        );
        let mut transition = jira_transition("CAR-5117");
        transition.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 11, 0, 0).unwrap();
        let mut events = vec![pr, transition];
        annotate_transition_with_mr(&mut events);
        let t = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(
            t.parent_external_id.as_deref(),
            Some("dayseam#42"),
            "GitHub PR external_id must be stamped on the transition"
        );
    }

    /// Plan v0.4 Task 6 invariant 2: a PR opened **after** the
    /// transition must NOT produce the annotation. Guards against
    /// the "attribution travelling backward in time" failure mode
    /// the v0.2 dogfood notes flagged.
    #[test]
    fn jira_transition_does_not_annotate_subsequent_github_pr() {
        let transition_time = Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap();
        let pr_time = transition_time + Duration::minutes(30);
        let pr = github_pr_event(
            ActivityKind::GitHubPullRequestOpened,
            "dayseam",
            99,
            pr_time,
            "CAR-5117",
        );
        let mut transition = jira_transition("CAR-5117");
        transition.occurred_at = transition_time;
        let mut events = vec![pr, transition];
        annotate_transition_with_mr(&mut events);
        let t = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(
            t.parent_external_id, None,
            "PR that happens after the transition must not claim authorship"
        );
    }

    /// Plan v0.4 Task 6 invariant 2 (stale side): a PR merged more
    /// than `MR_TRIGGER_WINDOW` before the transition is not
    /// credited. This is the "the MR that shipped last week" case.
    #[test]
    fn jira_transition_ignores_mr_outside_24h_window() {
        let transition_time = Utc.with_ymd_and_hms(2026, 4, 20, 12, 0, 0).unwrap();
        let stale_mr_time = transition_time - Duration::hours(25);
        let pr = github_pr_event(
            ActivityKind::GitHubPullRequestMerged,
            "dayseam",
            7,
            stale_mr_time,
            "CAR-5117",
        );
        let mut transition = jira_transition("CAR-5117");
        transition.occurred_at = transition_time;
        let mut events = vec![pr, transition];
        annotate_transition_with_mr(&mut events);
        let t = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(t.parent_external_id, None);
    }

    /// Exactly on the 24h boundary — the guard is inclusive so
    /// an MR that opened 24 hours prior still counts. This pins
    /// the boundary so a future refactor that flips it to
    /// `<` instead of `<=` is caught.
    #[test]
    fn jira_transition_annotates_mr_exactly_at_24h_window_edge() {
        let transition_time = Utc.with_ymd_and_hms(2026, 4, 20, 12, 0, 0).unwrap();
        let boundary_mr_time = transition_time - Duration::hours(24);
        let pr = github_pr_event(
            ActivityKind::GitHubPullRequestOpened,
            "dayseam",
            7,
            boundary_mr_time,
            "CAR-5117",
        );
        let mut transition = jira_transition("CAR-5117");
        transition.occurred_at = transition_time;
        let mut events = vec![pr, transition];
        annotate_transition_with_mr(&mut events);
        let t = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(t.parent_external_id.as_deref(), Some("dayseam#7"));
    }

    /// Plan v0.4 Task 6 invariant 3 (regression gate): the existing
    /// GitLab MR → Jira pipeline still fires, side-by-side with a
    /// GitHub PR for a different ticket. Proves the generalised
    /// index indexes both sources.
    #[test]
    fn both_gitlab_mr_and_github_pr_cross_source_annotate_in_one_pass() {
        let gitlab_mr = {
            let mut e = event(ActivityKind::MrOpened, "!321", "CAR-5117: Rename commands");
            e.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap();
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let gh_pr = github_pr_event(
            ActivityKind::GitHubPullRequestOpened,
            "dayseam",
            42,
            Utc.with_ymd_and_hms(2026, 4, 20, 9, 30, 0).unwrap(),
            "KTON-4550",
        );
        let mut t_car = jira_transition("CAR-5117");
        t_car.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 11, 0, 0).unwrap();
        t_car.external_id = "CAR-5117::transition::car".into();
        let mut t_kton = jira_transition("KTON-4550");
        t_kton.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 11, 30, 0).unwrap();
        t_kton.external_id = "KTON-4550::transition::kton".into();

        let mut events = vec![gitlab_mr, gh_pr, t_car, t_kton];
        annotate_transition_with_mr(&mut events);

        let got_car = events
            .iter()
            .find(|e| e.external_id == "CAR-5117::transition::car")
            .unwrap()
            .parent_external_id
            .as_deref();
        let got_kton = events
            .iter()
            .find(|e| e.external_id == "KTON-4550::transition::kton")
            .unwrap()
            .parent_external_id
            .as_deref();
        assert_eq!(got_car, Some("!321"));
        assert_eq!(got_kton, Some("dayseam#42"));
    }

    // ----- DAY-97: GitLab MR body mentions GitHub PR -----------------------

    fn mr_with_body(iid: &str, title: &str, body: &str) -> ActivityEvent {
        let mut e = event(ActivityKind::MrOpened, iid, title);
        e.body = Some(body.into());
        e
    }

    /// Plan v0.4 Task 6 invariant 4: a GitLab MR body mentioning
    /// `https://github.com/org/repo/pull/42` produces an
    /// `EntityKind::GitHubPullRequest` entity on the MR event, so
    /// downstream features (evidence popover, future "mirrored PR"
    /// render) can surface the cross-link.
    #[test]
    fn gitlab_mr_body_mentions_github_pr_attaches_entity() {
        let mut events = vec![mr_with_body(
            "!321",
            "CAR-5117: Rename commands",
            "Mirrors https://github.com/vedanthvdev/dayseam/pull/42 upstream.",
        )];
        extract_github_pr_urls(&mut events);
        let gh_ents: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::GitHubPullRequest)
            .collect();
        assert_eq!(gh_ents.len(), 1);
        assert_eq!(gh_ents[0].external_id, "dayseam#42");
        assert_eq!(gh_ents[0].label.as_deref(), Some("#42"));
    }

    /// Idempotent: a second call after the first must not push a
    /// duplicate entity. Same contract as [`extract_ticket_keys`]
    /// — callers run the pipeline repeatedly on the same data set
    /// (dogfood CLI, streaming preview re-renders) and a growing
    /// entity list would break evidence popovers.
    #[test]
    fn extract_github_pr_urls_is_idempotent() {
        let mut events = vec![mr_with_body(
            "!321",
            "CAR-5117",
            "See https://github.com/vedanthvdev/dayseam/pull/42",
        )];
        extract_github_pr_urls(&mut events);
        let first = events[0].entities.clone();
        extract_github_pr_urls(&mut events);
        assert_eq!(events[0].entities, first);
    }

    /// Two distinct PRs in the same body → two entities. Matches
    /// the DRY-RUN case where an MR body lists "closes X, Y,
    /// rebased from Z".
    #[test]
    fn extract_github_pr_urls_handles_multiple_links() {
        let mut events = vec![mr_with_body(
            "!321",
            "CAR-5117",
            "Closes https://github.com/vedanthvdev/dayseam/pull/42 \
             and https://github.com/vedanthvdev/dayseam/pull/99",
        )];
        extract_github_pr_urls(&mut events);
        let mut ids: Vec<&str> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::GitHubPullRequest)
            .map(|e| e.external_id.as_str())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["dayseam#42", "dayseam#99"]);
    }

    /// Trailing path / anchor characters (`#issuecomment-…`,
    /// `/files`) are tolerated. The scan stops consuming digits as
    /// soon as it sees a non-digit — everything after is ignored.
    #[test]
    fn extract_github_pr_urls_tolerates_trailing_path_fragments() {
        let mut events = vec![mr_with_body(
            "!321",
            "CAR-5117",
            "See https://github.com/vedanthvdev/dayseam/pull/42/files and \
             https://github.com/vedanthvdev/dayseam/pull/99#issuecomment-7",
        )];
        extract_github_pr_urls(&mut events);
        let mut ids: Vec<&str> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::GitHubPullRequest)
            .map(|e| e.external_id.as_str())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["dayseam#42", "dayseam#99"]);
    }

    /// A URL pointing at a different host (`gitlab.com`) must not
    /// match — the scan anchors on `github.com` explicitly.
    #[test]
    fn extract_github_pr_urls_ignores_non_github_hosts() {
        let mut events = vec![mr_with_body(
            "!321",
            "CAR-5117",
            "Upstream https://gitlab.com/org/repo/pull/42",
        )];
        extract_github_pr_urls(&mut events);
        assert!(
            events[0]
                .entities
                .iter()
                .all(|e| e.kind != EntityKind::GitHubPullRequest),
            "non-github hosts must not produce a github_pull_request entity"
        );
    }

    /// The pass is scoped to GitLab MR-shaped events — scanning
    /// commit messages or Jira comments would be noisy and risks
    /// attaching cross-links to events that weren't authored by
    /// the user. A commit that mentions a GitHub PR URL keeps its
    /// original entity list untouched.
    #[test]
    fn extract_github_pr_urls_leaves_non_mr_events_untouched() {
        let mut commit = event(ActivityKind::CommitAuthored, "sha1", "chore: bump deps");
        commit.body = Some("See https://github.com/vedanthvdev/dayseam/pull/42".into());
        let mut events = vec![commit];
        extract_github_pr_urls(&mut events);
        assert!(
            events[0]
                .entities
                .iter()
                .all(|e| e.kind != EntityKind::GitHubPullRequest),
            "non-MR events must be skipped"
        );
    }
}
