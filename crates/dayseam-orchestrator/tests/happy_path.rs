//! Invariant #1 (partial): a single source, happy path.
//!
//! The orchestrator must:
//! * transition the `sync_runs` row `Running → Completed`,
//! * persist exactly one `ReportDraft`,
//! * emit `Starting → InProgress → Completed` on the per-run progress
//!   stream,
//! * clear its in-flight entry,
//! * return [`dayseam_orchestrator::GenerateOutcome`]`::Completed`
//!   with the same `draft_id` the repo stores.

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;

use common::{
    build_orchestrator, fixture_date, fixture_event, seed_source, test_person, test_pool,
};
use connectors_sdk::MockConnector;
use dayseam_core::{ProgressPhase, SourceKind, SyncRunStatus};
use dayseam_orchestrator::{orchestrator::GenerateRequest, ConnectorRegistry, SinkRegistry};
use dayseam_report::DEV_EOD_TEMPLATE_ID;

#[tokio::test]
async fn single_source_completes_and_persists_draft() {
    let (pool, _tmp) = test_pool().await;
    let person = test_person();
    let date = fixture_date();

    // Seed a `LocalGit` source with one identity the mock will match on.
    let (source, _id, handle) = seed_source(
        &pool,
        &person,
        SourceKind::LocalGit,
        "local-git fixture",
        "dev@example.com",
    )
    .await;

    // Build a `MockConnector` registered under `LocalGit` with one
    // fixture event whose actor matches the identity above. The
    // orchestrator builds the `ConnCtx` for us; the mock honours the
    // authorship filter on the incoming `source_identities`.
    let event = fixture_event(source.id, "evt-1", "dev@example.com", date);
    let connector = Arc::new(MockConnector::new(SourceKind::LocalGit, vec![event]));

    let mut connectors = ConnectorRegistry::default();
    connectors.insert(SourceKind::LocalGit, connector);

    let orch = build_orchestrator(pool.clone(), connectors, SinkRegistry::default());

    let request = GenerateRequest {
        person: person.clone(),
        sources: vec![handle],
        date,
        template_id: DEV_EOD_TEMPLATE_ID.to_string(),
        template_version: "0.0.1".to_string(),
        verbose_mode: false,
    };
    let handle = orch.generate_report(request).await;
    let run_id = handle.run_id;

    // Observe the terminal outcome, then drain the progress stream
    // in that order. Reversing the order would deadlock if the
    // background task is blocked on the unbounded channel (it is
    // not, but future changes shouldn't silently start requiring a
    // specific ordering).
    let outcome = handle.completion.await.expect("join");
    let progress = common::drain_progress(handle.progress_rx).await;

    assert_eq!(outcome.run_id, run_id);
    assert_eq!(outcome.status, SyncRunStatus::Completed);
    let draft_id = outcome.draft_id.expect("completed runs carry a draft id");
    assert!(outcome.cancel_reason.is_none());

    // Progress: at least one Starting, one Completed, and no Failed /
    // Cancelled. The fan-out path also emits one InProgress on the
    // run-wide stream after fan-out drains.
    assert!(
        progress
            .iter()
            .any(|p| matches!(p.phase, ProgressPhase::Starting { .. })),
        "expected a Starting phase, got: {progress:#?}",
    );
    assert!(
        progress
            .iter()
            .any(|p| matches!(p.phase, ProgressPhase::Completed { .. })),
        "expected a Completed phase, got: {progress:#?}",
    );
    assert!(
        !progress.iter().any(|p| matches!(
            p.phase,
            ProgressPhase::Failed { .. } | ProgressPhase::Cancelled { .. }
        )),
        "unexpected terminal failure / cancellation phase: {progress:#?}",
    );

    // Durability: the sync_runs row is Completed, the drafts row is
    // present and round-trips.
    let syncrun = dayseam_db::SyncRunRepo::new(pool.clone())
        .get(&run_id)
        .await
        .expect("sync_runs lookup")
        .expect("row present");
    assert_eq!(syncrun.status, SyncRunStatus::Completed);
    assert!(syncrun.finished_at.is_some());
    assert_eq!(syncrun.per_source_state.len(), 1);

    let draft = dayseam_db::DraftRepo::new(pool.clone())
        .get(&draft_id)
        .await
        .expect("drafts lookup")
        .expect("draft persisted");
    assert_eq!(draft.id, draft_id);
    assert_eq!(draft.date, date);
    assert_eq!(draft.template_id, DEV_EOD_TEMPLATE_ID);
    assert_eq!(draft.per_source_state.len(), 1);

    // DAY-52: the orchestrator now persists `activity_events` before
    // render so the evidence popover can hydrate them. The draft's
    // `evidence` rows point at event ids that must exist on disk;
    // otherwise every bullet shows "no longer on disk".
    let activity_ids: Vec<_> = draft
        .evidence
        .iter()
        .flat_map(|e| e.event_ids.iter().copied())
        .collect();
    assert!(
        !activity_ids.is_empty(),
        "happy path draft must reference at least one event",
    );
    let hydrated = dayseam_db::ActivityRepo::new(pool.clone())
        .get_many(&activity_ids)
        .await
        .expect("activity_events lookup");
    assert_eq!(
        hydrated.len(),
        activity_ids.len(),
        "every evidence event id must resolve to a persisted row",
    );

    // In-flight entry cleared (terminal path owns cleanup).
    assert_eq!(orch.in_flight_count().await, 0);
}
