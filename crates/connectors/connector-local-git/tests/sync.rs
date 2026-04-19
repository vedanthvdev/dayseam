//! Integration tests that drive the full [`LocalGitConnector`]
//! against runtime-built fixture repositories. Every test maps
//! one-to-one onto an invariant from the Phase 2 plan Task 2
//! §"Invariants proven by tests".

mod common;

use std::collections::HashSet;

use chrono::NaiveDate;
use common::{
    at_utc, build_ctx, build_ctx_with_self, layout_two_repos, make_fixture_repo,
    make_fixture_repo_rebased, mark_private, tz_minus_five, utc_tz, FixtureCommit, RebasedCommit,
    OTHER_EMAIL, SELF_EMAIL,
};
use connector_local_git::{DiscoveryConfig, LocalGitConnector};
use connectors_sdk::{Checkpoint, SourceConnector, SyncRequest};
use dayseam_core::{error_codes, ArtifactPayload, DayseamError, Privacy, SourceKind};
use tempfile::tempdir;

// -------- Invariant 1: discovery is bounded and deterministic --------------

#[tokio::test]
async fn discovery_is_deterministic_and_bounded() {
    let root = tempdir().unwrap();
    let base = root.path();

    for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
        make_fixture_repo(
            &base.join(name),
            &[FixtureCommit {
                author_name: "Me",
                author_email: SELF_EMAIL,
                message: "init",
                when_utc: at_utc(2026, 4, 17, 12, 0),
            }],
        );
    }
    // Non-repo noise that should be ignored.
    std::fs::create_dir_all(base.join("not-a-repo")).unwrap();
    std::fs::create_dir_all(base.join(".hidden")).unwrap();

    let connector = LocalGitConnector::new(vec![base.to_path_buf()], HashSet::new(), utc_tz())
        .with_discovery(DiscoveryConfig {
            max_depth: 4,
            max_roots: 3,
        });

    // Pin the source_id so artefact ids are comparable across the
    // two runs (artefact ids are deterministic in `(source_id,
    // kind, external_id)` — a rotating source_id would make the
    // determinism assertion vacuous).
    let source_id = uuid::Uuid::new_v4();
    let identities = vec![dayseam_core::SourceIdentity {
        id: uuid::Uuid::new_v4(),
        person_id: uuid::Uuid::new_v4(),
        source_id: Some(source_id),
        kind: dayseam_core::SourceIdentityKind::GitEmail,
        external_actor_id: SELF_EMAIL.to_string(),
    }];

    let harness_a = build_ctx(source_id, identities.clone());
    let r_a = connector
        .sync(
            &harness_a.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect("sync ok");
    drop(harness_a);

    let harness_b = build_ctx(source_id, identities);
    let r_b = connector
        .sync(
            &harness_b.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect("sync ok");

    assert_eq!(r_a.artifacts.len(), 3, "bounded at max_roots = 3");
    assert_eq!(r_b.artifacts.len(), 3);

    let ids_a: Vec<_> = r_a.artifacts.iter().map(|a| a.id).collect();
    let ids_b: Vec<_> = r_b.artifacts.iter().map(|a| a.id).collect();
    assert_eq!(ids_a, ids_b, "deterministic artefact ids across runs");
    let paths_a: Vec<_> = r_a
        .artifacts
        .iter()
        .map(|a| a.external_id.clone())
        .collect();
    let paths_b: Vec<_> = r_b
        .artifacts
        .iter()
        .map(|a| a.external_id.clone())
        .collect();
    assert_eq!(paths_a, paths_b, "stable ordering across runs");

    // And a warning log with the `LOCAL_GIT_TOO_MANY_ROOTS` code was emitted.
    drop(harness_b.ctx);
    drop(harness_b.progress_tx);
    drop(harness_b.log_tx);
    drop(harness_b.progress_rx);
    let mut log_rx = harness_b.log_rx;
    let mut codes = Vec::new();
    while let Some(evt) = log_rx.recv().await {
        if let Some(code) = evt.context.get("code").and_then(|v| v.as_str()) {
            codes.push(code.to_string());
        }
    }
    assert!(
        codes
            .iter()
            .any(|c| c == error_codes::LOCAL_GIT_TOO_MANY_ROOTS),
        "expected LOCAL_GIT_TOO_MANY_ROOTS warning, got codes = {codes:?}"
    );
}

// -------- Invariant 2: filter by identity ----------------------------------

#[tokio::test]
async fn sync_filters_by_identity() {
    let root = tempdir().unwrap();
    let t = layout_two_repos(root.path());

    make_fixture_repo(
        &t.mine,
        &[FixtureCommit {
            author_name: "Me",
            author_email: SELF_EMAIL,
            message: "mine: fix thing",
            when_utc: at_utc(2026, 4, 17, 14, 0),
        }],
    );
    make_fixture_repo(
        &t.theirs,
        &[FixtureCommit {
            author_name: "Other",
            author_email: OTHER_EMAIL,
            message: "theirs: different fix",
            when_utc: at_utc(2026, 4, 17, 14, 0),
        }],
    );

    let connector = LocalGitConnector::new(vec![t.scan_root.clone()], HashSet::new(), utc_tz());
    let harness = build_ctx_with_self();
    let day = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
    let r = connector
        .sync(&harness.ctx, SyncRequest::Day(day))
        .await
        .expect("sync ok");

    assert_eq!(r.events.len(), 1, "only my commit should be kept");
    assert_eq!(r.events[0].actor.email.as_deref(), Some(SELF_EMAIL));
    assert_eq!(r.stats.filtered_by_identity, 1);
    assert_eq!(r.stats.fetched_count, 1);
}

/// DAY-52: when commits get silently dropped because their author /
/// committer email isn't in the user's identity list, the sync must
/// surface a warn log pointing the user at the fix. The most common
/// trigger is merge commits made through the GitHub/GitLab web UI
/// (committer becomes a `NNNN+user@users.noreply.github.com`
/// alias); without this log the user sees "15 commits" in git log
/// but a report listing only a subset with no explanation.
#[tokio::test]
async fn sync_warns_when_identity_filter_drops_commits() {
    let root = tempdir().unwrap();
    let t = layout_two_repos(root.path());

    make_fixture_repo(
        &t.mine,
        &[FixtureCommit {
            author_name: "Me",
            author_email: SELF_EMAIL,
            message: "mine: kept",
            when_utc: at_utc(2026, 4, 17, 14, 0),
        }],
    );
    // A commit authored by the GitHub noreply alias — looks like a
    // merge-via-UI commit. This is the exact failure mode DAY-52
    // was filed for.
    make_fixture_repo(
        &t.theirs,
        &[FixtureCommit {
            author_name: "noreply",
            author_email: "61700595+user@users.noreply.github.com",
            message: "Merge pull request #50",
            when_utc: at_utc(2026, 4, 17, 15, 0),
        }],
    );

    let connector = LocalGitConnector::new(vec![t.scan_root.clone()], HashSet::new(), utc_tz());
    let harness = build_ctx_with_self();
    let day = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
    let r = connector
        .sync(&harness.ctx, SyncRequest::Day(day))
        .await
        .expect("sync ok");

    assert_eq!(r.stats.filtered_by_identity, 1);
    assert_eq!(r.events.len(), 1, "only SELF_EMAIL's commit survives");

    // Drain the log channel and verify we emitted the diagnostic.
    drop(harness.ctx);
    drop(harness.progress_tx);
    drop(harness.log_tx);
    drop(harness.progress_rx);
    let mut log_rx = harness.log_rx;
    let mut saw_code = false;
    let mut saw_hint = false;
    while let Some(evt) = log_rx.recv().await {
        if evt
            .context
            .get("code")
            .and_then(|v| v.as_str())
            .is_some_and(|c| c == error_codes::LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY)
        {
            saw_code = true;
            if evt
                .message
                .to_lowercase()
                .contains("users.noreply.github.com")
            {
                saw_hint = true;
            }
            assert_eq!(
                evt.context.get("count").and_then(|v| v.as_u64()),
                Some(1),
                "warn log must carry the dropped-commit count"
            );
        }
    }
    assert!(
        saw_code,
        "expected a LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY warn log"
    );
    assert!(
        saw_hint,
        "warn log message must mention the noreply alias so the user has a fix"
    );
}

// -------- Invariant 3: one CommitSet per (source, repo, day) ---------------

#[tokio::test]
async fn sync_emits_one_commit_set_per_repo_per_day() {
    let root = tempdir().unwrap();
    let t = layout_two_repos(root.path());

    make_fixture_repo(
        &t.mine,
        &[
            FixtureCommit {
                author_name: "Me",
                author_email: SELF_EMAIL,
                message: "c1",
                when_utc: at_utc(2026, 4, 17, 9, 0),
            },
            FixtureCommit {
                author_name: "Me",
                author_email: SELF_EMAIL,
                message: "c2",
                when_utc: at_utc(2026, 4, 17, 10, 0),
            },
            FixtureCommit {
                author_name: "Me",
                author_email: SELF_EMAIL,
                message: "c3",
                when_utc: at_utc(2026, 4, 17, 11, 0),
            },
        ],
    );
    make_fixture_repo(
        &t.theirs,
        &[FixtureCommit {
            author_name: "Me",
            author_email: SELF_EMAIL,
            message: "different-repo commit",
            when_utc: at_utc(2026, 4, 17, 12, 0),
        }],
    );

    let connector = LocalGitConnector::new(vec![t.scan_root.clone()], HashSet::new(), utc_tz());
    let harness = build_ctx_with_self();
    let r = connector
        .sync(
            &harness.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect("sync ok");

    assert_eq!(r.events.len(), 4, "3 commits in mine + 1 in theirs");
    assert_eq!(r.artifacts.len(), 2, "one CommitSet per repo/day");

    for artefact in &r.artifacts {
        match &artefact.payload {
            ArtifactPayload::CommitSet {
                repo_path,
                date,
                event_ids,
                commit_shas,
            } => {
                assert_eq!(*date, NaiveDate::from_ymd_opt(2026, 4, 17).unwrap());
                assert!(repo_path.starts_with(&t.scan_root));
                assert_eq!(event_ids.len(), commit_shas.len());
                assert!(!event_ids.is_empty());
            }
        }
    }

    // The two artefact ids are distinct and deterministic for this
    // (source, repo, day) triple.
    let ids: HashSet<_> = r.artifacts.iter().map(|a| a.id).collect();
    assert_eq!(ids.len(), 2);
}

// -------- Invariant 4: timezone correctness --------------------------------

#[tokio::test]
async fn sync_uses_local_tz_for_day_window() {
    let root = tempdir().unwrap();
    let repo = root.path().join("night-owl");
    // Commit at 01:00 UTC on 2026-04-18 ⇒ 20:00 local on 2026-04-17
    // when the user is in UTC-5.
    make_fixture_repo(
        &repo,
        &[FixtureCommit {
            author_name: "Me",
            author_email: SELF_EMAIL,
            message: "late night",
            when_utc: at_utc(2026, 4, 18, 1, 0),
        }],
    );

    let connector = LocalGitConnector::new(
        vec![root.path().to_path_buf()],
        HashSet::new(),
        tz_minus_five(),
    );
    let harness = build_ctx_with_self();
    let r = connector
        .sync(
            &harness.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect("sync ok");

    assert_eq!(
        r.events.len(),
        1,
        "late-night commit should roll up to day D"
    );
    // And the inverse: if the user asks for day D+1 the commit should NOT appear.
    let harness2 = build_ctx_with_self();
    let r2 = connector
        .sync(
            &harness2.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 18).unwrap()),
        )
        .await
        .expect("sync ok");
    assert_eq!(r2.events.len(), 0);
}

// -------- Invariant 5: private-repo redaction ------------------------------

#[tokio::test]
async fn sync_redacts_private_repos() {
    let root = tempdir().unwrap();
    let repo = root.path().join("secret-stuff");
    make_fixture_repo(
        &repo,
        &[FixtureCommit {
            author_name: "Me",
            author_email: SELF_EMAIL,
            message: "fix: rotate secret keys\n\nsee PRIVATE-42",
            when_utc: at_utc(2026, 4, 17, 10, 0),
        }],
    );
    mark_private(&repo);

    let connector =
        LocalGitConnector::new(vec![root.path().to_path_buf()], HashSet::new(), utc_tz());
    let harness = build_ctx_with_self();
    let r = connector
        .sync(
            &harness.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect("sync ok");

    assert_eq!(r.events.len(), 1);
    let e = &r.events[0];
    assert_eq!(e.privacy, Privacy::RedactedPrivateRepo);
    assert_eq!(e.title, "");
    assert_eq!(e.body, None);
    assert!(e.raw_ref.storage_key.starts_with("redacted:"));
    // Identity + SHA are preserved.
    assert_eq!(e.actor.email.as_deref(), Some(SELF_EMAIL));
    assert!(!e.external_id.is_empty());
}

// -------- Invariant 6: cancellation is prompt ------------------------------

#[tokio::test]
async fn sync_aborts_promptly_on_cancel() {
    let root = tempdir().unwrap();
    // Populate enough repos that a few iterations must happen.
    for i in 0..5 {
        make_fixture_repo(
            &root.path().join(format!("r{i}")),
            &[FixtureCommit {
                author_name: "Me",
                author_email: SELF_EMAIL,
                message: "init",
                when_utc: at_utc(2026, 4, 17, 12, 0),
            }],
        );
    }

    let connector =
        LocalGitConnector::new(vec![root.path().to_path_buf()], HashSet::new(), utc_tz());
    let harness = build_ctx_with_self();
    // Trip cancellation before the call. The connector must bail
    // with `Cancelled` rather than silently returning an empty
    // result.
    harness.cancel.cancel();

    let start = std::time::Instant::now();
    let err = connector
        .sync(
            &harness.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect_err("cancelled");
    let elapsed = start.elapsed();

    assert!(
        matches!(err, DayseamError::Cancelled { .. }),
        "expected Cancelled, got {err:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "cancellation took {elapsed:?}"
    );
}

// -------- Invariant 7: Range/Since return Unsupported ----------------------

#[tokio::test]
async fn sync_unsupported_variants_return_unsupported() {
    let root = tempdir().unwrap();
    let connector =
        LocalGitConnector::new(vec![root.path().to_path_buf()], HashSet::new(), utc_tz());

    let harness = build_ctx_with_self();
    let err = connector
        .sync(
            &harness.ctx,
            SyncRequest::Range {
                start: NaiveDate::from_ymd_opt(2026, 4, 17).unwrap(),
                end: NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(),
            },
        )
        .await
        .expect_err("range unsupported");
    assert!(matches!(err, DayseamError::Unsupported { .. }));
    assert_eq!(err.code(), error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST);

    let harness2 = build_ctx_with_self();
    let err2 = connector
        .sync(
            &harness2.ctx,
            SyncRequest::Since(Checkpoint {
                connector: "local-git".into(),
                value: serde_json::json!({}),
            }),
        )
        .await
        .expect_err("since unsupported");
    assert!(matches!(err2, DayseamError::Unsupported { .. }));
    assert_eq!(err2.code(), error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST);
}

// -------- Invariant 4 (committer-time bucketing, Phase 2 DAY-50) -----------
// A commit authored last week but committed today belongs in today's
// report. The walker keys on committer-time, not author-time.

#[tokio::test]
async fn sync_buckets_by_committer_time_not_author_time() {
    let root = tempdir().unwrap();
    let repo = root.path().join("rebased");
    make_fixture_repo_rebased(
        &repo,
        &[RebasedCommit {
            author_name: "Me",
            author_email: SELF_EMAIL,
            committer_name: "Me",
            committer_email: SELF_EMAIL,
            message: "authored last week, rebased onto main today",
            author_when_utc: at_utc(2026, 4, 10, 9, 0),
            committer_when_utc: at_utc(2026, 4, 17, 14, 0),
        }],
    );

    let connector =
        LocalGitConnector::new(vec![root.path().to_path_buf()], HashSet::new(), utc_tz());
    let harness = build_ctx_with_self();
    let r = connector
        .sync(
            &harness.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect("sync ok");

    assert_eq!(
        r.events.len(),
        1,
        "committer-time falls on the requested day → commit kept"
    );
    assert_eq!(r.stats.filtered_by_date, 0);

    // Inverse: a request for the author's original day should return
    // zero (committer-time is the authority).
    let harness2 = build_ctx_with_self();
    let r2 = connector
        .sync(
            &harness2.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()),
        )
        .await
        .expect("sync ok");
    assert_eq!(r2.events.len(), 0, "author-time day must not match");
}

// -------- Invariant 2 (identity filter matches author OR committer) --------
// A commit where the committer email is self but the author email is
// someone else (common when merging a co-worker's PR locally) should
// be kept. Filtering on author-only was the Phase 1 bug.

#[tokio::test]
async fn sync_identity_filter_matches_committer_when_author_differs() {
    let root = tempdir().unwrap();
    let repo = root.path().join("merged-pr");
    make_fixture_repo_rebased(
        &repo,
        &[RebasedCommit {
            author_name: "Open Source Contributor",
            author_email: OTHER_EMAIL,
            committer_name: "Me",
            committer_email: SELF_EMAIL,
            message: "merge: OSS PR onto main",
            author_when_utc: at_utc(2026, 4, 17, 10, 0),
            committer_when_utc: at_utc(2026, 4, 17, 14, 0),
        }],
    );

    let connector =
        LocalGitConnector::new(vec![root.path().to_path_buf()], HashSet::new(), utc_tz());
    let harness = build_ctx_with_self();
    let r = connector
        .sync(
            &harness.ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()),
        )
        .await
        .expect("sync ok");

    assert_eq!(
        r.events.len(),
        1,
        "committer email matches self identity → commit kept even though author doesn't"
    );
    // The actor surfaced on the event is the matched identity (committer).
    assert_eq!(r.events[0].actor.email.as_deref(), Some(SELF_EMAIL));
    assert_eq!(r.stats.filtered_by_identity, 0);
}

// -------- Invariant 4 inverse (malformed timestamps drop, not surface today) ---
// A commit with an out-of-range committer timestamp must not be
// silently bucketed to "today" via a `Utc::now()` fallback. Phase 1
// did this via `.single().unwrap_or_else(Utc::now)`; Phase 2 drops
// the commit into `filtered_by_date` instead.
//
// `git2::Time::seconds()` returns `i64` and libgit2 accepts a wide
// range, so we can't easily forge a "malformed" value through the
// public fixture API. Instead we prove the behaviour via a direct
// unit test on `commit_timestamp_utc` in `walk.rs`'s test module —
// this file remains the integration surface.

// -------- Kind + healthcheck (not in the plan's numbered list but cheap) ---

#[tokio::test]
async fn kind_is_local_git_and_healthcheck_reports_ok() {
    let root = tempdir().unwrap();
    let connector =
        LocalGitConnector::new(vec![root.path().to_path_buf()], HashSet::new(), utc_tz());
    assert_eq!(connector.kind(), SourceKind::LocalGit);

    let harness = build_ctx_with_self();
    let health = connector.healthcheck(&harness.ctx).await.expect("health");
    assert!(health.ok);
}
