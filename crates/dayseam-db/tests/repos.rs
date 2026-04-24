//! Integration tests for every `dayseam-db` repository. These are
//! deliberately written against a real SQLite file in a `tempdir` rather
//! than an in-memory database — we want the pragmas, migrations, and FK
//! cascades to execute exactly as they will in production.

use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, Artifact, ArtifactId, ArtifactKind, ArtifactPayload,
    EntityKind, EntityRef, Evidence, Identity, Link, LocalRepo, LogLevel, PerSourceState, Person,
    Privacy, RawRef, RenderedBullet, RenderedSection, ReportDraft, RunId, RunStatus, SecretRef,
    Source, SourceConfig, SourceHealth, SourceIdentity, SourceIdentityKind, SourceKind,
    SourceRunState, SyncRun, SyncRunCancelReason, SyncRunStatus, SyncRunTrigger,
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
            kind: EntityKind::Other("mr".into()),
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

    // busy_timeout is set via `SqliteConnectOptions::busy_timeout`; it
    // surfaces as the configured ms value on each new connection.
    // Phase 2 Task 8 pinned it at 5s so retention sweeps do not
    // surface SQLITE_BUSY to the UI when they overlap a generate
    // fan-out.
    let busy_ms: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(busy_ms, 5000);

    // cache_size is set to -8000 → ~8 MiB per connection. SQLite
    // reports it back as the literal int we passed.
    let cache_size: i64 = sqlx::query_scalar("PRAGMA cache_size")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cache_size, -8000);

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

    let orphaned = repo.delete(&src.id).await.unwrap();
    assert_eq!(
        orphaned, src.secret_ref,
        "sole owner of a secret_ref → caller receives it back to drop from the keyring"
    );
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
async fn deleting_source_preserves_shared_secret_until_last_reference() {
    // DAY-81: when two `sources` rows share a `secret_ref` (the
    // Atlassian shared-PAT flow: one Jira row + one Confluence row
    // pointing at the same keychain slot), removing the first must
    // return `None` so the IPC layer does *not* drop the keyring
    // entry — the surviving source would otherwise silently fail to
    // authenticate on its next run. Removing the second (now the
    // sole holder) must return the `SecretRef` so the keyring row
    // can finally be cleaned up.
    let (pool, _dir) = test_pool().await;
    let repo = SourceRepo::new(pool.clone());

    let shared = SecretRef {
        keychain_service: "app.dayseam.desktop".into(),
        keychain_account: "atlassian:acme".into(),
    };

    let mut jira = fixture_source();
    jira.id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    jira.kind = SourceKind::Jira;
    jira.label = "jira:acme".into();
    jira.config = SourceConfig::Jira {
        workspace_url: "https://acme.atlassian.net".into(),
        email: "me@acme.com".into(),
    };
    jira.secret_ref = Some(shared.clone());

    let mut confluence = fixture_source();
    confluence.id = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
    confluence.kind = SourceKind::Confluence;
    confluence.label = "confluence:acme".into();
    confluence.config = SourceConfig::Confluence {
        workspace_url: "https://acme.atlassian.net".into(),
        email: "me@acme.com".into(),
    };
    confluence.secret_ref = Some(shared.clone());

    repo.insert(&jira).await.unwrap();
    repo.insert(&confluence).await.unwrap();

    // First delete: the Confluence row goes; Jira is still pointing
    // at the shared secret, so the caller must receive `None`.
    let orphaned = repo.delete(&confluence.id).await.unwrap();
    assert!(
        orphaned.is_none(),
        "shared secret must survive the first delete (the other source still holds it)"
    );
    // Baseline sanity: Jira row is still there and still knows the
    // shared secret.
    let surviving = repo.get(&jira.id).await.unwrap().expect("jira present");
    assert_eq!(surviving.secret_ref.as_ref(), Some(&shared));

    // Second delete: Jira row is the last holder — `delete` returns
    // the `SecretRef` so the IPC layer can finally drop it.
    let orphaned = repo.delete(&jira.id).await.unwrap();
    assert_eq!(
        orphaned,
        Some(shared),
        "last reference gone → caller receives the secret_ref to drop from the keyring"
    );
}

