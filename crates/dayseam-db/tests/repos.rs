//! Integration tests for every `dayseam-db` repository. These are
//! deliberately written against a real SQLite file in a `tempdir` rather
//! than an in-memory database — we want the pragmas, migrations, and FK
//! cascades to execute exactly as they will in production.

use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityRef, Evidence, Identity, Link, LocalRepo, LogLevel,
    Privacy, RawRef, RenderedBullet, RenderedSection, ReportDraft, RunStatus, SecretRef, Source,
    SourceConfig, SourceHealth, SourceKind, SourceRunState,
};
use dayseam_db::{
    open, ActivityRepo, DbError, DraftRepo, IdentityRepo, LocalRepoRepo, LogRepo, LogRow,
    RawPayload, RawPayloadRepo, SettingsRepo, SourceRepo,
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
    assert_eq!(tail[0], system_row);
    assert_eq!(tail[1], scoped_row);

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
