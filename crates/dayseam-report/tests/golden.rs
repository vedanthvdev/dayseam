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

/// DAY-52 regression: two configured sources scanning the same repo
/// each produce their own `CommitSet` artifact for the same day.
/// The rollup merges them by `(repo_path, date)` so the report
/// shows each commit exactly once, not twice. This is the Phase 2
/// "duplicate bullet" bug from the bug report — `2A` in the DAY-52
/// investigation.
#[test]
fn dev_eod_deduplicates_same_repo_across_sources() {
    let src_a = source_id(12);
    let src_b = source_id(13);
    let mut input = fixture_input();
    input.source_identities = vec![
        self_git_identity(src_a, "self@example.com"),
        self_git_identity(src_b, "self@example.com"),
    ];

    // Both sources saw the same two commits (same SHAs, same
    // repo_path). The connector emits one `CommitSet` artifact per
    // (source, repo, day) so artifacts don't collapse at the
    // per-source boundary; the rollup has to do it.
    let e1_a = commit_event(
        src_a,
        "dup11111",
        "/work/dayseam",
        "self@example.com",
        9,
        "feat: thing one",
        Privacy::Normal,
    );
    let e1_b = commit_event(
        src_b,
        "dup11111",
        "/work/dayseam",
        "self@example.com",
        9,
        "feat: thing one",
        Privacy::Normal,
    );
    let e2_a = commit_event(
        src_a,
        "dup22222",
        "/work/dayseam",
        "self@example.com",
        11,
        "feat: thing two",
        Privacy::Normal,
    );
    let e2_b = commit_event(
        src_b,
        "dup22222",
        "/work/dayseam",
        "self@example.com",
        11,
        "feat: thing two",
        Privacy::Normal,
    );

    let art_a = commit_set_artifact(src_a, "/work/dayseam", &[&e1_a, &e2_a]);
    let art_b = commit_set_artifact(src_b, "/work/dayseam", &[&e1_b, &e2_b]);
    input.events = vec![e1_a, e1_b, e2_a, e2_b];
    input.artifacts = vec![art_a, art_b];
    input.per_source_state.insert(src_a, succeeded_state(2));
    input.per_source_state.insert(src_b, succeeded_state(2));

    let draft = dayseam_report::render(input).expect("render must succeed");
    let bullets: Vec<&str> = draft
        .sections
        .iter()
        .flat_map(|s| s.bullets.iter().map(|b| b.text.as_str()))
        .collect();

    // Exactly two bullets, not four. Same commit rendered once even
    // though two sources saw it.
    assert_eq!(
        bullets.len(),
        2,
        "expected one bullet per commit with cross-source dedup, got: {bullets:?}"
    );
    assert!(
        bullets.iter().any(|b| b.contains("feat: thing one")),
        "bullets missing first commit: {bullets:?}"
    );
    assert!(
        bullets.iter().any(|b| b.contains("feat: thing two")),
        "bullets missing second commit: {bullets:?}"
    );
    // Bullet ids must be distinct so the UI can click-through to
    // per-commit evidence without one bullet masking another.
    let ids: std::collections::HashSet<&str> = draft
        .sections
        .iter()
        .flat_map(|s| s.bullets.iter().map(|b| b.id.as_str()))
        .collect();
    assert_eq!(ids.len(), bullets.len(), "duplicate bullet ids: {ids:?}");
}

/// Phase 3 Task 2: cross-source `CommitAuthored` dedup. Local-git
/// and GitLab both emit a `CommitAuthored` on the same SHA; after
/// the orchestrator-side `dedup_commit_authored` pass the render
/// shows exactly one bullet, and the surviving event carries the
/// union of links (so the UI's evidence popover can still deep-link
/// to both the local working copy and the GitLab commit page).
#[test]
fn dev_eod_dedups_commitauthored_across_sources() {
    use dayseam_core::Link;

    let src_local = source_id(14);
    let src_gitlab = source_id(15);
    let mut input = fixture_input();
    input.source_identities = vec![
        self_git_identity(src_local, "self@example.com"),
        self_git_identity(src_gitlab, "self@example.com"),
    ];

    let sha = "sha3ph32";
    let mut local = commit_event(
        src_local,
        sha,
        "/work/dayseam",
        "self@example.com",
        9,
        "feat: cross-source dedup",
        Privacy::Normal,
    );
    local.links = vec![Link {
        url: "file:///work/dayseam/.git".into(),
        label: Some("local".into()),
    }];
    // The GitLab-side row carries a longer body so the dedup
    // pass picks it as the canonical survivor.
    let mut gitlab = commit_event(
        src_gitlab,
        sha,
        "/work/dayseam",
        "self@example.com",
        9,
        "feat: cross-source dedup",
        Privacy::Normal,
    );
    gitlab.body = Some("Long GitLab-enriched commit message".into());
    gitlab.links = vec![Link {
        url: format!("https://gitlab.example/commit/{sha}"),
        label: Some("gitlab".into()),
    }];

    let deduped = dayseam_report::dedup_commit_authored(vec![local, gitlab]);
    assert_eq!(deduped.len(), 1, "dedup must collapse cross-source SHA");
    assert_eq!(
        deduped[0].source_id, src_gitlab,
        "richer body wins (GitLab-enriched row)"
    );
    assert_eq!(deduped[0].links.len(), 2, "links unioned across sources");

    input.events = deduped;
    // Leave `artifacts` empty so the rollup mints a synthetic
    // `CommitSet` for the single surviving event — this matches the
    // orchestrator shape when no connector pre-grouped.
    input.per_source_state.insert(src_local, succeeded_state(1));
    input
        .per_source_state
        .insert(src_gitlab, succeeded_state(1));

    let draft = dayseam_report::render(input).expect("render must succeed");
    let bullets: Vec<&str> = draft
        .sections
        .iter()
        .flat_map(|s| s.bullets.iter().map(|b| b.text.as_str()))
        .collect();
    assert_eq!(
        bullets.len(),
        1,
        "one bullet per SHA regardless of producer count, got: {bullets:?}"
    );
    assert!(
        bullets[0].contains("feat: cross-source dedup"),
        "bullet text missing commit title: {bullets:?}"
    );
}