#[tokio::test]
async fn deleting_source_with_no_secret_ref_returns_none() {
    // Local-git-style row that never had a keychain slot — the
    // return type is `Option<SecretRef>` but there's nothing to
    // hand back; the IPC layer must not call the keyring on the
    // way out.
    let (pool, _dir) = test_pool().await;
    let repo = SourceRepo::new(pool.clone());
    let src = fixture_local_source();
    repo.insert(&src).await.unwrap();
    let orphaned = repo.delete(&src.id).await.unwrap();
    assert!(
        orphaned.is_none(),
        "row with secret_ref=NULL → Option<SecretRef>::None"
    );
}

#[tokio::test]
async fn deleting_nonexistent_source_is_a_no_op() {
    // Safety net — a stray `sources_delete` IPC call for an id that
    // the UI raced past a concurrent remove must not synthesize a
    // ghost `SecretRef` out of thin air and trick the keyring into
    // a rogue delete.
    let (pool, _dir) = test_pool().await;
    let repo = SourceRepo::new(pool.clone());
    let ghost = Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap();
    let orphaned = repo.delete(&ghost).await.unwrap();
    assert!(orphaned.is_none());
}

#[tokio::test]
async fn distinct_secret_refs_lists_each_shared_slot_once() {
    // DAY-81 orphan-detector helper: two rows sharing a secret +
    // one independent row should yield two distinct refs, never
    // three — the keychain only holds one slot per unique ref and
    // the detector compares cardinalities.
    let (pool, _dir) = test_pool().await;
    let repo = SourceRepo::new(pool.clone());

    let shared = SecretRef {
        keychain_service: "app.dayseam.desktop".into(),
        keychain_account: "atlassian:acme".into(),
    };
    let solo = SecretRef {
        keychain_service: "app.dayseam.desktop".into(),
        keychain_account: "gitlab:solo".into(),
    };

    let mut jira = fixture_source();
    jira.id = Uuid::parse_str("dddddddd-dddd-dddd-dddd-dddddddddddd").unwrap();
    jira.kind = SourceKind::Jira;
    jira.config = SourceConfig::Jira {
        workspace_url: "https://acme.atlassian.net".into(),
        email: "me@acme.com".into(),
    };
    jira.secret_ref = Some(shared.clone());

    let mut confluence = fixture_source();
    confluence.id = Uuid::parse_str("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee").unwrap();
    confluence.kind = SourceKind::Confluence;
    confluence.config = SourceConfig::Confluence {
        workspace_url: "https://acme.atlassian.net".into(),
        email: "me@acme.com".into(),
    };
    confluence.secret_ref = Some(shared.clone());

    let mut gitlab = fixture_source();
    gitlab.id = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();
    gitlab.secret_ref = Some(solo.clone());

    let local = fixture_local_source();

    repo.insert(&jira).await.unwrap();
    repo.insert(&confluence).await.unwrap();
    repo.insert(&gitlab).await.unwrap();
    repo.insert(&local).await.unwrap();

    let mut refs = repo.distinct_secret_refs().await.unwrap();
    refs.sort_by(|a, b| a.keychain_account.cmp(&b.keychain_account));
    assert_eq!(
        refs,
        vec![shared, solo],
        "four sources but only two distinct keychain slots"
    );
}

