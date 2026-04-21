//! Smoke tests proving the DAY-79 scaffold + DAY-80 walker-wiring
//! invariants.
//!
//! From the plan (§Task 7 + §Task 8):
//!
//! 1. **Registered kind.** `ConfluenceConnector::kind()` /
//!    `ConfluenceMux::kind()` both return
//!    [`SourceKind::Confluence`] — the orchestrator registry keys off
//!    this, so a wrong return here would silently route Confluence
//!    fan-out to whatever mux accidentally got registered.
//! 2. **Non-Day unsupported.** After DAY-80 wired the CQL walker into
//!    `SyncRequest::Day`, `Range` / `Since` still return
//!    [`DayseamError::Unsupported`] until v0.3's incremental
//!    scheduler lands — matching the Jira connector's exact shape.
//! 3. Object-safety through `Arc<dyn SourceConnector>` — the
//!    registry stores mux handles behind exactly that bound, so a
//!    regression here would fail every `default_registries` call
//!    loudly.
//! 4. `ConfluenceMux::upsert` / `remove` round-trip by `source_id`.
//!
//! The `validate_auth` + `list_identities` happy paths live in
//! `tests/auth.rs` alongside the shared-identity invariant; the
//! walker end-to-end path lives in `tests/walk.rs`.

use std::sync::Arc;

use chrono::NaiveDate;
use connector_confluence::{
    ConfluenceConfig, ConfluenceConnector, ConfluenceMux, ConfluenceSourceCfg,
};
use connectors_sdk::{
    AuthStrategy, BasicAuth, Checkpoint, ConnCtx, HttpClient, NoneAuth, NoopRawStore,
    SourceConnector, SyncRequest, SystemClock,
};
use dayseam_core::{error_codes, DayseamError, Person, SourceKind};
use dayseam_events::RunStreams;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn test_config() -> ConfluenceConfig {
    ConfluenceConfig::from_raw("https://acme.atlassian.net")
        .expect("known-good workspace URL parses")
}

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
fn confluence_connector_kind_is_confluence() {
    let conn = ConfluenceConnector::new(test_config());
    assert_eq!(conn.kind(), SourceKind::Confluence);
}

#[test]
fn confluence_mux_kind_is_confluence() {
    let mux = ConfluenceMux::default();
    assert_eq!(mux.kind(), SourceKind::Confluence);
}

#[test]
fn confluence_mux_can_be_wrapped_as_arc_dyn_source_connector() {
    // The orchestrator registry stores `Arc<dyn SourceConnector>`
    // per kind; this sanity check catches any future regression
    // that would make `ConfluenceMux` un-wrappable (e.g. a stray
    // `?Sized` bound).
    let mux: Arc<dyn SourceConnector> = Arc::new(ConfluenceMux::default());
    assert_eq!(mux.kind(), SourceKind::Confluence);
}

#[tokio::test]
async fn confluence_mux_upsert_and_remove_round_trip() {
    let mux = ConfluenceMux::default();
    assert!(mux.is_empty().await);

    let source_id = Uuid::new_v4();
    mux.upsert(ConfluenceSourceCfg {
        source_id,
        config: test_config(),
    })
    .await;
    assert_eq!(mux.len().await, 1);

    mux.remove(source_id).await;
    assert!(mux.is_empty().await);
}

#[tokio::test]
async fn confluence_mux_sync_on_unregistered_source_returns_source_not_found() {
    let mux = ConfluenceMux::default();
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
async fn non_day_sync_request_unsupported() {
    // Post DAY-80, `Day` is handled by the CQL walker (see
    // `tests/walk.rs`). `Range` / `Since` continue to return
    // `Unsupported` until v0.3's incremental scheduler lands — the
    // invariant this test pins is that someone accidentally wiring
    // either of them without the scheduler fails loudly.
    let conn = ConfluenceConnector::new(test_config());
    let ctx = mk_ctx(Uuid::new_v4());

    let range = conn
        .sync(
            &ctx,
            SyncRequest::Range {
                start: NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(),
                end: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            },
        )
        .await
        .expect_err("Range is unsupported in v0.2 Confluence scaffold");
    assert_eq!(
        range.code(),
        error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST
    );

    let since = conn
        .sync(
            &ctx,
            SyncRequest::Since(Checkpoint {
                connector: "confluence".into(),
                value: serde_json::Value::Null,
            }),
        )
        .await
        .expect_err("Since is unsupported in v0.2 Confluence scaffold");
    assert_eq!(
        since.code(),
        error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST
    );
}

#[tokio::test]
async fn sync_day_routes_into_walker_when_no_identity_configured() {
    // Post DAY-80, `SyncRequest::Day` no longer returns Unsupported.
    // With no `AtlassianAccountId` identity registered on the ctx,
    // the walker bails early with an empty SyncResult — this test
    // verifies the wiring + the early-bail short-circuit so a future
    // refactor that forgets to plumb `source_identities` through
    // `ConnCtx` fails here instead of leaking a real HTTP call.
    let conn = ConfluenceConnector::new(test_config());
    let ctx = mk_ctx(Uuid::new_v4());
    let result = conn
        .sync(
            &ctx,
            SyncRequest::Day(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()),
        )
        .await
        .expect("identity miss should short-circuit, not error");
    assert!(result.events.is_empty(), "empty identity → empty events");
    assert_eq!(result.stats.fetched_count, 0);
}

#[test]
fn basic_auth_builds_for_confluence_flow() {
    // Belt-and-braces: the Add-Source dialog reaches for
    // `BasicAuth::atlassian` when wiring up a `SourceKind::Confluence`
    // row, and a future signature change there would break the IPC
    // command in a way that is easy to miss (the IPC path
    // double-wraps the BasicAuth under `Arc<dyn AuthStrategy>`
    // before this crate ever sees it). Mirrors the identically-named
    // test in `connector-jira` so refactors to `connectors-sdk`
    // fail in this PR's test surface too, not only in the distant
    // IPC crate.
    let _basic = BasicAuth::atlassian(
        "vedanth@acme.com",
        "api-token-xyz",
        "dayseam.atlassian",
        "vedanth@acme.com",
    );
}
