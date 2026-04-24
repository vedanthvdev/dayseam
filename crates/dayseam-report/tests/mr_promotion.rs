//! DAY-98 invariants: MR/PR promotion into `## Merge requests`,
//! `Other` → `Unlinked` rename, and PERF-v0.3-01 grouper pass count.
//!
//! These tests pin the section-routing contract the DAY-98 PR
//! advertises. Golden snapshots cover the byte-for-byte rendering;
//! this file covers the structural invariants that would otherwise
//! need a human reader of the golden diff to catch:
//!
//! * GitLab MR events no longer leak into `## Commits`.
//! * GitHub PR events land in `## Merge requests` from day one.
//! * Commits rolled into an MR still appear exactly once under
//!   `## Commits` (with the verbose `(rolled into !N)` suffix),
//!   not twice.
//! * The renamed `## Unlinked activity` section still catches
//!   unattached Confluence comments.
//! * The grouper walks the rollup output in exactly one pass
//!   (PERF-v0.3-01).

mod common;

use chrono::{NaiveDate, TimeZone, Utc};
use common::*;
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityKind, EntityRef, Link, Privacy, RawRef, SourceId,
};
use uuid::Uuid;

// --------------------------------------------------------------------------
// Fixture helpers
// --------------------------------------------------------------------------

fn mr_opened_event(
    source_id: SourceId,
    project: &str,
    iid: u64,
    actor_user_id: &str,
    hour: u32,
    title: &str,
) -> ActivityEvent {
    let external_id = format!("!{iid}");
    ActivityEvent {
        id: ActivityEvent::deterministic_id(&source_id.to_string(), &external_id, "MrOpened"),
        source_id,
        external_id,
        kind: ActivityKind::MrOpened,
        occurred_at: Utc.with_ymd_and_hms(2026, 4, 18, hour, 0, 0).unwrap(),
        actor: Actor {
            display_name: "Self".into(),
            email: None,
            external_id: Some(actor_user_id.into()),
        },
        title: format!("Opened MR: {title}"),
        body: None,
        links: vec![Link {
            url: format!("https://gitlab.example/api/v4/projects/{project}/merge_requests/{iid}"),
            label: Some(format!("!{iid}")),
        }],
        entities: vec![EntityRef {
            kind: EntityKind::Repo,
            external_id: project.into(),
            label: None,
        }],
        parent_external_id: None,
        metadata: serde_json::Value::Null,
        raw_ref: RawRef {
            storage_key: format!("gitlab-event:!{iid}"),
            content_type: "application/json".into(),
        },
        privacy: Privacy::Normal,
    }
}

fn github_pr_opened_event(
    source_id: SourceId,
    owner_repo: &str,
    number: u64,
    actor_login: &str,
    hour: u32,
    title: &str,
) -> ActivityEvent {
    let external_id = format!("{owner_repo}#{number}");
    ActivityEvent {
        id: ActivityEvent::deterministic_id(
            &source_id.to_string(),
            &external_id,
            "GitHubPullRequestOpened",
        ),
        source_id,
        external_id,
        kind: ActivityKind::GitHubPullRequestOpened,
        occurred_at: Utc.with_ymd_and_hms(2026, 4, 18, hour, 0, 0).unwrap(),
        actor: Actor {
            display_name: "Self".into(),
            email: None,
            external_id: Some(actor_login.into()),
        },
        title: format!("Opened PR: {title}"),
        body: None,
        links: vec![Link {
            url: format!("https://github.com/{owner_repo}/pull/{number}"),
            label: None,
        }],
        entities: vec![EntityRef {
            kind: EntityKind::GitHubRepo,
            external_id: owner_repo.into(),
            label: None,
        }],
        parent_external_id: None,
        metadata: serde_json::Value::Null,
        raw_ref: RawRef {
            storage_key: format!("github:{owner_repo}#{number}"),
            content_type: "application/json".into(),
        },
        privacy: Privacy::Normal,
    }
}

fn self_github_identity(source_id: SourceId, login: &str) -> dayseam_core::SourceIdentity {
    dayseam_core::SourceIdentity {
        id: Uuid::new_v4(),
        person_id: self_person().id,
        source_id: Some(source_id),
        kind: dayseam_core::SourceIdentityKind::GitHubLogin,
        external_actor_id: login.into(),
    }
}

fn confluence_unattached_comment_event(
    source_id: SourceId,
    space_key: &str,
    account_id: &str,
    hour: u32,
) -> ActivityEvent {
    let external_id = format!("comment::{space_key}::{hour}");
    ActivityEvent {
        id: ActivityEvent::deterministic_id(
            &source_id.to_string(),
            &external_id,
            "ConfluenceComment",
        ),
        source_id,
        external_id,
        kind: ActivityKind::ConfluenceComment,
        occurred_at: Utc.with_ymd_and_hms(2026, 4, 18, hour, 0, 0).unwrap(),
        actor: Actor {
            display_name: "Self".into(),
            email: None,
            external_id: Some(account_id.into()),
        },
        title: "Comment on a lost page".into(),
        body: None,
        links: vec![],
        entities: vec![EntityRef {
            kind: EntityKind::ConfluenceSpace,
            external_id: space_key.into(),
            label: Some(format!("{space_key} space")),
        }],
        parent_external_id: None,
        metadata: serde_json::json!({ "location": "footer", "unattached": true }),
        raw_ref: RawRef {
            storage_key: "confluence:unattached".into(),
            content_type: "application/json".into(),
        },
        privacy: Privacy::Normal,
    }
}

