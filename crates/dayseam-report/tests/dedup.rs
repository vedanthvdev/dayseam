//! Plan Task 2 invariants for `dedup_commit_authored`.
//!
//! The engine-facing tests in `src/dedup.rs` assert local correctness
//! (private helpers, single-function unit tests); this file asserts the
//! five invariants the Phase 3 plan lists as the public contract, and
//! threads the helper through the real fixture builders from
//! `tests/common` so collisions come from the same event shape a
//! connector would emit.

mod common;

use chrono::{TimeZone, Utc};
use common::{commit_event, fixture_date, source_id};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityRef, Link, Privacy, RawRef, SourceId,
};
use dayseam_report::dedup_commit_authored;
use uuid::Uuid;

/// Build a minimal `CommitAuthored` event keyed by `(source, sha)`.
/// Defaults to `Privacy::Normal`; callers override fields they care
/// about. Kept local so this file's tests stay readable without
/// teaching the common fixture module about every edge case.
fn commit(source: SourceId, sha: &str, body: Option<&str>, privacy: Privacy) -> ActivityEvent {
    let kind_token = "CommitAuthored";
    ActivityEvent {
        id: ActivityEvent::deterministic_id(&source.to_string(), sha, kind_token),
        source_id: source,
        external_id: sha.into(),
        kind: ActivityKind::CommitAuthored,
        occurred_at: Utc.with_ymd_and_hms(2026, 4, 18, 10, 0, 0).unwrap(),
        actor: Actor {
            display_name: "Self".into(),
            email: Some("self@example.com".into()),
            external_id: None,
        },
        title: format!("commit {sha}"),
        body: body.map(str::to_string),
        links: Vec::new(),
        entities: Vec::new(),
        parent_external_id: None,
        metadata: serde_json::Value::Null,
        raw_ref: RawRef {
            storage_key: format!("k:{sha}"),
            content_type: "application/x-git-commit".into(),
        },
        privacy,
    }
}

/// Plan invariant 1. `dedup(a ++ b) == dedup(b ++ a)` after sort: the
/// set of canonical survivors is invariant to input permutation.
#[test]
fn dedup_is_set_like_under_permutation() {
    let a = source_id(1);
    let b = source_id(2);
    let e_a = commit(a, "sha1", Some("short"), Privacy::Normal);
    let e_b = commit(b, "sha1", Some("richer body"), Privacy::Normal);
    let e_local_only = commit(a, "sha2", None, Privacy::Normal);

    let mut forward = dedup_commit_authored(vec![e_a.clone(), e_b.clone(), e_local_only.clone()]);
    let mut reverse = dedup_commit_authored(vec![e_local_only, e_b, e_a]);
    let sort_by_sha =
        |v: &mut Vec<ActivityEvent>| v.sort_by(|x, y| x.external_id.cmp(&y.external_id));
    sort_by_sha(&mut forward);
    sort_by_sha(&mut reverse);

    let forward_survivors: Vec<(String, SourceId)> = forward
        .iter()
        .map(|e| (e.external_id.clone(), e.source_id))
        .collect();
    let reverse_survivors: Vec<(String, SourceId)> = reverse
        .iter()
        .map(|e| (e.external_id.clone(), e.source_id))
        .collect();
    assert_eq!(forward_survivors, reverse_survivors);
}

/// Plan invariant 2. Dedup never invents or loses a SHA — the set of
/// `(kind, external_id)` pairs on the output equals the set on the
/// input, restricted to `CommitAuthored`.
#[test]
fn dedup_preserves_the_input_sha_set() {
    let a = source_id(1);
    let b = source_id(2);
    let input = vec![
        commit(a, "sha1", None, Privacy::Normal),
        commit(b, "sha1", Some("richer"), Privacy::Normal),
        commit(a, "sha2", None, Privacy::Normal),
        commit(b, "sha3", None, Privacy::Normal),
    ];
    let input_shas: std::collections::BTreeSet<String> =
        input.iter().map(|e| e.external_id.clone()).collect();

    let out = dedup_commit_authored(input);
    let out_shas: std::collections::BTreeSet<String> =
        out.iter().map(|e| e.external_id.clone()).collect();
    assert_eq!(out_shas, input_shas);
}

/// Plan invariant 3. Dedup picks the richer body. Tie-break is
/// lex-smallest `source_id` so the choice is deterministic.
#[test]
fn dedup_picks_richer_body_with_lex_tiebreak() {
    let a = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00AA);
    let b = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00BB);
    let short = commit(a, "sha1", Some("short"), Privacy::Normal);
    let rich = commit(b, "sha1", Some("a far more detailed body"), Privacy::Normal);

    let out = dedup_commit_authored(vec![short.clone(), rich.clone()]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source_id, b, "richer body wins");
    assert_eq!(out[0].body.as_deref(), rich.body.as_deref());

    let tie_a = commit(a, "sha2", Some("same"), Privacy::Normal);
    let tie_b = commit(b, "sha2", Some("same"), Privacy::Normal);
    let out = dedup_commit_authored(vec![tie_b, tie_a]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source_id, a, "lex-smallest wins on body tie");
}

