//! Golden snapshots of the engine's output for every scenario
//! `connector-local-git` (Phase 2 Task 2) can produce.
//!
//! Each fixture builds a [`ReportInput`] from plain values, renders
//! it, and snapshots the resulting [`ReportDraft`] as YAML. Drift in
//! either the rollup, the template, or the `bullet_id` derivation
//! fails the snapshot — the two halves travel together.
//!
//! When intentionally changing the rendered output, run:
//!
//! ```sh
//! cargo insta accept -p dayseam-report
//! ```
//!
//! and review the resulting `.snap` diff in the PR.

mod common;

use chrono::TimeZone;
use common::*;
use dayseam_core::Privacy;

/// One repo, three commits, one author (the self). Happy path.
#[test]
fn dev_eod_single_repo_happy_path() {
    let src = source_id(1);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];

    let e1 = commit_event(
        src,
        "sha1aaaa",
        "/work/repo-a",
        "self@example.com",
        9,
        "feat: add activity store",
        Privacy::Normal,
    );
    let e2 = commit_event(
        src,
        "sha1bbbb",
        "/work/repo-a",
        "self@example.com",
        11,
        "refactor: extract rollup helper",
        Privacy::Normal,
    );
    let e3 = commit_event(
        src,
        "sha1cccc",
        "/work/repo-a",
        "self@example.com",
        14,
        "test: cover empty day path",
        Privacy::Normal,
    );

    let artifact = commit_set_artifact(src, "/work/repo-a", &[&e1, &e2, &e3]);
    input.events = vec![e1, e2, e3];
    input.artifacts = vec![artifact];
    input.per_source_state.insert(src, succeeded_state(3));

    let draft = dayseam_report::render(input).expect("render must succeed");
    insta::assert_yaml_snapshot!("dev_eod_single_repo", draft);
}

/// Two repos on the same day → two `CommitSet`s → two bullets, sorted
/// deterministically by `(kind, external_id)`.
#[test]
fn dev_eod_multi_repo() {
    let src = source_id(2);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];

    let a1 = commit_event(
        src,
        "aaa1aaaa",
        "/work/repo-a",
        "self@example.com",
        9,
        "fix: repo-a quirk",
        Privacy::Normal,
    );
    let b1 = commit_event(
        src,
        "bbb1bbbb",
        "/work/repo-b",
        "self@example.com",
        10,
        "feat: repo-b thing",
        Privacy::Normal,
    );
    let b2 = commit_event(
        src,
        "bbb2bbbb",
        "/work/repo-b",
        "self@example.com",
        13,
        "chore: repo-b cleanup",
        Privacy::Normal,
    );

    let art_a = commit_set_artifact(src, "/work/repo-a", &[&a1]);
    let art_b = commit_set_artifact(src, "/work/repo-b", &[&b1, &b2]);
    input.events = vec![a1, b1, b2];
    input.artifacts = vec![art_a, art_b];
    input.per_source_state.insert(src, succeeded_state(3));

    let draft = dayseam_report::render(input).expect("render must succeed");
    insta::assert_yaml_snapshot!("dev_eod_multi_repo", draft);
}

/// Private repo: events are flagged [`Privacy::RedactedPrivateRepo`]
/// so the bullet must say "(private work)" with no title, body, or
/// commit shas leaking into the rendered text.
#[test]
fn dev_eod_private_repo_redacted() {
    let src = source_id(3);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];

    let p1 = commit_event(
        src,
        "priv1111",
        "/work/secret-repo",
        "self@example.com",
        10,
        "REDACTED_TITLE_SHOULD_NEVER_APPEAR",
        Privacy::RedactedPrivateRepo,
    );
    let p2 = commit_event(
        src,
        "priv2222",
        "/work/secret-repo",
        "self@example.com",
        11,
        "ANOTHER_REDACTED_TITLE",
        Privacy::RedactedPrivateRepo,
    );

    let art = commit_set_artifact(src, "/work/secret-repo", &[&p1, &p2]);
    input.events = vec![p1, p2];
    input.artifacts = vec![art];
    input.per_source_state.insert(src, succeeded_state(2));

    let draft = dayseam_report::render(input.clone()).expect("render must succeed");

    // Extra defensive check beyond the golden: even if someone
    // accepts a wrong snapshot, we never want these strings in the
    // draft's markdown.
    let serialized = serde_json::to_string(&draft).unwrap();
    assert!(
        !serialized.contains("REDACTED_TITLE_SHOULD_NEVER_APPEAR"),
        "redacted title leaked into draft: {serialized}"
    );
    assert!(
        !serialized.contains("ANOTHER_REDACTED_TITLE"),
        "redacted title leaked into draft: {serialized}"
    );
    assert!(
        !serialized.contains("priv1111"),
        "commit sha leaked into redacted draft: {serialized}"
    );

    insta::assert_yaml_snapshot!("dev_eod_private_repo", draft);
}

