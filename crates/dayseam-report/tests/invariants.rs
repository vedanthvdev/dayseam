//! Invariant tests for the report engine. These are the property /
//! structural assertions that the plan's seven-invariant matrix
//! demands; golden snapshots (tests/golden.rs) cover the rendered
//! surface itself.

mod common;

use std::path::PathBuf;

use chrono::Utc;
use common::*;
use dayseam_core::{
    ActivityEvent, Artifact, ArtifactId, ArtifactKind, ArtifactPayload, EntityKind, Privacy,
    RenderedBullet, ReportDraft,
};
use proptest::prelude::*;
// ---- Invariant #1: purity -----------------------------------------------

/// Two identical inputs yield byte-identical drafts — every bullet id,
/// every evidence edge, every field.
#[test]
fn render_is_deterministic() {
    let src = source_id(7);
    let mut base = fixture_input();
    base.source_identities = vec![self_git_identity(src, "self@example.com")];

    let e1 = commit_event(
        src,
        "pur11111",
        "/work/repo-a",
        "self@example.com",
        9,
        "feat: one",
        Privacy::Normal,
    );
    let e2 = commit_event(
        src,
        "pur22222",
        "/work/repo-a",
        "self@example.com",
        10,
        "feat: two",
        Privacy::Normal,
    );
    let art = commit_set_artifact(src, "/work/repo-a", &[&e1, &e2]);
    base.events = vec![e1, e2];
    base.artifacts = vec![art];

    let one = dayseam_report::render(base.clone()).unwrap();
    // 100 runs: same input must produce byte-identical output. If
    // the engine ever consults a clock, a RNG, or an unordered map
    // iterator, this will flake.
    for _ in 0..100 {
        let next = dayseam_report::render(base.clone()).unwrap();
        assert_eq!(draft_fingerprint(&one), draft_fingerprint(&next));
    }
}

/// Permutations of the input event order must not change the output.
/// This is the structural form of determinism: rollup is the only
/// place order could leak through, so it gets a dedicated check.
#[test]
fn render_is_order_independent_in_events() {
    let src = source_id(8);
    let mut base = fixture_input();
    base.source_identities = vec![self_git_identity(src, "self@example.com")];

    let events = vec![
        commit_event(
            src,
            "ord11111",
            "/work/repo-a",
            "self@example.com",
            9,
            "feat: one",
            Privacy::Normal,
        ),
        commit_event(
            src,
            "ord22222",
            "/work/repo-a",
            "self@example.com",
            11,
            "feat: two",
            Privacy::Normal,
        ),
        commit_event(
            src,
            "ord33333",
            "/work/repo-b",
            "self@example.com",
            10,
            "fix: three",
            Privacy::Normal,
        ),
    ];
    let art_a = commit_set_artifact(src, "/work/repo-a", &[&events[0], &events[1]]);
    let art_b = commit_set_artifact(src, "/work/repo-b", &[&events[2]]);

    let mut shuffled = events.clone();
    shuffled.reverse();

    let forward = {
        let mut i = base.clone();
        i.events = events;
        i.artifacts = vec![art_a.clone(), art_b.clone()];
        dayseam_report::render(i).unwrap()
    };
    let reverse = {
        let mut i = base.clone();
        i.events = shuffled;
        i.artifacts = vec![art_b, art_a];
        dayseam_report::render(i).unwrap()
    };

    assert_eq!(draft_fingerprint(&forward), draft_fingerprint(&reverse));
}

// ---- Invariant #2: verbose mode is additive -----------------------------