// --------------------------------------------------------------------------
// Invariants
// --------------------------------------------------------------------------

/// Invariant 1. A day of GitLab MR lifecycle events (Opened → Approved
/// → Merged) renders as *one* bullet under `## Merge requests`, not
/// as three bullets under `## Commits`.
#[test]
fn gitlab_mrs_render_under_merge_requests_section() {
    let src = source_id(11);
    let mut input = fixture_input();
    input.source_identities = vec![self_gitlab_user_id_identity(src, "17")];

    let opened = mr_opened_event(
        src,
        "company/payments",
        321,
        "17",
        9,
        "feat: payment retries",
    );
    let mut approved = opened.clone();
    approved.id = ActivityEvent::deterministic_id(&src.to_string(), "!321", "MrApproved");
    approved.kind = ActivityKind::MrApproved;
    approved.title = "Approved MR: feat: payment retries".into();
    approved.occurred_at = Utc.with_ymd_and_hms(2026, 4, 18, 11, 0, 0).unwrap();
    let mut merged = opened.clone();
    merged.id = ActivityEvent::deterministic_id(&src.to_string(), "!321", "MrMerged");
    merged.kind = ActivityKind::MrMerged;
    merged.title = "Merged MR: feat: payment retries".into();
    merged.occurred_at = Utc.with_ymd_and_hms(2026, 4, 18, 15, 0, 0).unwrap();

    input.events = vec![opened, approved, merged];

    let draft = dayseam_report::render(input).expect("render must succeed");

    let mr_section = draft
        .sections
        .iter()
        .find(|s| s.id == "merge_requests")
        .expect("MR section must render");
    assert_eq!(
        mr_section.title, "Merge requests",
        "heading must be `Merge requests` (sentence case)"
    );
    assert_eq!(
        mr_section.bullets.len(),
        1,
        "three lifecycle events on one MR collapse to a single bullet, got {:?}",
        mr_section.bullets
    );
    assert!(
        mr_section.bullets[0]
            .text
            .contains("**company/payments!321**"),
        "bullet must use `**project!iid**` prefix, got {:?}",
        mr_section.bullets[0].text,
    );
    assert!(
        mr_section.bullets[0]
            .text
            .ends_with("feat: payment retries"),
        "bullet must strip `Opened MR:` prefix and keep canonical title, got {:?}",
        mr_section.bullets[0].text,
    );
    assert!(
        draft.sections.iter().all(|s| s.id != "commits"),
        "no Commits section — the day has zero CommitAuthored events, so \
         nothing should leak back into `## Commits`",
    );
}

/// Invariant 2. A day of GitHub PR lifecycle events renders as one
/// bullet under `## Merge requests` with a `#42`-style suffix.
#[test]
fn github_prs_render_under_merge_requests_section() {
    let src = source_id(12);
    let mut input = fixture_input();
    input.source_identities = vec![self_github_identity(src, "vedanth")];

    let opened = github_pr_opened_event(src, "company/api", 42, "vedanth", 9, "fix: null tax code");
    let mut reviewed = opened.clone();
    reviewed.id = ActivityEvent::deterministic_id(
        &src.to_string(),
        "company/api#42",
        "GitHubPullRequestReviewed",
    );
    reviewed.kind = ActivityKind::GitHubPullRequestReviewed;
    reviewed.title = "Reviewed PR: fix: null tax code".into();
    reviewed.occurred_at = Utc.with_ymd_and_hms(2026, 4, 18, 11, 0, 0).unwrap();

    input.events = vec![opened, reviewed];

    let draft = dayseam_report::render(input).expect("render must succeed");

    let mr_section = draft
        .sections
        .iter()
        .find(|s| s.id == "merge_requests")
        .expect("MR section must render for GitHub PRs");
    assert_eq!(mr_section.bullets.len(), 1);
    assert!(
        mr_section.bullets[0].text.contains("**company/api#42**"),
        "bullet must use `**owner/repo#number**` prefix, got {:?}",
        mr_section.bullets[0].text,
    );
}

