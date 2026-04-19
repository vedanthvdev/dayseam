//! Integration tests for every `dayseam-db` repository. These are
//! deliberately written against a real SQLite file in a `tempdir` rather
//! than an in-memory database — we want the pragmas, migrations, and FK
//! cascades to execute exactly as they will in production.

use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, Artifact, ArtifactId, ArtifactKind, ArtifactPayload,
    EntityRef, Evidence, Identity, Link, LocalRepo, LogLevel, PerSourceState, Person, Privacy,
    RawRef, RenderedBullet, RenderedSection, ReportDraft, RunId, RunStatus, SecretRef, Source,
    SourceConfig, SourceHealth, SourceIdentity, SourceIdentityKind, SourceKind, SourceRunState,
    SyncRun, SyncRunCancelReason, SyncRunStatus, SyncRunTrigger,
};
use dayseam_db::{
    open, ActivityRepo, ArtifactRepo, DbError, DraftRepo, IdentityRepo, LocalRepoRepo, LogRepo,
    LogRow, PersonRepo, RawPayload, RawPayloadRepo, SettingsRepo, SourceIdentityRepo, SourceRepo,
    SyncRunRepo,
};
use sqlx::SqlitePool;
use tempfile::TempDir;
use uuid::Uuid;

async fn test_pool() -> (SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let pool = open(&path).await.expect("open");
    (pool, dir)
}

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap()
}

fn fixture_source() -> Source {
    Source {
        id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
        kind: SourceKind::GitLab,
        label: "gitlab.internal.acme.com".into(),
        config: SourceConfig::GitLab {
            base_url: "https://gitlab.internal.acme.com".into(),
            user_id: 42,
            username: "vedanthv".into(),
        },
        secret_ref: Some(SecretRef {
            keychain_service: "app.dayseam.desktop".into(),
            keychain_account: "gitlab:11111111".into(),
        }),
        created_at: fixed_now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    }
}

fn fixture_local_source() -> Source {
    Source {
        id: Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
        kind: SourceKind::LocalGit,
        label: "Work laptop".into(),
        config: SourceConfig::LocalGit {
            scan_roots: vec![PathBuf::from("/Users/v/Code")],
        },
        secret_ref: None,
        created_at: fixed_now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    }
}

fn fixture_event_for(src: &Source) -> ActivityEvent {
    let external_id = "123".to_string();
    let kind_str = "MrOpened";
    ActivityEvent {
        id: ActivityEvent::deterministic_id(&src.id.to_string(), &external_id, kind_str),
        source_id: src.id,
        external_id,
        kind: ActivityKind::MrOpened,
        occurred_at: fixed_now(),
        actor: Actor {
            display_name: "Vedanth".into(),
            email: Some("vedanth@example.com".into()),
            external_id: Some("42".into()),
        },
        title: "feat: land activity store".into(),
        body: Some("Closes #9".into()),
        links: vec![Link {
            url: "https://gitlab.internal.acme.com/x/-/merge_requests/123".into(),
            label: Some("!123".into()),
        }],
        entities: vec![EntityRef {
            kind: "mr".into(),
            external_id: "123".into(),
            label: Some("!123".into()),
        }],
        parent_external_id: None,
        metadata: serde_json::json!({ "state": "opened", "iid": 123 }),
        raw_ref: RawRef {
            storage_key: "raw://gitlab/123".into(),
            content_type: "application/json".into(),
        },
        privacy: Privacy::Normal,
    }
}

