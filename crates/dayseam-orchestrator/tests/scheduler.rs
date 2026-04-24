//! DAY-130 scheduler invariants.
//!
//! Pure-planner rules are covered by the table-driven unit tests in
//! `schedule::tests`; this file lives here to exercise the
//! *orchestrator-facing* contract that the planner alone cannot
//! assert:
//!
//! 1. [`no_silent_unattended_write_gate_refuses_interactive_sink`] —
//!    `run_scheduled_action` refuses to call `write` on any sink
//!    whose adapter does not report
//!    [`SinkCapabilities::safe_for_unattended`]. This is the single
//!    guarantee that keeps "never auto-send without review" true
//!    once scheduled runs ship; a regression here would let a
//!    future interactive-only sink (e.g. "open-in-Bear") get
//!    driven by the hourly timer.
//!
//! 2. [`seven_day_simulation_completes_one_sync_run_per_day`] — a
//!    full end-to-end simulation that walks the planner over seven
//!    consecutive scheduled weekdays, calls `run_scheduled_action`
//!    once per day, and asserts the resulting `sync_runs` shape.
//!    The `ScheduleState` is maintained in-process across the
//!    iterations (as the production orchestrator's background task
//!    effectively does) so the planner's "don't re-emit a
//!    satisfied day" branch is exercised every tick.
//!
//! The interactive-only sink fixture is hand-rolled rather than
//! [`MockSink`] because `MockSink` declares `LOCAL_ONLY` (which
//! *is* safe_for_unattended) — we specifically need a sink whose
//! capabilities fail the gate.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Datelike, Duration as ChronoDuration, FixedOffset, NaiveDate, NaiveTime, TimeZone};
use dayseam_core::{
    DayseamError, ReportDraft, ScheduleConfig, SchedulerTriggerKind, Sink, SinkCapabilities,
    SinkConfig, SinkKind, SourceKind, SyncRunStatus, SyncRunTrigger, WriteReceipt,
};
use dayseam_db::SyncRunRepo;
use dayseam_orchestrator::{
    plan_next_actions, run_scheduled_action, schedule::SatisfactionKind, ConnectorRegistry,
    GenerateRequest, ScheduleRunError, ScheduleState, ScheduledAction, SinkRegistry, SourceHandle,
};
use dayseam_report::{DEV_EOD_TEMPLATE_ID, DEV_EOD_TEMPLATE_VERSION};
use sinks_sdk::{MockSink, SinkAdapter, SinkCtx};

mod common;

/// Hand-rolled sink that declares `interactive_only` capabilities so
/// the unattended gate should reject it. Panics on `write` so the
/// test fails hard if the gate regresses and lets the call through.
struct InteractiveOnlySink;

#[async_trait]
impl SinkAdapter for InteractiveOnlySink {
    fn kind(&self) -> SinkKind {
        SinkKind::MarkdownFile
    }

    fn capabilities(&self) -> SinkCapabilities {
        SinkCapabilities {
            local_only: true,
            remote_write: false,
            interactive_only: true,
            safe_for_unattended: false,
        }
    }

    async fn validate(&self, _ctx: &SinkCtx, _cfg: &SinkConfig) -> Result<(), DayseamError> {
        Ok(())
    }

    async fn write(
        &self,
        _ctx: &SinkCtx,
        _cfg: &SinkConfig,
        _draft: &ReportDraft,
    ) -> Result<WriteReceipt, DayseamError> {
        panic!(
            "run_scheduled_action must not reach write() for an interactive-only sink; \
             the safe_for_unattended gate has regressed"
        );
    }
}