/// Empty day: no events, no artifacts. Produces the explicit
/// empty-state section rather than an empty `sections` vec — the UI
/// relies on the section being present so it can render the
/// placeholder copy.
#[test]
fn dev_eod_empty_day() {
    let input = fixture_input();
    let draft = dayseam_report::render(input).expect("render must succeed");
    insta::assert_yaml_snapshot!("dev_eod_empty_day", draft);
}

/// Mixed authors: one commit by the self, two by someone else. The
/// engine must filter out the non-self commits before rollup, so the
/// resulting draft has exactly one bullet with `reason = "1 commit"`.
#[test]
fn dev_eod_filters_non_self_commits() {
    let src = source_id(4);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];

    let mine = commit_event(
        src,
        "mine0001",
        "/work/repo-a",
        "self@example.com",
        10,
        "feat: mine",
        Privacy::Normal,
    );
    let theirs1 = commit_event(
        src,
        "them0001",
        "/work/repo-a",
        "teammate@example.com",
        11,
        "feat: theirs",
        Privacy::Normal,
    );
    let theirs2 = commit_event(
        src,
        "them0002",
        "/work/repo-a",
        "teammate@example.com",
        12,
        "fix: theirs",
        Privacy::Normal,
    );

    // The connector would emit the CommitSet for all three commits
    // because upstream dedup happens at the rollup stage. The
    // engine's identity filter kicks in first.
    let art = commit_set_artifact(src, "/work/repo-a", &[&mine, &theirs1, &theirs2]);
    input.events = vec![mine, theirs1, theirs2];
    input.artifacts = vec![art];
    input.per_source_state.insert(src, succeeded_state(3));

    let draft = dayseam_report::render(input).expect("render must succeed");
    insta::assert_yaml_snapshot!("dev_eod_filters_non_self", draft);
}

/// Verbose mode: same input as the happy path but with
/// `verbose_mode = true`. Invariant #2 demands this is *additive* —
/// the non-verbose bullet's id and evidence must appear unchanged
/// and verbose text is appended.
#[test]
fn dev_eod_verbose_mode_is_additive() {
    let src = source_id(5);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];

    let c1 = commit_event(
        src,
        "ver11111",
        "/work/repo-a",
        "self@example.com",
        9,
        "feat: one",
        Privacy::Normal,
    );
    let c2 = commit_event(
        src,
        "ver22222",
        "/work/repo-a",
        "self@example.com",
        11,
        "feat: two",
        Privacy::Normal,
    );

    let art = commit_set_artifact(src, "/work/repo-a", &[&c1, &c2]);
    input.events = vec![c1, c2];
    input.artifacts = vec![art];
    input.verbose_mode = true;
    input.per_source_state.insert(src, succeeded_state(2));

    let draft = dayseam_report::render(input).expect("render must succeed");
    insta::assert_yaml_snapshot!("dev_eod_verbose", draft);
}

/// Sanity: `generated_at` threads through untouched. If the engine
/// ever starts calling `Utc::now()` this test catches it — drift
/// here is a leaked side-effect, not a template change.
#[test]
fn generated_at_is_not_rewritten() {
    let mut input = fixture_input();
    let unusual = chrono::Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    input.generated_at = unusual;
    let draft = dayseam_report::render(input).unwrap();
    assert_eq!(draft.generated_at, unusual);
}