/// Plan invariant 4. Dedup unions `links` and `entities`.
#[test]
fn dedup_unions_links_and_entities() {
    let a = source_id(1);
    let b = source_id(2);
    let mut e_a = commit(a, "sha1", Some("short"), Privacy::Normal);
    e_a.links = vec![Link {
        url: "file:///repo/.git".into(),
        label: Some("local-git".into()),
    }];
    e_a.entities = vec![EntityRef {
        kind: "repo".into(),
        external_id: "/work/foo".into(),
        label: None,
    }];
    let mut e_b = commit(b, "sha1", Some("a far more detailed body"), Privacy::Normal);
    e_b.links = vec![Link {
        url: "https://gitlab/commit/sha1".into(),
        label: Some("gitlab".into()),
    }];
    e_b.entities = vec![EntityRef {
        kind: "project".into(),
        external_id: "42".into(),
        label: Some("payments".into()),
    }];

    let out = dedup_commit_authored(vec![e_a, e_b]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].links.len(), 2, "links unioned");
    assert_eq!(out[0].entities.len(), 2, "entities unioned");
}

/// Plan invariant 5. Dedup respects `RedactedPrivateRepo` — the
/// louder side wins. If local-git flagged the repo private, the
/// GitLab-enriched row inherits the redaction.
#[test]
fn dedup_respects_redacted_private_repo() {
    let a = source_id(1);
    let b = source_id(2);
    let redacted_local = commit(a, "sha1", Some("short"), Privacy::RedactedPrivateRepo);
    let enriched_gitlab = commit(b, "sha1", Some("long"), Privacy::Normal);

    let out = dedup_commit_authored(vec![enriched_gitlab, redacted_local]);
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0].privacy,
        Privacy::RedactedPrivateRepo,
        "private repo wins regardless of which side has the richer body"
    );
}

/// Dedup is idempotent: running it twice is the same as running it
/// once. This prevents a future refactor from sneaking a non-stable
/// canonical-survivor pick in without a test failure.
#[test]
fn dedup_is_idempotent() {
    let a = source_id(1);
    let b = source_id(2);
    let input = vec![
        commit(a, "sha1", None, Privacy::Normal),
        commit(b, "sha1", Some("gitlab"), Privacy::Normal),
        commit(a, "sha2", None, Privacy::Normal),
    ];
    let once = dedup_commit_authored(input);
    let twice = dedup_commit_authored(once.clone());
    assert_eq!(once, twice);
}

/// Dedup only touches `CommitAuthored` rows — every other
/// `ActivityKind` passes through untouched. This guards against a
/// future fan-out of the helper into a generic dedup.
#[test]
fn dedup_passes_through_non_commit_authored_events() {
    let s = source_id(1);
    let mr = ActivityEvent {
        kind: ActivityKind::MrOpened,
        ..commit(s, "!42", Some("open"), Privacy::Normal)
    };
    let issue = ActivityEvent {
        kind: ActivityKind::IssueOpened,
        ..commit(s, "#7", Some("open"), Privacy::Normal)
    };
    let com = commit(s, "sha1", None, Privacy::Normal);

    let out = dedup_commit_authored(vec![mr.clone(), issue.clone(), com.clone()]);
    assert_eq!(out.len(), 3);
    assert!(out.iter().any(|e| e.kind == ActivityKind::MrOpened));
    assert!(out.iter().any(|e| e.kind == ActivityKind::IssueOpened));
    assert!(out.iter().any(|e| e.kind == ActivityKind::CommitAuthored));
}

/// Smoke test that runs the real `commit_event` fixture builder from
/// `tests/common` through the dedup pass, guaranteeing the helper
/// tolerates the exact event shape the local-git connector emits.
#[test]
fn commit_event_fixture_is_dedup_compatible() {
    let src_a = source_id(3);
    let src_b = source_id(4);
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let repo = "/work/dayseam";
    let e_a = commit_event(
        src_a,
        sha,
        repo,
        "self@example.com",
        9,
        "Refactor feature flag plumbing",
        Privacy::Normal,
    );
    let e_b = commit_event(
        src_b,
        sha,
        repo,
        "self@example.com",
        9,
        "Refactor feature flag plumbing",
        Privacy::Normal,
    );

    let out = dedup_commit_authored(vec![e_a, e_b]);
    assert_eq!(out.len(), 1, "same SHA → one surviving event");
    assert_eq!(out[0].external_id, sha);
    // The fixture uses a single repo entity; the union collapses the
    // duplicate rather than emitting it twice.
    assert_eq!(out[0].entities.len(), 1);
    let _ = fixture_date();
}
