//! Shared test scaffolding for the orchestrator integration tests.
//!
//! Every invariant test starts from the same shape: a fresh SQLite
//! pool with the migrations applied, an `AppBus`, a self-`Person`, a
//! pre-seeded `Source` and `SourceIdentity`, and an
//! [`crate::Orchestrator`] built from a hand-populated
//! [`crate::ConnectorRegistry`] / [`crate::SinkRegistry`].
//!
//! Keeping the scaffolding in one file means a later change to the
//! repo shape or the default registries has a single edit site
//! rather than N copy-pasted fixtures across the `tests/` directory.

#![allow(dead_code)]

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use dayseam_core::{
    ActivityEvent, Person, Source, SourceConfig, SourceHealth, SourceIdentity, SourceIdentityKind,
    SourceKind,
};
use dayseam_events::AppBus;
use dayseam_orchestrator::{
    orchestrator::{OrchestratorBuilder, SourceHandle},
    ConnectorRegistry, Orchestrator, SinkRegistry,
};
use sqlx::SqlitePool;
use uuid::Uuid;

/// One-shot test pool. Returns a pool backed by a throwaway temp
/// file so every test starts with a clean schema, with the shared
/// [`test_person`] already inserted — a foreign-key dependency of
/// every `source_identities` row and every `sync_runs` row, so we
/// do it once at the top rather than sprinkling the seeding across
/// every call site.
pub async fn test_pool() -> (SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("dayseam-test.db");
    let pool = dayseam_db::open(&db_path).await.expect("open pool");
    dayseam_db::PersonRepo::new(pool.clone())
        .insert(&test_person())
        .await
        .expect("seed person");
    (pool, dir)
}

/// Self-`Person` used across all tests. Deterministic id so failure
/// messages stay stable and the `in_flight` key is reproducible.
pub fn test_person() -> Person {
    let id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").expect("fixed uuid");
    Person {
        id,
        display_name: "Test User".into(),
        is_self: true,
    }
}

/// One test source. Seeds the `sources` and `source_identities`
/// tables so the orchestrator can pull the rows back during a run if
/// it wants to; also returns the matching [`SourceHandle`] that the
/// caller passes into `generate_report`.
pub async fn seed_source(
    pool: &SqlitePool,
    person: &Person,
    kind: SourceKind,
    label: &str,
    actor_email: &str,
) -> (Source, SourceIdentity, SourceHandle) {
    let source = Source {
        id: Uuid::new_v4(),
        kind,
        label: label.to_string(),
        config: match kind {
            SourceKind::LocalGit => SourceConfig::LocalGit { scan_roots: vec![] },
            SourceKind::GitLab => SourceConfig::GitLab {
                base_url: "https://mock.example".to_string(),
                user_id: 1,
                username: "mock".to_string(),
            },
            SourceKind::Jira | SourceKind::Confluence => {
                unreachable!(
                    "Atlassian seed_source helper lands with SourceConfig variants in DAY-74"
                )
            }
        },
        secret_ref: None,
        created_at: Utc::now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    dayseam_db::SourceRepo::new(pool.clone())
        .insert(&source)
        .await
        .expect("seed source");
    let identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: person.id,
        source_id: Some(source.id),
        kind: SourceIdentityKind::GitEmail,
        external_actor_id: actor_email.to_string(),
    };
    dayseam_db::SourceIdentityRepo::new(pool.clone())
        .insert(&identity)
        .await
        .expect("seed identity");
    let handle = SourceHandle {
        source_id: source.id,
        kind,
        auth: Arc::new(connectors_sdk::NoneAuth),
        source_identities: vec![identity.clone()],
    };
    (source, identity, handle)
}

/// Build an orchestrator from hand-built registries. Tests never
/// go through `default_registries` — they substitute
/// [`connectors_sdk::MockConnector`] / [`sinks_sdk::MockSink`].
pub fn build_orchestrator(
    pool: SqlitePool,
    connectors: ConnectorRegistry,
    sinks: SinkRegistry,
) -> Orchestrator {
    OrchestratorBuilder::new(pool, AppBus::new(), connectors, sinks)
        .build()
        .expect("orchestrator build")
}

/// A fixture timestamp on `d` at 09:00 UTC. Deterministic so golden
/// comparisons (if any future test wants them) stay stable.
pub fn fixture_ts(d: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&d.and_hms_opt(9, 0, 0).expect("valid hms"))
}

/// Helper to build a fixture event on `d` for `source_id` / `actor`.
pub fn fixture_event(
    source_id: Uuid,
    external_id: &str,
    actor_email: &str,
    d: NaiveDate,
) -> ActivityEvent {
    connectors_sdk::MockConnector::fixture_event(source_id, external_id, actor_email, fixture_ts(d))
}

/// The `NaiveDate` every test pins itself to. Picked to match the
/// `docs/plan/2026-04-18-*.md` path so a failure message has a
/// reproducible reference.
pub fn fixture_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 4, 18).expect("valid date")
}

/// Drain every `ProgressEvent` currently queued up on `rx`. Used by
/// tests that want to assert on the full progress transcript after
/// the run terminates. The receiver close is observed by the caller
/// awaiting the `completion` future.
pub async fn drain_progress(
    mut rx: dayseam_events::ProgressReceiver,
) -> Vec<dayseam_core::ProgressEvent> {
    let mut out = Vec::new();
    while let Some(evt) = rx.recv().await {
        out.push(evt);
    }
    out
}