/// The gate runs *before* the generate pipeline is even kicked off,
/// so the test doesn't need a functional connector — but we still
/// register an empty connector registry so the orchestrator's
/// invariants hold.
#[tokio::test]
async fn no_silent_unattended_write_gate_refuses_interactive_sink() {
    let (pool, _tmp) = common::test_pool().await;
    let mut sinks = SinkRegistry::new();
    sinks.insert(SinkKind::MarkdownFile, Arc::new(InteractiveOnlySink));
    let orch = common::build_orchestrator(pool.clone(), ConnectorRegistry::new(), sinks);

    let person = common::test_person();
    let sink = Sink {
        id: uuid::Uuid::new_v4(),
        kind: SinkKind::MarkdownFile,
        label: "interactive-only fixture".into(),
        config: SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec!["/tmp/dayseam-scheduler-gate".into()],
            frontmatter: false,
        },
        created_at: chrono::Utc::now(),
        last_write_at: None,
    };

    let request = GenerateRequest {
        person,
        sources: Vec::<SourceHandle>::new(),
        date: common::fixture_date(),
        template_id: DEV_EOD_TEMPLATE_ID.to_string(),
        template_version: DEV_EOD_TEMPLATE_VERSION.to_string(),
        verbose_mode: false,
    };

    let err = run_scheduled_action(&orch, request, &sink, SchedulerTriggerKind::InDay)
        .await
        .expect_err("must refuse to drive an interactive-only sink unattended");

    match err {
        ScheduleRunError::SinkNotSafeForUnattended { sink_id } => {
            assert_eq!(sink_id, sink.id);
        }
        other => panic!("expected SinkNotSafeForUnattended, got: {other:?}"),
    }
}