/// Turning `verbose_mode` on must never change an existing bullet's
/// id or evidence vector — it only appends verbose text.
#[test]
fn verbose_mode_only_adds_bullets() {
    let src = source_id(9);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];

    let e1 = commit_event(
        src,
        "vrb11111",
        "/work/repo-a",
        "self@example.com",
        9,
        "feat: one",
        Privacy::Normal,
    );
    let e2 = commit_event(
        src,
        "vrb22222",
        "/work/repo-a",
        "self@example.com",
        10,
        "feat: two",
        Privacy::Normal,
    );
    let art = commit_set_artifact(src, "/work/repo-a", &[&e1, &e2]);
    input.events = vec![e1, e2];
    input.artifacts = vec![art];

    let plain = dayseam_report::render({
        let mut i = input.clone();
        i.verbose_mode = false;
        i
    })
    .unwrap();
    let verbose = dayseam_report::render({
        let mut i = input.clone();
        i.verbose_mode = true;
        i
    })
    .unwrap();

    // Ids and evidence unchanged.
    assert_eq!(bullet_ids(&plain), bullet_ids(&verbose));
    assert_eq!(plain.evidence, verbose.evidence);

    // Verbose bullets are *supersets* of the plain ones.
    let p_texts: Vec<&str> = plain.sections[0]
        .bullets
        .iter()
        .map(|b| b.text.as_str())
        .collect();
    let v_texts: Vec<&str> = verbose.sections[0]
        .bullets
        .iter()
        .map(|b| b.text.as_str())
        .collect();
    for (p, v) in p_texts.iter().zip(v_texts.iter()) {
        assert!(
            v.starts_with(p),
            "verbose bullet must start with plain bullet text — plain={p:?} verbose={v:?}"
        );
    }
}

// ---- Invariant #3: every bullet has evidence ---------------------------

proptest! {
    /// Every bullet in a rendered draft must carry at least one
    /// event in its evidence vector, except the synthetic empty-state
    /// bullet. Generated input sets are kept small (≤10 events) so
    /// property-test failures are readable.
    #[test]
    fn every_bullet_has_evidence(events_spec in events_strategy()) {
        let src = source_id(10);
        let mut input = fixture_input();
        input.source_identities = vec![self_git_identity(src, "self@example.com")];

        let (events, artifacts) = materialise(src, events_spec);
        let is_empty_case = events.is_empty();
        input.events = events;
        input.artifacts = artifacts;

        let draft = dayseam_report::render(input).expect("render must succeed");

        for section in &draft.sections {
            for bullet in &section.bullets {
                if is_empty_case {
                    prop_assert!(bullet_has_no_evidence(bullet, &draft));
                } else {
                    prop_assert!(bullet_has_evidence(bullet, &draft));
                }
            }
        }
    }
}

// ---- Invariant #5: empty day renders explicit empty state --------------

#[test]
fn empty_day_renders_empty_state() {
    let draft = dayseam_report::render(fixture_input()).unwrap();
    assert_eq!(draft.sections.len(), 1, "empty day still has one section");
    let section = &draft.sections[0];
    assert_eq!(section.id, "commits");
    assert_eq!(section.bullets.len(), 1);
    let body = &section.bullets[0].text;
    assert!(
        body.contains("No tracked activity"),
        "empty day bullet must use the documented empty-state string, got: {body}"
    );
    assert!(draft.evidence.is_empty(), "empty day has no evidence edges");
}

// ---- DAY-71: render-stage self-filter on GitLab events ----------------
//
// The production bug the backfill in `sources_add` /
// `sources_update` / startup closes looked like this: `sync_runs`
// showed `fetched_count: N`, `activity_events` had all N rows, but
// `report_drafts` came back with "No tracked activity". That's the
// render-stage self-filter dropping every event because no
// `SourceIdentity` matched the GitLab-shaped actor. These two tests
// pin the filter contract so regressions in either direction trip
// immediately.