#[tokio::test]
async fn pool_is_idempotent_and_pragmas_are_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let pool = open(&path).await.unwrap();

    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(journal.to_lowercase(), "wal");

    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(fk, 1);

    drop(pool);
    let pool2 = open(&path).await.expect("second open");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources")
        .fetch_one(&pool2)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn sources_round_trip_and_delete_cascades() {
    let (pool, _dir) = test_pool().await;
    let repo = SourceRepo::new(pool.clone());
    let events = ActivityRepo::new(pool.clone());
    let raws = RawPayloadRepo::new(pool.clone());

    let src = fixture_source();
    repo.insert(&src).await.unwrap();

    let got = repo.get(&src.id).await.unwrap().expect("present");
    assert_eq!(got, src);

    let listed = repo.list().await.unwrap();
    assert_eq!(listed, vec![src.clone()]);

    events
        .insert_many(&[fixture_event_for(&src)])
        .await
        .unwrap();
    raws.insert(&RawPayload {
        id: Uuid::new_v4(),
        source_id: src.id,
        endpoint: "/api/v4/events".into(),
        fetched_at: fixed_now(),
        payload_json: "{}".into(),
        payload_sha256: "0".repeat(64),
    })
    .await
    .unwrap();

    repo.delete(&src.id).await.unwrap();
    let leftover_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM activity_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leftover_events, 0, "activity_events should cascade");
    let leftover_raws: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM raw_payloads")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leftover_raws, 0, "raw_payloads should cascade");
}

#[tokio::test]
async fn source_update_health_persists() {
    let (pool, _dir) = test_pool().await;
    let repo = SourceRepo::new(pool.clone());
    let src = fixture_source();
    repo.insert(&src).await.unwrap();

    let updated = SourceHealth {
        ok: false,
        checked_at: Some(fixed_now()),
        last_error: None,
    };
    repo.update_health(&src.id, &updated).await.unwrap();
    let got = repo.get(&src.id).await.unwrap().unwrap();
    assert_eq!(got.last_health, updated);
    assert_eq!(got.last_sync_at, Some(fixed_now()));
}

#[tokio::test]
async fn identities_round_trip_and_update() {
    let (pool, _dir) = test_pool().await;
    let repo = IdentityRepo::new(pool);
    let id = Identity {
        id: Uuid::new_v4(),
        emails: vec!["vedanth@work.example".into(), "v@personal.example".into()],
        gitlab_user_ids: vec![42],
        display_name: "Vedanth V".into(),
    };
    repo.insert(&id).await.unwrap();

    let listed = repo.list().await.unwrap();
    assert_eq!(listed, vec![id.clone()]);

    let mut updated = id.clone();
    updated.display_name = "Vedanth Vasudev".into();
    repo.update(&updated).await.unwrap();
    assert_eq!(repo.list().await.unwrap(), vec![updated]);
}

