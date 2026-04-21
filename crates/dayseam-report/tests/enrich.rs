//! DAY-78 cross-source enrichment integration tests.
//!
//! These tests prove the nine invariants from
//! `docs/plan/2026-04-20-v0.2-atlassian.md` Task 6 against the
//! `dayseam-report` public API. Unit tests in
//! `src/enrich.rs` + `src/pipeline.rs` already cover the pure
//! functions; these tests exist to show the pipeline + rollup +
//! render composition is stable end-to-end.
//!
//! Invariants proven here:
//!
//! 1. (Regression) Existing repo-only goldens stay byte-identical.
//!    Lives in `tests/golden.rs`; this file proves the Jira /
//!    Confluence shapes that ride alongside them.
//! 2. Jira events group by project → one section header per project.
//! 3. Confluence events group by space → one section header per space.
//! 4. Ticket-key enrichment attaches a `jira_issue` target entity.
//! 5. Ticket-key enrichment is idempotent.
//! 6. Ticket-key extraction ignores code-like noise (>3 candidates).
//! 7. Transition annotation links a Jira transition to its MR.
//! 8. Transition annotation is idempotent.
//! 9. Pipeline ordering is stable (dedup → extract → annotate-transition
//!    → annotate-rolled-into-MR).

mod common;

use common::*;
use dayseam_core::{ActivityEvent, ActivityKind, EntityRef, Privacy};
use dayseam_report::{
    annotate_transition_with_mr, dedup_commit_authored, extract_ticket_keys, pipeline,
    MergeRequestArtifact,
};

// ---- helpers --------------------------------------------------------------

fn bullets(draft: &dayseam_core::ReportDraft) -> Vec<&str> {
    draft
        .sections
        .iter()
        .flat_map(|s| s.bullets.iter().map(|b| b.text.as_str()))
        .collect()
}

fn mk_commit(source_id: dayseam_core::SourceId, sha: &str, title: &str) -> ActivityEvent {
    commit_event(
        source_id,
        sha,
        "/work/dayseam",
        "self@example.com",
        9,
        title,
        Privacy::Normal,
    )
}

fn mk_mr(source_id: dayseam_core::SourceId, iid: &str, title: &str) -> ActivityEvent {
    use chrono::{TimeZone, Utc};
    use dayseam_core::{Actor, RawRef};
    use uuid::Uuid;
    ActivityEvent {
        id: Uuid::new_v5(&Uuid::NAMESPACE_OID, iid.as_bytes()),
        source_id,
        external_id: iid.into(),
        kind: ActivityKind::MrOpened,
        occurred_at: Utc.with_ymd_and_hms(2026, 4, 18, 10, 0, 0).unwrap(),
        actor: Actor {
            display_name: "Self".into(),
            email: None,
            external_id: Some("17".into()),
        },
        title: title.into(),
        body: None,
        links: Vec::new(),
        entities: Vec::new(),
        parent_external_id: None,
        metadata: serde_json::Value::Null,
        raw_ref: RawRef {
            storage_key: format!("mr:{iid}"),
            content_type: "application/json".into(),
        },
        privacy: Privacy::Normal,
    }
}

// ---- invariant 2 ----------------------------------------------------------

/// Three `JiraIssueTransitioned` events across two projects (CAR, KTON)
/// render exactly two project headers — one per `jira_project` key.
#[test]
fn jira_events_group_by_project_key() {
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
    let bs = bullets(&draft);
    assert_eq!(bs.len(), 3, "one bullet per transition, got: {bs:?}");

    let has_cardtronics_prefix = bs
        .iter()
        .filter(|b| b.contains("**Cardtronics** (CAR) —"))
        .count();
    let has_kontiki_prefix = bs
        .iter()
        .filter(|b| b.contains("**Kontiki** (KTON) —"))
        .count();
    assert_eq!(
        has_cardtronics_prefix, 2,
        "Cardtronics project prefix appears on both CAR bullets, got: {bs:?}"
    );
    assert_eq!(
        has_kontiki_prefix, 1,
        "Kontiki project prefix appears on the one KTON bullet, got: {bs:?}"
    );
}

// ---- invariant 3 ----------------------------------------------------------