/// Phase 3 Task 2: verbose mode renders `(rolled into !N)` when the
/// orchestrator's `annotate_rolled_into_mr` pass stamped the MR iid
/// on a `CommitAuthored`. The plain-mode rendering is unchanged (the
/// verbose suffix only lives behind the `verbose_mode` gate).
#[test]
fn dev_eod_verbose_annotates_rolled_into_mr() {
    use dayseam_report::{annotate_rolled_into_mr, MergeRequestArtifact};

    let src = source_id(16);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];
    input.verbose_mode = true;

    let c_on_mr = commit_event(
        src,
        "mr111111",
        "/work/dayseam",
        "self@example.com",
        9,
        "feat: MR part one",
        Privacy::Normal,
    );
    let c_outside = commit_event(
        src,
        "solo2222",
        "/work/dayseam",
        "self@example.com",
        10,
        "chore: unrelated",
        Privacy::Normal,
    );

    let mut events = vec![c_on_mr, c_outside];
    annotate_rolled_into_mr(
        &mut events,
        &[MergeRequestArtifact {
            external_id: "!42".into(),
            commit_shas: vec!["mr111111".into()],
        }],
    );
    input.events = events;
    input.per_source_state.insert(src, succeeded_state(2));

    let draft = dayseam_report::render(input).expect("render must succeed");
    let bullets: Vec<&str> = draft
        .sections
        .iter()
        .flat_map(|s| s.bullets.iter().map(|b| b.text.as_str()))
        .collect();
    let on_mr_bullet = bullets
        .iter()
        .find(|b| b.contains("MR part one"))
        .unwrap_or_else(|| panic!("on-MR bullet missing, bullets: {bullets:?}"));
    assert!(
        on_mr_bullet.contains("(rolled into !42)"),
        "verbose bullet missing (rolled into !42) suffix: {on_mr_bullet:?}"
    );

    let outside_bullet = bullets
        .iter()
        .find(|b| b.contains("unrelated"))
        .unwrap_or_else(|| panic!("outside bullet missing, bullets: {bullets:?}"));
    assert!(
        !outside_bullet.contains("rolled into"),
        "non-MR bullet must not carry the rolled-into suffix: {outside_bullet:?}"
    );
}

/// DAY-78 Jira: three `JiraIssueTransitioned` events across two
/// projects (CAR, KTON) render as three bullets with project-scoped
/// prefixes. The goldens lock the `**<project_name>** (<project_key>)
/// — <title>` shape so future dispatch changes surface as a diff.
#[test]
fn dev_eod_jira_two_projects() {
    let src = source_id(20);
    let mut input = fixture_input();
    input.source_identities = vec![self_atlassian_identity(src, "acct-self")];

    let t1 = jira_transition_event(
        src,
        "CAR-5117",
        "CAR",
        "Cardtronics",
        "acct-self",
        9,
        "CAR-5117: In Progress → In Review",
    );
    let t2 = jira_transition_event(
        src,
        "CAR-6001",
        "CAR",
        "Cardtronics",
        "acct-self",
        10,
        "CAR-6001: To Do → In Progress",
    );
    let t3 = jira_transition_event(
        src,
        "KTON-4550",
        "KTON",
        "Kontiki",
        "acct-self",
        11,
        "KTON-4550: In Review → Done",
    );
    input.events = vec![t1, t2, t3];
    input.per_source_state.insert(src, succeeded_state(3));

    let draft = dayseam_report::render(input).expect("render must succeed");
    insta::assert_yaml_snapshot!("dev_eod_jira_two_projects", draft);
}