#[tokio::test]
async fn local_repos_upsert_and_private_toggle() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let local = LocalRepoRepo::new(pool.clone());
    let src = fixture_local_source();
    sources.insert(&src).await.unwrap();

    let repo = LocalRepo {
        path: PathBuf::from("/Users/v/Code/dayseam"),
        label: "dayseam".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    local.upsert(&src.id, &repo).await.unwrap();
    local.upsert(&src.id, &repo).await.unwrap();

    let listed = local.list_for_source(&src.id).await.unwrap();
    assert_eq!(listed, vec![repo.clone()]);

    local.set_is_private(&repo.path, true).await.unwrap();
    let after = local.list_for_source(&src.id).await.unwrap();
    assert!(after[0].is_private);

    sources.delete(&src.id).await.unwrap();
    assert!(local.list_for_source(&src.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn activity_events_round_trip_and_unique() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let events = ActivityRepo::new(pool.clone());

    let src = fixture_source();
    sources.insert(&src).await.unwrap();
    let event = fixture_event_for(&src);
    events.insert_many(&[event.clone()]).await.unwrap();

    let date = event.occurred_at.date_naive();
    let got = events.list_by_source_date(&src.id, date).await.unwrap();
    assert_eq!(got, vec![event.clone()]);

    let err = events.insert_many(&[event.clone()]).await.unwrap_err();
    assert!(
        matches!(err, DbError::Conflict { .. }),
        "expected Conflict, got {err:?}"
    );
}

#[tokio::test]
async fn activity_events_filter_by_utc_date() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let events = ActivityRepo::new(pool.clone());
    let src = fixture_source();
    sources.insert(&src).await.unwrap();

    let mut e1 = fixture_event_for(&src);
    e1.external_id = "one".into();
    e1.id = ActivityEvent::deterministic_id(&src.id.to_string(), &e1.external_id, "MrOpened");
    e1.occurred_at = Utc.with_ymd_and_hms(2026, 4, 17, 1, 0, 0).unwrap();

    let mut e2 = fixture_event_for(&src);
    e2.external_id = "two".into();
    e2.id = ActivityEvent::deterministic_id(&src.id.to_string(), &e2.external_id, "MrOpened");
    e2.occurred_at = Utc.with_ymd_and_hms(2026, 4, 18, 1, 0, 0).unwrap();

    events.insert_many(&[e1.clone(), e2.clone()]).await.unwrap();

    let day17 = events
        .list_by_source_date(&src.id, NaiveDate::from_ymd_opt(2026, 4, 17).unwrap())
        .await
        .unwrap();
    assert_eq!(day17.len(), 1);
    assert_eq!(day17[0].external_id, "one");
}

#[tokio::test]
async fn raw_payloads_insert_get_and_prune() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let raws = RawPayloadRepo::new(pool.clone());
    let src = fixture_source();
    sources.insert(&src).await.unwrap();

    let old = RawPayload {
        id: Uuid::new_v4(),
        source_id: src.id,
        endpoint: "/api/v4/events".into(),
        fetched_at: fixed_now() - Duration::days(40),
        payload_json: "{\"a\":1}".into(),
        payload_sha256: "a".repeat(64),
    };
    let fresh = RawPayload {
        id: Uuid::new_v4(),
        source_id: src.id,
        endpoint: "/api/v4/events".into(),
        fetched_at: fixed_now() - Duration::days(1),
        payload_json: "{\"a\":2}".into(),
        payload_sha256: "b".repeat(64),
    };
    raws.insert(&old).await.unwrap();
    raws.insert(&fresh).await.unwrap();

    assert_eq!(raws.get(&old.id).await.unwrap().unwrap(), old);
    let pruned = raws
        .prune_older_than(fixed_now() - Duration::days(30))
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    assert!(raws.get(&old.id).await.unwrap().is_none());
    assert!(raws.get(&fresh.id).await.unwrap().is_some());
}

#[tokio::test]
async fn drafts_insert_list_recent_and_prune() {
    let (pool, _dir) = test_pool().await;
    let repo = DraftRepo::new(pool);

    let draft = ReportDraft {
        id: Uuid::new_v4(),
        date: NaiveDate::from_ymd_opt(2026, 4, 17).unwrap(),
        template_id: "dev_eod".into(),
        template_version: "1.0.0".into(),
        sections: vec![RenderedSection {
            id: "completed".into(),
            title: "Completed".into(),
            bullets: vec![RenderedBullet {
                id: "b1".into(),
                text: "Shipped the DB layer".into(),
            }],
        }],
        evidence: vec![Evidence {
            bullet_id: "b1".into(),
            event_ids: vec![Uuid::new_v4()],
            reason: "1 MR".into(),
        }],
        per_source_state: std::collections::HashMap::new(),
        verbose_mode: false,
        generated_at: fixed_now(),
    };
    repo.insert(&draft).await.unwrap();

    let got = repo.get(&draft.id).await.unwrap().unwrap();
    assert_eq!(got, draft);

    let recent = repo.list_recent(10).await.unwrap();
    assert_eq!(recent.len(), 1);

    let pruned = repo
        .prune_older_than(fixed_now() + Duration::days(1))
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    assert!(repo.get(&draft.id).await.unwrap().is_none());
}

#[tokio::test]
async fn logs_append_tail_prune_and_system_source() {
    let (pool, _dir) = test_pool().await;
    let repo = LogRepo::new(pool);

    let system_row = LogRow {
        ts: fixed_now(),
        level: LogLevel::Info,
        source_id: None,
        message: "startup complete".into(),
        context: Some(serde_json::json!({ "version": "0.0.0" })),
    };
    let source_id = Uuid::new_v4();
    let scoped_row = LogRow {
        ts: fixed_now() + Duration::seconds(1),
        level: LogLevel::Warn,
        source_id: Some(source_id),
        message: "retrying fetch".into(),
        context: None,
    };
    repo.append(&system_row).await.unwrap();
    repo.append(&scoped_row).await.unwrap();

    let tail = repo
        .tail(fixed_now() - Duration::hours(1), 100)
        .await
        .unwrap();
    assert_eq!(tail.len(), 2);
    // `tail` returns newest-first so the scoped row (ts + 1s) comes
    // before the system row.
    assert_eq!(tail[0], scoped_row);
    assert_eq!(tail[1], system_row);

    // A tight limit must keep the newest row, not discard it.
    let newest_only = repo
        .tail(fixed_now() - Duration::hours(1), 1)
        .await
        .unwrap();
    assert_eq!(newest_only, vec![scoped_row.clone()]);

    let pruned = repo
        .prune_older_than(fixed_now() + Duration::seconds(2))
        .await
        .unwrap();
    assert_eq!(pruned, 2);
}