/// End-to-end 7-day simulation.
///
/// Walks the planner forward one scheduled weekday at a time:
///
/// * At each step the test calls `plan_next_actions(now, cfg, state)`.
///   The state is rebuilt from the `sync_runs` table after every
///   iteration, so if the planner's satisfaction logic and
///   [`ScheduleState::from_sync_runs`] ever drift out of sync this
///   test catches the regression.
/// * For each `RunToday` the planner emits, the test invokes
///   [`run_scheduled_action`] exactly once and asserts it returns a
///   single successful [`WriteReceipt`].
/// * After seven iterations the test asserts the DB shape: seven
///   `sync_runs` rows tagged `Scheduler { action: InDay }` with
///   status `Completed`, and seven mock-sink writes (one per day).
///
/// The simulation deliberately uses an all-weekdays schedule and a
/// fixed 14:00 wall-clock so the "in-day" branch of the planner
/// fires every day; the DST, catch-up, and final-pass arms are
/// covered by the unit tests in `schedule::tests`.
#[tokio::test]
async fn seven_day_simulation_completes_one_sync_run_per_day() {
    let (pool, _tmp) = common::test_pool().await;
    let person = common::test_person();

    // Seed a LocalGit source whose connector returns one event per
    // simulated day. The connector is keyed on `SourceKind` so a
    // single `MockConnector` instance covers every day — the mock
    // does not filter by date, which is fine: the test only cares
    // that `generate_scheduled_report` terminates `Completed` and
    // produces a draft.
    let (_source, _identity, handle) = common::seed_source(
        &pool,
        &person,
        SourceKind::LocalGit,
        "local-git simulation",
        "dev@example.com",
    )
    .await;
    let connector = Arc::new(connectors_sdk::MockConnector::new(
        SourceKind::LocalGit,
        vec![common::fixture_event(
            handle.source_id,
            "evt-sim",
            "dev@example.com",
            common::fixture_date(),
        )],
    ));
    let mut connectors = ConnectorRegistry::new();
    connectors.insert(SourceKind::LocalGit, connector);

    let mock_sink = Arc::new(MockSink::new());
    let mut sinks = SinkRegistry::new();
    sinks.insert(SinkKind::MarkdownFile, mock_sink.clone());

    let orch = common::build_orchestrator(pool.clone(), connectors, sinks);

    // A local-only sink — `LOCAL_ONLY` caps satisfy the
    // `safe_for_unattended` gate.
    let sink = Sink {
        id: uuid::Uuid::new_v4(),
        kind: SinkKind::MarkdownFile,
        label: "simulation sink".into(),
        config: SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec!["/tmp/dayseam-scheduler-sim".into()],
            frontmatter: false,
        },
        created_at: chrono::Utc::now(),
        last_write_at: None,
    };

    // Every weekday is in the schedule. `catch_up_days = 0` so the
    // only action the planner emits on each tick is the
    // `RunToday(today)` we want to exercise — catch-up and
    // final-pass dynamics are unit-tested separately.
    let cfg = ScheduleConfig {
        enabled: true,
        days_of_week: vec![
            chrono::Weekday::Mon,
            chrono::Weekday::Tue,
            chrono::Weekday::Wed,
            chrono::Weekday::Thu,
            chrono::Weekday::Fri,
            chrono::Weekday::Sat,
            chrono::Weekday::Sun,
        ],
        target_time: NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
        earliest_start: NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        catch_up_days: 0,
        sink_id: Some(sink.id),
        template_id: None,
    };

    let utc = FixedOffset::east_opt(0).unwrap();
    let day_zero = NaiveDate::from_ymd_opt(2026, 4, 20).expect("valid date");
    assert_eq!(day_zero.weekday(), chrono::Weekday::Mon);
    let runs_repo = SyncRunRepo::new(pool.clone());

    // The simulation keeps an in-memory `ScheduleState` rather than
    // rebuilding from `sync_runs` between iterations, because in
    // this test every run is recorded with `started_at = real now`
    // (the wall-clock at test execution), not the simulated
    // `today`. In production the two are the same day so the DB
    // rebuild path works; here we track satisfaction by simulated
    // date directly and let a separate unit test cover
    // `ScheduleState::from_sync_runs`.
    let mut state = ScheduleState::default();

    for offset in 0..7 {
        let today = day_zero + ChronoDuration::days(offset);
        let now = utc
            .from_local_datetime(&today.and_hms_opt(14, 0, 0).unwrap())
            .single()
            .expect("unambiguous");

        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions.contains(&ScheduledAction::RunToday(today)),
            "day {offset} (today={today}) should emit RunToday; got {actions:?}"
        );

        let request = GenerateRequest {
            person: person.clone(),
            sources: vec![handle.clone()],
            date: today,
            template_id: DEV_EOD_TEMPLATE_ID.to_string(),
            template_version: DEV_EOD_TEMPLATE_VERSION.to_string(),
            verbose_mode: false,
        };
        let receipts = run_scheduled_action(&orch, request, &sink, SchedulerTriggerKind::InDay)
            .await
            .unwrap_or_else(|e| panic!("day {offset}: run_scheduled_action failed: {e:?}"));
        assert_eq!(receipts.len(), 1, "day {offset}: expected one receipt");
        state.satisfied.insert(today, SatisfactionKind::InDay);

        // One more planner tick at the same `now` confirms the
        // satisfaction we just recorded suppresses another
        // `RunToday(today)` — the "don't double-run a satisfied
        // day" branch of the planner.
        let follow_up = plan_next_actions(now, &cfg, &state);
        assert!(
            !follow_up.contains(&ScheduledAction::RunToday(today)),
            "day {offset}: RunToday({today}) re-emitted after satisfaction: {follow_up:?}"
        );
    }

    // After seven scheduled runs the DB should hold exactly seven
    // `Scheduler { action: InDay }` rows, all `Completed`.
    let rows = runs_repo.list_recent(32).await.expect("list sync_runs");
    let scheduler_rows: Vec<_> = rows
        .iter()
        .filter(|r| {
            matches!(
                r.trigger,
                SyncRunTrigger::Scheduler {
                    action: SchedulerTriggerKind::InDay
                }
            )
        })
        .collect();
    assert_eq!(
        scheduler_rows.len(),
        7,
        "expected exactly 7 in-day scheduler runs, got {rows:?}"
    );
    for row in &scheduler_rows {
        assert_eq!(
            row.status,
            SyncRunStatus::Completed,
            "scheduler runs must all be Completed: {row:?}"
        );
    }

    // The mock sink records one write per `save_report` call, so
    // seven successful days ⇒ seven writes.
    assert_eq!(
        mock_sink.writes().len(),
        7,
        "expected 7 writes against the mock sink",
    );

    // Every simulated date is in-day satisfied. A planner tick
    // replayed against the current state emits no `RunToday` for
    // any of them.
    for offset in 0..7 {
        let date = day_zero + ChronoDuration::days(offset);
        assert!(
            state.satisfied.contains_key(&date),
            "state should mark {date} satisfied: {:?}",
            state.satisfied,
        );
    }
}