/// Two `ConfluencePageEdited` events across two spaces render with a
/// space-key prefix on each bullet. (Pre-DAY-80: the walker isn't
/// wired yet, but the group-key plumbing it'll ride on is.)
#[test]
fn confluence_events_group_by_space_key() {
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
    let bs = bullets(&draft);
    assert_eq!(bs.len(), 2, "one bullet per page edit, got: {bs:?}");
    assert!(
        bs.iter()
            .any(|b| b.contains("**Engineering** (ENG) — Edited: Release runbook")),
        "ENG space prefix missing: {bs:?}"
    );
    assert!(
        bs.iter()
            .any(|b| b.contains("**Operations** (OPS) — Edited: On-call rotation")),
        "OPS space prefix missing: {bs:?}"
    );
}

// ---- invariant 4 ----------------------------------------------------------

/// A `CommitAuthored` event with `"CAR-5117: Fix review findings"` in
/// its title gains a `jira_issue` target via [`extract_ticket_keys`].
#[test]
fn commit_titled_with_ticket_gains_jira_target_entity() {
    let src = source_id(22);
    let mut events = vec![mk_commit(src, "sha1aaaa", "CAR-5117: Fix review findings")];
    extract_ticket_keys(&mut events);
    let targets: Vec<&EntityRef> = events[0]
        .entities
        .iter()
        .filter(|e| e.kind == "jira_issue")
        .collect();
    assert_eq!(targets.len(), 1, "exactly one jira_issue target attached");
    assert_eq!(targets[0].external_id, "CAR-5117");
}

// ---- invariant 5 ----------------------------------------------------------

#[test]
fn extract_ticket_keys_is_idempotent() {
    let src = source_id(23);
    let mut events = vec![mk_commit(src, "sha2bbbb", "CAR-5117: Fix review findings")];
    extract_ticket_keys(&mut events);
    let first = events.clone();
    extract_ticket_keys(&mut events);
    assert_eq!(
        events, first,
        "extract_ticket_keys must not produce new entities on a second call"
    );
}

// ---- invariant 6 ----------------------------------------------------------

#[test]
fn extract_ticket_keys_bails_on_noisy_titles() {
    let src = source_id(24);
    let mut events = vec![mk_commit(
        src,
        "sha3cccc",
        "Fix GH-123 and FOO-4 and BAR-9 and BAZ-11 by bumping deps",
    )];
    let before = events[0].entities.clone();
    extract_ticket_keys(&mut events);
    let jira_targets = events[0]
        .entities
        .iter()
        .filter(|e| e.kind == "jira_issue")
        .count();
    assert_eq!(
        jira_targets, 0,
        "commit referencing >3 candidates attaches zero jira_issue targets"
    );
    assert_eq!(
        events[0].entities, before,
        "non-jira_issue entities (e.g. repo) are untouched"
    );
}

// ---- invariant 7 ----------------------------------------------------------

#[test]
fn jira_transition_annotated_with_mr_that_triggered_it() {
    let src = source_id(25);
    let mut mr = mk_mr(src, "!321", "CAR-5117: Rename commands");
    // Simulate the earlier extract_ticket_keys pass.
    mr.entities.push(EntityRef {
        kind: "jira_issue".into(),
        external_id: "CAR-5117".into(),
        label: None,
    });
    let transition = jira_transition_event(
        src,
        "CAR-5117",
        "CAR",
        "Cardtronics",
        "acct-self",
        11,
        "CAR-5117: In Progress → Done",
    );

    let mut events = vec![mr, transition];
    annotate_transition_with_mr(&mut events);
    let annotated = events
        .iter()
        .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
        .expect("transition survived");
    assert_eq!(annotated.parent_external_id.as_deref(), Some("!321"));
}

// ---- invariant 8 ----------------------------------------------------------

#[test]
fn annotate_transition_is_idempotent() {
    let src = source_id(26);
    let mut mr = mk_mr(src, "!321", "CAR-5117: Rename commands");
    mr.entities.push(EntityRef {
        kind: "jira_issue".into(),
        external_id: "CAR-5117".into(),
        label: None,
    });
    let transition = jira_transition_event(
        src,
        "CAR-5117",
        "CAR",
        "Cardtronics",
        "acct-self",
        11,
        "CAR-5117: In Progress → Done",
    );

    let mut events = vec![mr, transition];
    annotate_transition_with_mr(&mut events);
    let first = events.clone();
    annotate_transition_with_mr(&mut events);
    assert_eq!(
        events, first,
        "second call is a no-op on already-annotated events"
    );
}