#[tokio::test]
async fn source_update_secret_ref_round_trips() {
    // DAY-70 regression: `sources_add` used to hardcode `secret_ref: None`
    // and there was no way to associate a GitLab source with its PAT
    // slot in Keychain after the row was written. The repo needs to be
    // able to install (and clear) the secret_ref so the IPC layer can
    // finish the GitLab add flow atomically.
    let (pool, _dir) = test_pool().await;
    let repo = SourceRepo::new(pool.clone());

    // Start from a GitLab source whose PAT slot was not set at insert.
    let mut src = fixture_source();
    src.secret_ref = None;
    repo.insert(&src).await.unwrap();
    let got = repo.get(&src.id).await.unwrap().unwrap();
    assert!(got.secret_ref.is_none(), "baseline: no secret_ref yet");

    // Install the keychain pointer the IPC layer will compute after
    // `gitlab_validate_pat` succeeds.
    let sr = SecretRef {
        keychain_service: "app.dayseam.desktop".into(),
        keychain_account: format!("gitlab:{}", src.id),
    };
    repo.update_secret_ref(&src.id, Some(&sr)).await.unwrap();
    let got = repo.get(&src.id).await.unwrap().unwrap();
    assert_eq!(got.secret_ref.as_ref(), Some(&sr));

    // And make sure we can clear it (used on `sources_delete` + as a
    // safety net if the keychain write fails mid-add).
    repo.update_secret_ref(&src.id, None).await.unwrap();
    let got = repo.get(&src.id).await.unwrap().unwrap();
    assert!(got.secret_ref.is_none(), "cleared secret_ref");
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

/// DAY-72 CORR-addendum-02 regression: a user marks a repo private
/// via `set_is_private(true)`. The next discovery-driven `upsert`
/// carries `is_private: false` (because `upsert_discovered_repos`
/// always constructs discovery rows that way — discovery has no
/// ground-truth for privacy). The UPSERT must **not** clobber the
/// user's flag.
#[tokio::test]
async fn local_repos_upsert_preserves_user_set_is_private_on_rescan() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let local = LocalRepoRepo::new(pool.clone());
    let src = fixture_local_source();
    sources.insert(&src).await.unwrap();

    let path = PathBuf::from("/Users/v/Code/secret-repo");
    let discovered = LocalRepo {
        path: path.clone(),
        label: "secret-repo".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    local.upsert(&src.id, &discovered).await.unwrap();

    // User explicitly marks the repo private.
    local.set_is_private(&path, true).await.unwrap();
    assert!(local.get(&path).await.unwrap().unwrap().is_private);

    // Rescan: discovery re-upserts with is_private=false (the shape
    // the production `upsert_discovered_repos` always produces).
    local.upsert(&src.id, &discovered).await.unwrap();

    let after = local.get(&path).await.unwrap().unwrap();
    assert!(
        after.is_private,
        "rescan must preserve user-set is_private=true; got is_private={}",
        after.is_private
    );
}

/// DOGFOOD-v0.4-03 regression: `reconcile_for_source` must upsert
/// every `keep` row, delete rows that were previously tracked for
/// this source but are absent from `keep`, and preserve a
/// user-toggled `is_private` flag on rows that are still in `keep`.
#[tokio::test]
async fn local_repos_reconcile_prunes_stale_rows_and_keeps_private_flag() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let local = LocalRepoRepo::new(pool.clone());
    let src = fixture_local_source();
    sources.insert(&src).await.unwrap();

    let kept = LocalRepo {
        path: PathBuf::from("/Users/v/Code/kept"),
        label: "kept".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    let stale = LocalRepo {
        path: PathBuf::from("/Users/v/Code/stale"),
        label: "stale".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    // Seed the table as it would look after the previous walk.
    local.upsert(&src.id, &kept).await.unwrap();
    local.upsert(&src.id, &stale).await.unwrap();
    // User marks `kept` private before the rescan.
    local.set_is_private(&kept.path, true).await.unwrap();

    // Rescan sees `kept` only; `stale` is no longer discovered.
    let removed = local
        .reconcile_for_source(&src.id, std::slice::from_ref(&kept))
        .await
        .unwrap();

    assert_eq!(removed, 1, "exactly one stale row should have been pruned");
    let remaining = local.list_for_source(&src.id).await.unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "table must match fresh walk size exactly (no high-water mark)"
    );
    assert_eq!(remaining[0].path, kept.path);
    assert!(
        remaining[0].is_private,
        "reconcile must preserve user-set is_private"
    );
    assert!(
        local.get(&stale.path).await.unwrap().is_none(),
        "stale row must be deleted, not just hidden"
    );
}

/// F-7 (DAY-105). A steady-state rescan — where the walker returns
/// exactly the same approved repos already tracked — must return `0`
/// and must not emit a `DELETE` statement. The batched
/// `reconcile_for_source` path short-circuits on `current ⊆
/// keep_paths`, and this test pins that behaviour at the observable
/// layer: no row mutates, no paths disappear, `Ok(0)` flows back.
#[tokio::test]
async fn local_repos_reconcile_no_stale_is_a_no_op_and_returns_zero() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let local = LocalRepoRepo::new(pool.clone());
    let src = fixture_local_source();
    sources.insert(&src).await.unwrap();

    let a = LocalRepo {
        path: PathBuf::from("/Users/v/Code/a"),
        label: "a".into(),
        is_private: true,
        discovered_at: fixed_now(),
    };
    let b = LocalRepo {
        path: PathBuf::from("/Users/v/Code/b"),
        label: "b".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    local.upsert(&src.id, &a).await.unwrap();
    local.upsert(&src.id, &b).await.unwrap();
    local.set_is_private(&a.path, true).await.unwrap();

    let kept = [a.clone(), b.clone()];
    let removed = local.reconcile_for_source(&src.id, &kept).await.unwrap();
    assert_eq!(
        removed, 0,
        "identical rescan must report zero stale rows (not just preserve them)"
    );

    let remaining = local.list_for_source(&src.id).await.unwrap();
    assert_eq!(remaining.len(), 2);
    // is_private on `a` still survives the no-op path — this is the
    // DOGFOOD-v0.4-03 invariant, re-asserted here because the
    // batched path is new code and the no-stale short-circuit skips
    // the DELETE but still runs the upserts.
    let a_after = remaining.iter().find(|r| r.path == a.path).unwrap();
    assert!(a_after.is_private);
}

/// F-7 (DAY-105). Pin the batched `DELETE … NOT IN (…)` on a wider
/// stale set than the existing one-row test — multiple stale paths
/// exercise the placeholder-building branch specifically, since the
/// fast path builds a single SQL statement with `N` placeholders
/// and binds `N + 1` parameters (one `source_id` plus each `keep`
/// path). A per-row-loop regression would still pass the
/// single-stale test; this one doesn't.
#[tokio::test]
async fn local_repos_reconcile_prunes_many_stale_rows_in_one_batch() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let local = LocalRepoRepo::new(pool.clone());
    let src = fixture_local_source();
    sources.insert(&src).await.unwrap();

    // Seed six repos (one kept, five stale).
    let kept = LocalRepo {
        path: PathBuf::from("/Users/v/Code/kept"),
        label: "kept".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    local.upsert(&src.id, &kept).await.unwrap();
    for i in 0..5 {
        let stale = LocalRepo {
            path: PathBuf::from(format!("/Users/v/Code/stale-{i}")),
            label: format!("stale-{i}"),
            is_private: false,
            discovered_at: fixed_now(),
        };
        local.upsert(&src.id, &stale).await.unwrap();
    }

    let removed = local
        .reconcile_for_source(&src.id, std::slice::from_ref(&kept))
        .await
        .unwrap();
    assert_eq!(
        removed, 5,
        "batched DELETE must report the full stale count"
    );
    let remaining = local.list_for_source(&src.id).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].path, kept.path);
}