#[tokio::test]
async fn settings_get_set_round_trip() {
    let (pool, _dir) = test_pool().await;
    let repo = SettingsRepo::new(pool);

    assert!(repo.get::<String>("theme").await.unwrap().is_none());
    repo.set("theme", &"dark".to_string()).await.unwrap();
    repo.set("theme", &"light".to_string()).await.unwrap();
    assert_eq!(
        repo.get::<String>("theme").await.unwrap(),
        Some("light".into())
    );

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Window {
        width: u32,
        height: u32,
    }
    repo.set(
        "window",
        &Window {
            width: 1280,
            height: 720,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        repo.get::<Window>("window").await.unwrap(),
        Some(Window {
            width: 1280,
            height: 720,
        })
    );
}

#[tokio::test]
async fn run_status_and_source_run_state_round_trip_via_draft() {
    let (pool, _dir) = test_pool().await;
    let repo = DraftRepo::new(pool);
    let src_id = Uuid::new_v4();
    let mut state_map = std::collections::HashMap::new();
    state_map.insert(
        src_id,
        SourceRunState {
            status: RunStatus::Succeeded,
            started_at: fixed_now(),
            finished_at: Some(fixed_now() + Duration::seconds(3)),
            fetched_count: 12,
            error: None,
        },
    );
    let draft = ReportDraft {
        id: Uuid::new_v4(),
        date: NaiveDate::from_ymd_opt(2026, 4, 17).unwrap(),
        template_id: "dev_eod".into(),
        template_version: "1.0.0".into(),
        sections: vec![],
        evidence: vec![],
        per_source_state: state_map,
        verbose_mode: true,
        generated_at: fixed_now(),
    };
    repo.insert(&draft).await.unwrap();
    let got = repo.get(&draft.id).await.unwrap().unwrap();
    assert_eq!(got, draft);
    // `_` silences clippy::items_after_test_module warnings when no
    // `fixture_*` helpers are used further down the file.
    let _ = (SourceKind::GitLab, SourceKind::LocalGit);
}

// ---------------------------------------------------------------------------
// Phase 2: artifacts, sync_runs, persons, source_identities
// ---------------------------------------------------------------------------

/// Reopening a database must not drop data and must be a no-op on the
/// second reopen. Separately we pin the Phase 1 → Phase 2 upgrade
/// story: `PersonRepo::bootstrap_from_identity` promotes the legacy
/// `identities` row to the canonical self-`Person` without losing the
/// original UUID.
#[tokio::test]
async fn migrations_are_additive_and_idempotent_across_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");

    let pool = open(&path).await.unwrap();
    let sources = SourceRepo::new(pool.clone());
    let identities = IdentityRepo::new(pool.clone());
    let src = fixture_source();
    sources.insert(&src).await.unwrap();
    let identity = Identity {
        id: Uuid::new_v4(),
        emails: vec!["vedanth@work.example".into()],
        gitlab_user_ids: vec![42],
        display_name: "Vedanth".into(),
    };
    identities.insert(&identity).await.unwrap();
    drop(pool);

    let pool2 = open(&path).await.expect("reopen 1");
    let sources2 = SourceRepo::new(pool2.clone());
    assert_eq!(sources2.list().await.unwrap(), vec![src.clone()]);

    let persons = PersonRepo::new(pool2.clone());
    let me = persons
        .bootstrap_from_identity(&identity)
        .await
        .expect("bootstrap from identity");
    assert!(me.is_self);
    assert_eq!(me.id, identity.id);
    assert_eq!(me.display_name, identity.display_name);

    // A second call is a no-op; it must find the same row rather than
    // inserting another self-`Person`.
    let again = persons
        .bootstrap_from_identity(&identity)
        .await
        .expect("second bootstrap");
    assert_eq!(again, me);

    drop(pool2);
    let pool3 = open(&path).await.expect("reopen 2");
    let persons3 = PersonRepo::new(pool3);
    assert_eq!(persons3.list().await.unwrap().len(), 1, "no duplicates");
}