/// Without a `GitLabUserId` identity, GitLab events are silently
/// dropped and the section collapses to the empty-state bullet.
/// This is the bug the DAY-71 backfill exists to prevent in
/// production — in the engine it's the correct, documented
/// behaviour (unknown actor ≠ self), so we test for it explicitly.
#[test]
fn gitlab_events_without_matching_identity_render_empty_state() {
    let src = source_id(12);
    let mut input = fixture_input();
    // Deliberately empty: no identity at all.
    input.source_identities = Vec::new();

    let event = gitlab_commit_event(
        src,
        "glbwithout",
        "/work/gitlab-repo",
        "291",
        10,
        "feat: unmatched actor",
        Privacy::Normal,
    );
    input.events = vec![event];
    input.artifacts = Vec::new();

    let draft = dayseam_report::render(input).expect("render must succeed");
    assert_eq!(draft.sections.len(), 1);
    let section = &draft.sections[0];
    assert_eq!(section.bullets.len(), 1);
    assert!(
        section.bullets[0].text.contains("No tracked activity"),
        "unmatched GitLab actor must render empty state, got: {}",
        section.bullets[0].text
    );
    assert!(
        draft.evidence.is_empty(),
        "unmatched actor must not produce evidence edges"
    );
}

/// With a `GitLabUserId` identity whose `external_actor_id` equals
/// the event's `actor.external_id`, the event passes the filter and
/// a real bullet is rendered. This is the behaviour the DAY-71
/// identity auto-seed in `sources_add` / `sources_update` plus the
/// startup backfill guarantee for every configured GitLab source.
#[test]
fn gitlab_events_with_matching_user_id_identity_render() {
    let src = source_id(13);
    let mut input = fixture_input();
    input.source_identities = vec![self_gitlab_user_id_identity(src, "291")];

    let event = gitlab_commit_event(
        src,
        "glbmatched",
        "/work/gitlab-repo",
        "291",
        10,
        "feat: matched actor",
        Privacy::Normal,
    );
    input.events = vec![event];
    input.artifacts = Vec::new();

    let draft = dayseam_report::render(input).expect("render must succeed");
    assert_eq!(draft.sections.len(), 1);
    let bullets = &draft.sections[0].bullets;
    assert_eq!(
        bullets.len(),
        1,
        "matched actor must produce exactly one bullet, got: {bullets:?}"
    );
    assert!(
        !bullets[0].text.contains("No tracked activity"),
        "matched actor must not collapse to empty state, got: {}",
        bullets[0].text
    );
    assert!(
        bullets[0].text.contains("feat: matched actor"),
        "bullet must surface the commit title, got: {}",
        bullets[0].text
    );
    assert_eq!(
        draft.evidence.len(),
        1,
        "matched actor must produce one evidence edge"
    );
}

// ---- Invariant #4: redacted events render as "(private work)" ----------

#[test]
fn redacted_events_render_without_message() {
    let src = source_id(11);
    let mut input = fixture_input();
    input.source_identities = vec![self_git_identity(src, "self@example.com")];
    let e = commit_event(
        src,
        "prv11111",
        "/work/secret",
        "self@example.com",
        10,
        "SENSITIVE_TITLE_DO_NOT_LEAK",
        Privacy::RedactedPrivateRepo,
    );
    let art = commit_set_artifact(src, "/work/secret", &[&e]);
    input.events = vec![e];
    input.artifacts = vec![art];

    let draft = dayseam_report::render(input).unwrap();

    let serialized = serde_json::to_string(&draft).unwrap();
    assert!(
        !serialized.contains("SENSITIVE_TITLE_DO_NOT_LEAK"),
        "redacted title leaked: {serialized}"
    );
    assert!(
        draft
            .sections
            .iter()
            .flat_map(|s| &s.bullets)
            .any(|b| b.text.contains("(private work)")),
        "redacted bullet must say (private work)"
    );
}

// ---- Unknown template id fails typed -----------------------------------

#[test]
fn unknown_template_id_returns_typed_error() {
    let mut input = fixture_input();
    input.template_id = "not.a.real.template".into();
    let err = dayseam_report::render(input).unwrap_err();
    assert!(
        matches!(err, dayseam_report::ReportError::UnknownTemplate(ref s) if s == "not.a.real.template"),
        "unknown template id must return UnknownTemplate, got: {err:?}"
    );
}

// ---- helpers ------------------------------------------------------------