/// Reconciling with an empty `keep` set must clear every row for the
/// source (but only for that source — other sources' rows are
/// untouched).
#[tokio::test]
async fn local_repos_reconcile_is_scoped_to_its_source() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let local = LocalRepoRepo::new(pool.clone());

    // Two distinct LocalGit sources. `fixture_local_source` gives us
    // one; build a second with a different id.
    let src_a = fixture_local_source();
    sources.insert(&src_a).await.unwrap();
    let src_b = {
        let mut s = fixture_local_source();
        s.id = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        s.label = "Second local source".into();
        s
    };
    sources.insert(&src_b).await.unwrap();

    let a_repo = LocalRepo {
        path: PathBuf::from("/Users/v/Code/source-a-repo"),
        label: "a".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    let b_repo = LocalRepo {
        path: PathBuf::from("/Users/v/Code/source-b-repo"),
        label: "b".into(),
        is_private: false,
        discovered_at: fixed_now(),
    };
    local.upsert(&src_a.id, &a_repo).await.unwrap();
    local.upsert(&src_b.id, &b_repo).await.unwrap();

    // Reconcile source A with an empty keep set.
    let removed = local.reconcile_for_source(&src_a.id, &[]).await.unwrap();
    assert_eq!(removed, 1);

    assert!(local.list_for_source(&src_a.id).await.unwrap().is_empty());
    // Source B is untouched.
    let b_after = local.list_for_source(&src_b.id).await.unwrap();
    assert_eq!(b_after.len(), 1);
    assert_eq!(b_after[0].path, b_repo.path);
}

#[tokio::test]
async fn activity_events_round_trip_and_reinsert_is_idempotent() {
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

    // DAY-52 + DAY-71: `insert_many` is now an upsert on
    // `(source_id, external_id, kind)`. Re-inserting the same
    // deterministic-id event on a second generate run must succeed
    // silently (not error) so regenerations don't tear the sync down,
    // *and* must refresh the row's payload so a connector-side bug fix
    // (e.g. adding a previously-missing `repo` entity) lands on the
    // next generate.
    events
        .insert_many(&[event.clone()])
        .await
        .expect("re-insert of a deterministic event id must be idempotent");

    let still_one = events.list_by_source_date(&src.id, date).await.unwrap();
    assert_eq!(
        still_one,
        vec![event.clone()],
        "idempotent re-insert must not duplicate rows",
    );
}