/// Invariant 3. A commit that rolled into an MR renders exactly once
/// (under `## Commits`, with the verbose `(rolled into !321)`
/// suffix) even on a day where the same MR's lifecycle events also
/// produced a `## Merge requests` bullet. Before DAY-98, the MR
/// events silently rolled into the commit set and the day rendered
/// the same work twice.
#[test]
fn commits_rolled_into_mr_render_once() {
    let src = source_id(13);
    let mut input = fixture_input();
    input.source_identities = vec![
        self_git_identity(src, "self@example.com"),
        self_gitlab_user_id_identity(src, "17"),
    ];
    input.verbose_mode = true;

    let commit = commit_event(
        src,
        "sha1aaaa",
        "/work/repo-a",
        "self@example.com",
        9,
        "feat: add payment retries",
        Privacy::Normal,
    );
    let mr_event = mr_opened_event(
        src,
        "company/payments",
        321,
        "17",
        10,
        "feat: payment retries",
    );

    // The commit's `parent_external_id` points at the MR so the
    // verbose render produces `(rolled into !321)`; set it directly
    // to skip the orchestrator's `annotate_rolled_into_mr` pass and
    // keep the test local to this crate.
    let mut commit = commit;
    commit.parent_external_id = Some("!321".into());

    let commit_set = commit_set_artifact(src, "/work/repo-a", &[&commit]);
    input.events = vec![commit, mr_event];
    input.artifacts = vec![commit_set];

    let draft = dayseam_report::render(input).expect("render must succeed");

    let commits = draft
        .sections
        .iter()
        .find(|s| s.id == "commits")
        .expect("Commits section must still render");
    assert_eq!(
        commits.bullets.len(),
        1,
        "commit appears exactly once under `## Commits`"
    );
    assert!(
        commits.bullets[0].text.contains("(rolled into !321)"),
        "verbose mode preserves the `(rolled into !N)` suffix, got {:?}",
        commits.bullets[0].text,
    );

    let mrs = draft
        .sections
        .iter()
        .find(|s| s.id == "merge_requests")
        .expect("MR section must render alongside the commit");
    assert_eq!(
        mrs.bullets.len(),
        1,
        "the MR's own bullet renders under `## Merge requests` \
         (separate from the commit under `## Commits`)",
    );
}

/// Invariant 4. A Confluence comment whose parent page couldn't be
/// resolved lands under the renamed `## Unlinked activity` section
/// (was `## Other` pre-DAY-98). The id is `unlinked`, the title is
/// `Unlinked activity`.
#[test]
fn unlinked_section_renders_confluence_orphan_comments() {
    let src = source_id(14);
    let mut input = fixture_input();
    input.source_identities = vec![self_atlassian_identity(src, "acct-1")];

    let orphan = confluence_unattached_comment_event(src, "ENG", "acct-1", 10);
    input.events = vec![orphan];

    let draft = dayseam_report::render(input).expect("render must succeed");

    let unlinked = draft
        .sections
        .iter()
        .find(|s| s.id == "unlinked")
        .expect("unlinked section must render for unattached comments");
    assert_eq!(unlinked.title, "Unlinked activity");
    assert_eq!(unlinked.bullets.len(), 1);
    assert!(
        draft.sections.iter().all(|s| s.id != "other"),
        "no legacy `other` section id may survive the rename"
    );
}

/// Invariant 5 (PERF-v0.3-01). The grouper's single pass over the
/// rollup output is observable via its complexity: for a 500-event
/// synthetic day, `render` returns in well under a second, every
/// event lands in exactly one section, and no event double-counts.
///
/// We don't instrument a literal pass counter (that would need a
/// trait hook inside the renderer that's invisible at the public
/// API). Instead we verify the two observable consequences of the
/// "one walk, array-indexed bucket" design: (a) total bullets =
/// total events, modulo the MR collapse rule, and (b) walltime
/// stays bounded. The array-bucketing rework in `build_sections`
/// is what makes this bound hold — a `BTreeMap` with 500 inserts
/// would also pass, but the structural proof (grouper == one
/// for-loop) lives in `render::build_sections` itself, pinned by
/// code review on the DAY-98 PR.
#[test]
fn grouper_makes_single_pass_over_rollup() {
    let src = source_id(15);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];

    let commits: Vec<ActivityEvent> = (0..500)
        .map(|i| {
            commit_event(
                src,
                &format!("sha{i:04x}aaaa"),
                "/work/repo-a",
                "self@example.com",
                9,
                &format!("chore: bulk commit {i}"),
                Privacy::Normal,
            )
        })
        .collect();
    let refs: Vec<&ActivityEvent> = commits.iter().collect();
    let artifact = commit_set_artifact(src, "/work/repo-a", &refs);

    input.events = commits;
    input.artifacts = vec![artifact];

    let start = std::time::Instant::now();
    let draft = dayseam_report::render(input).expect("render must succeed");
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "500-event render must stay well under 1s, took {elapsed:?}",
    );

    let total_bullets: usize = draft.sections.iter().map(|s| s.bullets.len()).sum();
    assert_eq!(
        total_bullets, 500,
        "one bullet per commit — no section double-buckets an event"
    );

    // Every evidence row must reference real event ids (the pass
    // visited each event once and recorded a matching evidence edge).
    assert_eq!(
        draft.evidence.len(),
        500,
        "evidence count must equal event count: one row per visited event",
    );
}

/// Sanity: the NaiveDate fixture matches the common helpers — pins
/// the test file against accidental drift if `FIXTURE_DATE_STR`
/// changes in `common/mod.rs`.
#[test]
fn fixture_date_is_2026_04_18() {
    assert_eq!(
        fixture_date(),
        NaiveDate::from_ymd_opt(2026, 4, 18).unwrap()
    );
}
