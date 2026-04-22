//! Smoke tests for the DAY-95 GitHub connector scaffold.
//!
//! DAY-96 replaces the `SyncRequest::Day` `Unsupported` arm with the
//! real events-endpoint + search walker; until then the scaffold
//! invariants below are the only thing standing between a typo in the
//! wiring and a silent `no connector registered` at report time.
//!
//! Mirrors `connector-jira::tests::scaffold` and
//! `connector-gitlab::tests::scaffold` one-for-one so a refactor of
//! the shared `SourceConnector` trait fails in every connector's test
//! surface simultaneously. Invariants:
//!
//! 1. `GithubConnector::kind() == SourceKind::GitHub`.
//! 2. `GithubMux` is object-safe through `Arc<dyn SourceConnector>` —
//!    the orchestrator registry stores it behind exactly that bound.
//! 3. `GithubMux::upsert` / `remove` round-trip by `source_id`.
//! 4. Every `SyncRequest` variant returns
//!    [`DayseamError::Unsupported`] in this scaffold release. DAY-96
//!    flips the `Day` arm; `Range` + `Since` wait on v0.5's
//!    incremental scheduler.
//! 5. `sync` on an unregistered `source_id` surfaces as
//!    [`dayseam_core::error_codes::IPC_SOURCE_NOT_FOUND`] (not a
//!    silent empty-events result).
//! 6. `PatAuth::github` is reachable from this crate — belt-and-braces
//!    against a future rename in `connectors-sdk::auth` that would
//!    break the IPC `build_source_auth` arm in a way not caught by
//!    this crate's own unit tests.

use std::sync::Arc;

use chrono::NaiveDate;
use connector_github::{GithubConfig, GithubConnector, GithubMux, GithubSourceCfg};
use connectors_sdk::{
    AuthStrategy, Checkpoint, ConnCtx, HttpClient, NoneAuth, NoopRawStore, PatAuth,
    SourceConnector, SyncRequest, SystemClock,
};
use dayseam_core::{error_codes, DayseamError, Person, SourceKind};
use dayseam_events::RunStreams;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn mk_ctx(source_id: Uuid) -> ConnCtx {
    let streams = RunStreams::new(dayseam_events::RunId::new());
    let ((ptx, ltx), (_, _)) = streams.split();
    let run_id = ptx.run_id();
    ConnCtx {
        run_id,
        source_id,
        person: Person::new_self("Test"),
        source_identities: Vec::new(),
        auth: Arc::new(NoneAuth) as Arc<dyn AuthStrategy>,
        progress: ptx,
        logs: ltx,
        raw_store: Arc::new(NoopRawStore),
        clock: Arc::new(SystemClock),
        http: HttpClient::new().expect("http client builds"),
        cancel: CancellationToken::new(),
    }
}

#[test]
fn github_connector_kind_is_github() {
    let conn = GithubConnector::new(GithubConfig::github_com());
    assert_eq!(conn.kind(), SourceKind::GitHub);
}

#[test]
fn github_mux_kind_is_github() {
    let mux = GithubMux::default();
    assert_eq!(mux.kind(), SourceKind::GitHub);
}

#[test]
fn github_mux_can_be_wrapped_as_arc_dyn_source_connector() {
    let mux: Arc<dyn SourceConnector> = Arc::new(GithubMux::default());
    assert_eq!(mux.kind(), SourceKind::GitHub);
}

#[tokio::test]
async fn github_mux_upsert_and_remove_round_trip() {
    let mux = GithubMux::default();
    assert!(mux.is_empty().await);

    let source_id = Uuid::new_v4();
    mux.upsert(GithubSourceCfg {
        source_id,
        config: GithubConfig::github_com(),
    })
    .await;
    assert_eq!(mux.len().await, 1);

    mux.remove(source_id).await;
    assert!(mux.is_empty().await);
}

#[tokio::test]
async fn github_mux_sync_on_unregistered_source_returns_source_not_found() {
    let mux = GithubMux::default();
    let ctx = mk_ctx(Uuid::new_v4());
    let err = mux
        .sync(
            &ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()),
        )
        .await
        .expect_err("an unregistered source_id has to surface as InvalidConfig");
    assert_eq!(err.code(), error_codes::IPC_SOURCE_NOT_FOUND);
    assert!(matches!(err, DayseamError::InvalidConfig { .. }));
}

#[tokio::test]
async fn sync_day_with_no_identity_returns_empty_outcome() {
    // DAY-96 wired `SyncRequest::Day` onto the events + search walker.
    // Without a registered `GitHubUserId` identity in the context,
    // `walk_day` early-bails with an empty outcome rather than
    // issuing a request that would get filtered to zero anyway.
    // (The walker logs a `Warn` explaining why — see `self_identity`.)
    let conn = GithubConnector::new(GithubConfig::github_com());
    let ctx = mk_ctx(Uuid::new_v4());
    let result = conn
        .sync(
            &ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()),
        )
        .await
        .expect("Day should succeed with empty outcome when no identity configured");
    assert!(result.events.is_empty());
    assert_eq!(result.stats.fetched_count, 0);
}

#[tokio::test]
async fn sync_range_returns_unsupported() {
    let conn = GithubConnector::new(GithubConfig::github_com());
    let ctx = mk_ctx(Uuid::new_v4());
    let err = conn
        .sync(
            &ctx,
            SyncRequest::Range {
                start: NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(),
                end: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            },
        )
        .await
        .expect_err("Range is unsupported in v0.4 GitHub");
    assert_eq!(err.code(), error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST);
}

#[tokio::test]
async fn sync_since_returns_unsupported() {
    let conn = GithubConnector::new(GithubConfig::github_com());
    let ctx = mk_ctx(Uuid::new_v4());
    let err = conn
        .sync(
            &ctx,
            SyncRequest::Since(Checkpoint {
                connector: "github".into(),
                value: serde_json::Value::Null,
            }),
        )
        .await
        .expect_err("Since is unsupported in v0.4 GitHub");
    assert_eq!(err.code(), error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST);
}

#[test]
fn pat_auth_github_builds_for_ipc_flow() {
    // Belt-and-braces against a rename / signature change in
    // `connectors_sdk::auth::PatAuth::github`. The IPC
    // `build_source_auth` arm reaches for this constructor when
    // wiring up a `SourceKind::GitHub` row, and a silent refactor
    // there would only be caught once a real GitHub source landed
    // in the DB — the exact silent-failure mode DOG-v0.2-01 was
    // about.
    let _pat = PatAuth::github("ghp-test-token", "dayseam.github", "vedanth");
}