/// DAY-71 regression: `insert_many` must refresh non-key columns on
/// re-sync of the same `(source_id, external_id, kind)`.
///
/// Before this change `INSERT OR IGNORE` left the first-written row
/// untouched, so the GitLab connector's fix to emit a `repo` entity
/// only took effect for *new* events — today's 16 rows that were
/// already persisted without it kept rendering as `**/** — …` in the
/// report. The upsert lands the new shape on the next generate.
#[tokio::test]
async fn activity_events_upsert_refreshes_payload_on_conflict() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let events = ActivityRepo::new(pool.clone());

    let src = fixture_source();
    sources.insert(&src).await.unwrap();

    let mut first = fixture_event_for(&src);
    first.title = "stale title".to_string();
    first.entities = Vec::new();
    events
        .insert_many(&[first.clone()])
        .await
        .expect("first insert");

    // Second sync produces the same `(source_id, external_id, kind)`
    // and therefore the same deterministic id, but with an enriched
    // payload — the shape DAY-71's normaliser now emits.
    let mut refreshed = first.clone();
    refreshed.title = "refreshed title".to_string();
    refreshed.entities = vec![EntityRef {
        kind: EntityKind::Repo,
        external_id: "company/modulo-local-infra".into(),
        label: Some("modulo-local-infra".into()),
    }];
    events
        .insert_many(&[refreshed.clone()])
        .await
        .expect("upsert of same key must succeed");

    let rows = events
        .list_by_source_date(&src.id, first.occurred_at.date_naive())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "upsert must not duplicate rows");
    let row = &rows[0];
    assert_eq!(
        row.id, first.id,
        "stable deterministic id must survive the refresh so evidence edges still resolve"
    );
    assert_eq!(row.title, "refreshed title", "title must be refreshed");
    let repo_entity = row
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::Repo)
        .expect("repo entity must land on the row after upsert");
    assert_eq!(repo_entity.external_id, "company/modulo-local-infra");
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
                source_kind: Some(dayseam_core::SourceKind::GitLab),
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
async fn sync_runs_list_running_returns_only_rows_without_finished_at() {
    let (pool, _dir) = test_pool().await;
    let repo = SyncRunRepo::new(pool);

    // Two still-running rows and one row already finished — only the
    // two running ones should come back. This is the exact predicate
    // the orchestrator's crash-recovery sweep leans on.
    let a = fixture_running_run();
    let b = fixture_running_run();
    let done = fixture_running_run();
    repo.insert(&a).await.unwrap();
    repo.insert(&b).await.unwrap();
    repo.insert(&done).await.unwrap();
    repo.mark_finished(&done.id, fixed_now(), &[])
        .await
        .unwrap();

    let running = repo.list_running().await.unwrap();
    let ids: Vec<_> = running.iter().map(|r| r.id).collect();
    assert_eq!(running.len(), 2, "got {ids:?}");
    assert!(ids.contains(&a.id));
    assert!(ids.contains(&b.id));
    assert!(!ids.contains(&done.id));
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
async fn persons_update_display_name_rewrites_row_in_place() {
    let (pool, _dir) = test_pool().await;
    let repo = PersonRepo::new(pool);
    let initial = repo.bootstrap_self("Me").await.unwrap();

    let updated = repo
        .update_display_name(initial.id, "Vedanth")
        .await
        .unwrap();

    assert_eq!(updated.id, initial.id, "id must not change");
    assert_eq!(updated.display_name, "Vedanth");
    assert!(updated.is_self, "is_self must not be toggled off");

    let after = repo.get_self().await.unwrap().unwrap();
    assert_eq!(after.display_name, "Vedanth");

    let all = repo.list().await.unwrap();
    assert_eq!(all.len(), 1, "no second row may have been inserted");
}

#[tokio::test]
async fn persons_update_display_name_returns_not_found_for_unknown_id() {
    let (pool, _dir) = test_pool().await;
    let repo = PersonRepo::new(pool);
    let err = repo
        .update_display_name(Uuid::new_v4(), "Ghost")
        .await
        .unwrap_err();
    assert!(
        matches!(err, DbError::NotFound { .. }),
        "update against missing id must be NotFound, got {err:?}"
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

/// `ensure` is the idempotent-upsert path added for DAY-71. Both
/// `sources_add` / `sources_update` and the startup backfill call it
/// on every invocation to guarantee a `GitLabUserId`
/// [`SourceIdentity`] exists for every GitLab source, and the plain
/// `insert` path would trip the UNIQUE constraint on the second call.
#[tokio::test]
async fn source_identities_ensure_is_idempotent_on_natural_key() {
    let (pool, _dir) = test_pool().await;
    let sources = SourceRepo::new(pool.clone());
    let persons = PersonRepo::new(pool.clone());
    let identities = SourceIdentityRepo::new(pool.clone());

    let src = fixture_source();
    sources.insert(&src).await.unwrap();
    let me = persons.bootstrap_self("Vedanth").await.unwrap();

    let first = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: me.id,
        source_id: Some(src.id),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: "291".into(),
    };
    let wrote = identities.ensure(&first).await.unwrap();
    assert!(wrote, "first ensure must write the row");

    // Same natural key, different surrogate id — the UNIQUE index on
    // `(person_id, source_id, kind, external_actor_id)` should coalesce
    // the second call into a no-op, and `ensure` should report that
    // via `false` so the startup backfill can tell "seeded on this
    // boot" from "already had it".
    let twin = SourceIdentity {
        id: Uuid::new_v4(),
        ..first.clone()
    };
    let wrote_again = identities.ensure(&twin).await.unwrap();
    assert!(
        !wrote_again,
        "repeat ensure must be a no-op, got rows_affected > 0"
    );

    let listed = identities.list_for_source(me.id, &src.id).await.unwrap();
    assert_eq!(
        listed.len(),
        1,
        "exactly one row must exist for the natural key"
    );
    assert_eq!(
        listed[0].id, first.id,
        "the surviving row must be the one we inserted first, not the twin"
    );

    // A different `external_actor_id` under the same kind / source
    // is a *different* identity and must land as its own row.
    let other = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: me.id,
        source_id: Some(src.id),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: "999".into(),
    };
    let wrote_other = identities.ensure(&other).await.unwrap();
    assert!(wrote_other, "distinct natural key must insert a fresh row");
    let listed = identities.list_for_source(me.id, &src.id).await.unwrap();
    assert_eq!(listed.len(), 2);
}

/// DAY-89 PERF-v0.2-01. Migration `0005_secret_ref_index.sql` is
/// forward-compatibility insurance for the repair-pipeline lookups
/// DAY-90 introduces and the multi-account-per-product expansion
/// queued for v0.4. The immediate contract it must satisfy is that
/// opening a pool — fresh or upgrading — always leaves the partial
/// index present with the exact predicate the planner can actually
/// use (`WHERE secret_ref IS NOT NULL`). This test is the canonical
/// regression for that contract: if a future migration drops the
/// index or rewrites the predicate in a way the planner won't hit,
/// this check fails loudly at `cargo test` time rather than
/// silently at production query time.
#[tokio::test]
async fn opening_pool_creates_secret_ref_partial_index() {
    let (pool, _dir) = test_pool().await;

    // `sqlite_master` is SQLite's catalog table. For `CREATE INDEX`
    // it stores the exact CREATE statement we ran, so we can both
    // confirm the index exists *and* verify the partial predicate
    // survived the migration runner.
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master
         WHERE type = 'index' AND name = 'idx_sources_secret_ref'",
    )
    .fetch_optional(&pool)
    .await
    .expect("query sqlite_master");

    let (name, sql) = row.expect(
        "idx_sources_secret_ref must exist after migrations run — \
         did 0005_secret_ref_index.sql get removed or renamed?",
    );
    assert_eq!(name, "idx_sources_secret_ref");

    // The partial predicate is load-bearing: without it the index
    // carries a NULL-keyed entry for every local-git source (which
    // has `secret_ref = NULL`) and wastes space without helping any
    // real query. Assert the predicate text on the stored SQL so a
    // well-meaning future edit that turns the partial index into a
    // full index fails here first.
    let sql = sql.expect("CREATE INDEX statement must be stored in sqlite_master");
    assert!(
        sql.contains("WHERE secret_ref IS NOT NULL"),
        "idx_sources_secret_ref must be a PARTIAL index with \
         `WHERE secret_ref IS NOT NULL` (got: {sql})"
    );

    // Re-opening the pool against the same file must be idempotent.
    // `CREATE INDEX IF NOT EXISTS` is the guard; if a future edit
    // drops the guard the second migration pass will fail here.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let first = open(&path).await.expect("first open");
    drop(first);
    let _second = open(&path).await.expect(
        "second open on the same file must be idempotent — did the \
         migration lose its `IF NOT EXISTS` guard?",
    );
}