#[tokio::test]
async fn artifacts_upsert_round_trip_and_cascade() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let artifacts = ArtifactRepo::new(pool.clone());
    let src = fixture_local_source();
    sources.insert(&src).await.unwrap();

    let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
    let external_id = format!("/Users/v/Code/dayseam@{date}");
    let id = ArtifactId::deterministic(&src.id, ArtifactKind::CommitSet, &external_id);
    let artifact = Artifact {
        id,
        source_id: src.id,
        kind: ArtifactKind::CommitSet,
        external_id: external_id.clone(),
        payload: ArtifactPayload::CommitSet {
            repo_path: PathBuf::from("/Users/v/Code/dayseam"),
            date,
            event_ids: vec![Uuid::new_v4()],
            commit_shas: vec!["abc123".into()],
        },
        created_at: fixed_now(),
    };

    artifacts.upsert(&artifact).await.unwrap();
    assert_eq!(artifacts.get(&id).await.unwrap().unwrap(), artifact);

    // Upsert is idempotent and replaces payload in place.
    let mut replaced = artifact.clone();
    replaced.payload = ArtifactPayload::CommitSet {
        repo_path: PathBuf::from("/Users/v/Code/dayseam"),
        date,
        event_ids: vec![],
        commit_shas: vec!["def456".into()],
    };
    artifacts.upsert(&replaced).await.unwrap();
    assert_eq!(artifacts.get(&id).await.unwrap().unwrap(), replaced);

    let same_date = artifacts.list_for_source_date(&src.id, date).await.unwrap();
    assert_eq!(same_date.len(), 1);
    let other_date = artifacts
        .list_for_source_date(&src.id, NaiveDate::from_ymd_opt(2026, 4, 18).unwrap())
        .await
        .unwrap();
    assert!(other_date.is_empty());

    sources.delete(&src.id).await.unwrap();
    assert!(
        artifacts.get(&id).await.unwrap().is_none(),
        "artifact should cascade"
    );
}

fn fixture_running_run() -> SyncRun {
    SyncRun {
        id: RunId::new(),
        started_at: fixed_now(),
        finished_at: None,
        trigger: SyncRunTrigger::User,
        status: SyncRunStatus::Running,
        cancel_reason: None,
        superseded_by: None,
        per_source_state: vec![],
    }
}

#[tokio::test]
async fn sync_runs_running_to_completed() {
    let (pool, _dir) = test_pool().await;
    let repo = SyncRunRepo::new(pool);
    let run = fixture_running_run();
    repo.insert(&run).await.unwrap();

    let src_id = Uuid::new_v4();
    let per_source = vec![PerSourceState {
        source_id: src_id,
        status: RunStatus::Succeeded,
        started_at: fixed_now(),
        finished_at: Some(fixed_now() + Duration::seconds(2)),
        fetched_count: 7,
        error: None,
    }];
    repo.mark_finished(&run.id, fixed_now() + Duration::seconds(2), &per_source)
        .await
        .unwrap();

    let got = repo.get(&run.id).await.unwrap().unwrap();
    assert_eq!(got.status, SyncRunStatus::Completed);
    assert_eq!(got.finished_at, Some(fixed_now() + Duration::seconds(2)));
    assert_eq!(got.per_source_state, per_source);
    assert!(got.cancel_reason.is_none());
    assert!(got.superseded_by.is_none());
}