/// DAY-78 Confluence: two `ConfluencePageEdited` events across two
/// spaces render with the `**<space_name>** (<space_key>) —` prefix.
/// Pre-DAY-80 the walker isn't wired yet, but the group-key plumbing
/// it'll ride on is — this fixture locks that plumbing in place.
#[test]
fn dev_eod_confluence_two_spaces() {
    let src = source_id(21);
    let mut input = fixture_input();
    input.source_identities = vec![self_atlassian_identity(src, "acct-self")];

    let p_eng = confluence_page_edited_event(
        src,
        "page-1001",
        "ENG",
        "Engineering",
        "acct-self",
        9,
        "Edited: Release runbook",
    );
    let p_ops = confluence_page_edited_event(
        src,
        "page-2002",
        "OPS",
        "Operations",
        "acct-self",
        10,
        "Edited: On-call rotation",
    );
    input.events = vec![p_eng, p_ops];
    input.per_source_state.insert(src, succeeded_state(2));

    let draft = dayseam_report::render(input).expect("render must succeed");
    insta::assert_yaml_snapshot!("dev_eod_confluence_two_spaces", draft);
}

/// DAY-86 regression for issue #86: a day that mixes commits, Jira
/// transitions and Confluence edits must render as **three distinct
/// sections** (`Commits` → `Jira issues` → `Confluence pages`), not
/// one catch-all `Commits` heading the way v0.1/v0.2 did.
///
/// This is the exact scenario the user hit in dogfood — a Confluence
/// edit + two Jira transitions + a commit, all rendered under a
/// single `## Commits` heading. The golden snapshot locks:
///
/// 1. **Section count**: three sections, none of them empty.
/// 2. **Section order**: Commits first (shipping), Jira second
///    (triage), Confluence third (docs), as
///    `sections::ReportSection`'s derived `Ord` dictates. Even
///    though the Confluence event arrives *before* the Jira events
///    in wall-clock time in this fixture (hour 8 vs 10/12), the
///    render order is by section-ordinal, not event-time.
/// 3. **Section ids / titles**: `commits` / `jira_issues` /
///    `confluence_pages` — the contract the markdown sink and the
///    streaming preview key on.
/// 4. **Bullets in each section only contain the right kind**:
///    no Jira or Confluence bullet leaks into `Commits`, which is
///    the v0.2.x bug this work fixes.
#[test]
fn dev_eod_mixed_commits_jira_confluence() {
    let git_src = source_id(30);
    let jira_src = source_id(31);
    let conf_src = source_id(32);

    let mut input = fixture_input();
    input.source_identities = vec![
        self_git_identity(git_src, "self@example.com"),
        self_atlassian_identity(jira_src, "acct-self"),
        self_atlassian_identity(conf_src, "acct-self"),
    ];

    // Confluence event fires earliest in wall-clock order (hour 8).
    // Rendering order must still be Commits → Jira → Confluence
    // regardless, because sections render by ordinal not by
    // occurred-at.
    let conf = confluence_page_edited_event(
        conf_src,
        "page-3003",
        "ST",
        "Delivery Tribes",
        "acct-self",
        8,
        "Edited page: Kanban Release Process",
    );
    let commit = commit_event(
        git_src,
        "sha1aaaa",
        "/work/repo-a",
        "self@example.com",
        9,
        "feat: wire per-kind report sections",
        Privacy::Normal,
    );
    let jira_a = jira_transition_event(
        jira_src,
        "CAR-5117",
        "CAR",
        "Carbon Team",
        "acct-self",
        10,
        "CAR-5117 Production Verification → Done",
    );
    let jira_b = jira_transition_event(
        jira_src,
        "CAR-5190",
        "CAR",
        "Carbon Team",
        "acct-self",
        12,
        "CAR-5190: Test affected test plugin",
    );

    let commit_artifact = commit_set_artifact(git_src, "/work/repo-a", &[&commit]);

    input.events = vec![conf, commit, jira_a, jira_b];
    input.artifacts = vec![commit_artifact];
    // Deliberately one per_source_state entry only. `per_source_state`
    // is a `HashMap<Uuid, SourceRunState>` and its iteration order is
    // non-deterministic, so snapshotting a map with >1 entry flakes
    // across runs. This test is about section bucketing; the other
    // sources' state is irrelevant. If `per_source_state` ever
    // stabilises on an ordered container, populate all three here.
    input.per_source_state.insert(git_src, succeeded_state(1));

    let draft = dayseam_report::render(input).expect("render must succeed");

    // Structural invariants first — the snapshot pins everything
    // else, but these three assertions make the *intent* of the
    // test obvious at read time and give a better failure message
    // than a diff of YAML if a regression flips them.
    assert_eq!(
        draft.sections.len(),
        3,
        "mixed-day must render 3 sections (Commits / Jira issues / Confluence pages)",
    );
    let ids: Vec<&str> = draft.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["commits", "jira_issues", "confluence_pages"],
        "section order must be Commits → Jira issues → Confluence pages",
    );
    let commits_section = &draft.sections[0];
    assert!(
        commits_section
            .bullets
            .iter()
            .all(|b| !b.text.contains("Jira") && !b.text.contains("Confluence")),
        "Jira/Confluence bullets must not leak into the Commits section \
         (the v0.2.x bug this test guards against)",
    );

    insta::assert_yaml_snapshot!("dev_eod_mixed_commits_jira_confluence", draft);
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