fn draft_fingerprint(d: &ReportDraft) -> String {
    // Serialize to a stable JSON form so the comparison fails with
    // an informative diff instead of a bare `PartialEq` panic.
    serde_json::to_string(d).unwrap()
}

fn bullet_ids(d: &ReportDraft) -> Vec<String> {
    d.sections
        .iter()
        .flat_map(|s| s.bullets.iter().map(|b| b.id.clone()))
        .collect()
}

fn bullet_has_evidence(bullet: &RenderedBullet, draft: &ReportDraft) -> bool {
    draft
        .evidence
        .iter()
        .any(|e| e.bullet_id == bullet.id && !e.event_ids.is_empty())
}

fn bullet_has_no_evidence(bullet: &RenderedBullet, draft: &ReportDraft) -> bool {
    !draft.evidence.iter().any(|e| e.bullet_id == bullet.id)
}

/// Shape of a generated input event. Kept minimal so shrinkage picks
/// readable counterexamples.
#[derive(Debug, Clone)]
struct EventSpec {
    sha: String,
    repo: String,
    hour: u32,
    title: String,
}

fn events_strategy() -> impl Strategy<Value = Vec<EventSpec>> {
    proptest::collection::vec(event_spec_strategy(), 0..10)
}

fn event_spec_strategy() -> impl Strategy<Value = EventSpec> {
    (
        "[a-f0-9]{8}",
        prop::sample::select(vec!["/work/repo-a", "/work/repo-b", "/work/repo-c"]),
        0u32..23,
        prop::string::string_regex("[a-z ]{1,20}").unwrap(),
    )
        .prop_map(|(sha, repo, hour, title)| EventSpec {
            sha,
            repo: repo.to_string(),
            hour,
            title,
        })
}

fn materialise(
    src: dayseam_core::SourceId,
    specs: Vec<EventSpec>,
) -> (Vec<ActivityEvent>, Vec<Artifact>) {
    use std::collections::BTreeMap;

    let mut events: Vec<ActivityEvent> = specs
        .into_iter()
        .map(|s| {
            commit_event(
                src,
                &s.sha,
                &s.repo,
                "self@example.com",
                s.hour,
                &s.title,
                Privacy::Normal,
            )
        })
        .collect();
    // Proptest can generate duplicate SHAs; dedup by id so the
    // CommitSet's `event_ids` list stays consistent with the event
    // set the engine sees. Deduping by id (not sha) is safe because
    // `deterministic_id` is a function of the sha.
    events.sort_by_key(|e| e.id);
    events.dedup_by_key(|e| e.id);

    let mut by_repo: BTreeMap<String, Vec<&ActivityEvent>> = BTreeMap::new();
    for e in &events {
        let repo = e
            .entities
            .iter()
            .find(|ent| ent.kind == EntityKind::Repo)
            .map(|ent| ent.external_id.clone())
            .unwrap_or_else(|| "/".to_string());
        by_repo.entry(repo).or_default().push(e);
    }

    let artifacts: Vec<Artifact> = by_repo
        .into_iter()
        .map(|(repo, es)| {
            let external_id = format!("{repo}::{}", fixture_date());
            Artifact {
                id: ArtifactId::deterministic(&src, ArtifactKind::CommitSet, &external_id),
                source_id: src,
                kind: ArtifactKind::CommitSet,
                external_id,
                payload: ArtifactPayload::CommitSet {
                    repo_path: PathBuf::from(&repo),
                    date: fixture_date(),
                    event_ids: es.iter().map(|e| e.id).collect(),
                    commit_shas: es.iter().map(|e| e.external_id.clone()).collect(),
                },
                created_at: Utc::now(),
            }
        })
        .collect();

    // `Utc::now()` above only touches artifact `created_at` which
    // the engine does not read for rendering; it only influences
    // `draft.generated_at` indirectly, and that is taken verbatim
    // from the input, not derived. Using `Utc::now()` here keeps
    // the helper simple without threading a fake clock into
    // property tests. The engine's purity is asserted elsewhere.

    (events, artifacts)
}