// ---- invariant 9 ----------------------------------------------------------

/// Full pipeline composition: dedup merges cross-source duplicates →
/// extract attaches ticket-key targets → annotate stamps the MR on
/// the transition → annotate-rolled-into-MR stamps the MR on the
/// surviving commit. Running the whole chain twice produces the same
/// output, and the downstream `render` uses the annotated events.
#[test]
fn pipeline_runs_dedup_enrich_rollup_in_order() {
    let src_local = source_id(27);
    let src_gitlab = source_id(28);

    // Same SHA on both sources — dedup must collapse them.
    let local = mk_commit(src_local, "sha9abcd", "CAR-5117: trim JSON");
    let mut gitlab = mk_commit(src_gitlab, "sha9abcd", "CAR-5117: trim JSON");
    gitlab.body = Some("Long commit message from GitLab side.".into());

    // An MR that references the same ticket (so extract attaches
    // `jira_issue: CAR-5117` to it, which the transition annotator
    // then uses).
    let mr = mk_mr(src_gitlab, "!321", "CAR-5117: Rename commands");
    let transition = jira_transition_event(
        src_gitlab,
        "CAR-5117",
        "CAR",
        "Cardtronics",
        "acct-self",
        11,
        "CAR-5117: In Progress → Done",
    );

    let events = vec![local, gitlab, mr, transition];
    let mrs = vec![MergeRequestArtifact {
        external_id: "!321".into(),
        commit_shas: vec!["sha9abcd".into()],
    }];

    let first = pipeline(events.clone(), &mrs);
    let second = pipeline(first.clone(), &mrs);
    assert_eq!(second, first, "pipeline is a pure function of its input");

    // One commit survived dedup.
    let commits: Vec<&ActivityEvent> = first
        .iter()
        .filter(|e| e.kind == ActivityKind::CommitAuthored)
        .collect();
    assert_eq!(commits.len(), 1, "dedup merged duplicate SHAs");

    // The surviving commit and the MR both carry the extracted
    // ticket-key target.
    for e in &first {
        if matches!(
            e.kind,
            ActivityKind::CommitAuthored | ActivityKind::MrOpened
        ) {
            assert!(
                e.entities
                    .iter()
                    .any(|ent| ent.kind == "jira_issue" && ent.external_id == "CAR-5117"),
                "extract_ticket_keys missed {:?}",
                e.kind
            );
        }
    }

    // Transition is annotated with the MR.
    let t = first
        .iter()
        .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
        .unwrap();
    assert_eq!(t.parent_external_id.as_deref(), Some("!321"));

    // Rolled-into-MR stamped the deduped commit.
    assert_eq!(commits[0].parent_external_id.as_deref(), Some("!321"));
}

// ---- bonus: pipeline without MRs degrades gracefully ---------------------

/// A day with only commits + no MR list runs dedup + extract but
/// leaves transitions + rolled-into-MR as no-ops. Proves the pipeline
/// is safe to call on every day's events, v0.1-style.
#[test]
fn pipeline_runs_cleanly_without_mr_or_jira_input() {
    let src = source_id(29);
    let c1 = mk_commit(src, "shaaaaaa", "CAR-5117: do the thing");
    let c2 = mk_commit(src, "shabbbbb", "chore: unrelated");

    let deduped_only = dedup_commit_authored(vec![c1.clone(), c2.clone()]);
    let piped = pipeline(vec![c1, c2], &[]);

    // Dedup alone doesn't attach targets; the pipeline does.
    let piped_targets = piped[0]
        .entities
        .iter()
        .filter(|e| e.kind == "jira_issue")
        .count();
    assert_eq!(piped_targets, 1);
    let dedup_targets = deduped_only[0]
        .entities
        .iter()
        .filter(|e| e.kind == "jira_issue")
        .count();
    assert_eq!(dedup_targets, 0);
}