#[tokio::test]
async fn sync_runs_running_to_cancelled_with_superseded_by() {
    let (pool, _dir) = test_pool().await;
    let repo = SyncRunRepo::new(pool);
    let old = fixture_running_run();
    let new = fixture_running_run();
    repo.insert(&old).await.unwrap();
    repo.insert(&new).await.unwrap();

    repo.mark_cancelled(
        &old.id,
        fixed_now() + Duration::seconds(1),
        SyncRunCancelReason::SupersededBy { run_id: new.id },
        &[],
    )
    .await
    .unwrap();

    let got = repo.get(&old.id).await.unwrap().unwrap();
    assert_eq!(got.status, SyncRunStatus::Cancelled);
    assert_eq!(
        got.cancel_reason,
        Some(SyncRunCancelReason::SupersededBy { run_id: new.id })
    );
    assert_eq!(got.superseded_by, Some(new.id));
}

#[tokio::test]
async fn sync_runs_reject_terminal_reentry() {
    let (pool, _dir) = test_pool().await;
    let repo = SyncRunRepo::new(pool);
    let run = fixture_running_run();
    repo.insert(&run).await.unwrap();
    repo.mark_finished(&run.id, fixed_now(), &[]).await.unwrap();

    let err = repo
        .mark_finished(&run.id, fixed_now(), &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, DbError::InvalidData { .. }),
        "terminal → terminal must be rejected, got {err:?}"
    );
}

#[tokio::test]
async fn persons_bootstrap_self_is_idempotent() {
    let (pool, _dir) = test_pool().await;
    let repo = PersonRepo::new(pool);
    let first = repo.bootstrap_self("Vedanth").await.unwrap();
    let second = repo.bootstrap_self("Someone Else").await.unwrap();
    assert_eq!(first, second, "second call must return the same row");
    let all = repo.list().await.unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].is_self);
}

#[tokio::test]
async fn persons_enforce_single_self_at_db_layer() {
    let (pool, _dir) = test_pool().await;
    let repo = PersonRepo::new(pool);
    repo.insert(&Person::new_self("A")).await.unwrap();
    let err = repo.insert(&Person::new_self("B")).await.unwrap_err();
    assert!(
        matches!(err, DbError::Conflict { .. }),
        "second self-person must violate unique index, got {err:?}"
    );
}

#[tokio::test]
async fn source_identities_resolve_and_cascade() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let persons = PersonRepo::new(pool.clone());
    let identities = SourceIdentityRepo::new(pool.clone());

    let src = fixture_source();
    sources.insert(&src).await.unwrap();
    let me = persons.bootstrap_self("Vedanth").await.unwrap();

    let email_identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: me.id,
        source_id: None,
        kind: SourceIdentityKind::GitEmail,
        external_actor_id: "vedanth@example.com".into(),
    };
    let user_id_identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: me.id,
        source_id: Some(src.id),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: "42".into(),
    };
    identities.insert(&email_identity).await.unwrap();
    identities.insert(&user_id_identity).await.unwrap();

    let listed = identities.list_for_source(me.id, &src.id).await.unwrap();
    assert_eq!(listed.len(), 2);

    let resolved = identities
        .resolve_person_id(Some(&src.id), SourceIdentityKind::GitLabUserId, "42")
        .await
        .unwrap();
    assert_eq!(resolved, Some(me.id));
    let missing = identities
        .resolve_person_id(Some(&src.id), SourceIdentityKind::GitLabUserId, "999")
        .await
        .unwrap();
    assert!(missing.is_none());
    // Source-agnostic email matches regardless of the source id the
    // caller asks for.
    let agnostic = identities
        .resolve_person_id(
            Some(&src.id),
            SourceIdentityKind::GitEmail,
            "vedanth@example.com",
        )
        .await
        .unwrap();
    assert_eq!(agnostic, Some(me.id));

    sources.delete(&src.id).await.unwrap();
    let after_src_delete = identities.list_for_person(me.id).await.unwrap();
    assert_eq!(
        after_src_delete.len(),
        1,
        "source-scoped identity should cascade, source-agnostic one should stay"
    );
    assert_eq!(after_src_delete[0].kind, SourceIdentityKind::GitEmail);
}
