//! DAY-130 scheduler background loop.
//!
//! One long-lived Tokio task that wakes every hour (plus a small
//! jitter) and runs the planner against the current persisted
//! [`ScheduleConfig`]. Executable actions (`RunToday`,
//! `FinalPassYesterday`) fire through [`run_scheduled_action`]; the
//! unattended-write safety gate inside that function is the final
//! word on whether anything actually lands in the sink. Non-
//! executable actions (`CatchUp`) are surfaced to the frontend via
//! the `scheduler:catch-up-suggested` Tauri event so the banner can
//! prompt the user.
//!
//! The cold-start pass runs the same planner once, synchronously,
//! during [`spawn`] so the banner can render on first paint instead
//! of popping up an hour later.

use std::time::Duration as StdDuration;

use chrono::{FixedOffset, Local, Offset};
use dayseam_core::{ScheduleConfig, SCHEDULE_CONFIG_KEY};
use dayseam_db::{SettingsRepo, SinkRepo, SyncRunRepo};
use dayseam_orchestrator::{
    plan_next_actions, run_scheduled_action, ScheduleState, SchedulerRunRow,
};
use tauri::{AppHandle, Emitter, Manager};

use crate::ipc::scheduler::build_scheduler_request;
use crate::state::AppState;

/// How far back we scan `sync_runs` to build the planner's
/// satisfaction map. 90 days is well above the 30-day catch-up hard
/// cap and keeps the per-tick query trivially fast.
const RECENT_RUNS_LOOKBACK_LIMIT: i64 = 2_000;

/// Tauri event name the frontend subscribes to in order to render
/// the catch-up banner. Payload is a JSON array of ISO-8601 date
/// strings (one per missed day, oldest-first).
pub const CATCH_UP_EVENT: &str = "scheduler:catch-up-suggested";

/// Kick off the scheduler. Spawns a detached task; callers don't
/// need to await it. The returned future resolves after the cold-
/// start scan has posted its banner (if any), so the frontend's
/// `App` component can trust that the event has already fired by
/// the time it mounts its listener if the initial paint happens
/// after the cold-start tick.
pub fn spawn(app: AppHandle) {
    dayseam_core::runtime::supervised_spawn("scheduler_task", async move {
        // The very first tick runs immediately on boot so the user
        // sees the catch-up banner on first paint (if applicable)
        // and any in-window `RunToday` action starts without
        // waiting an hour.
        tick(&app).await;

        loop {
            let sleep = StdDuration::from_secs(hourly_sleep_secs());
            tokio::time::sleep(sleep).await;
            tick(&app).await;
        }
    });
}

fn hourly_sleep_secs() -> u64 {
    // 60 minutes plus a small jitter derived from the boot clock
    // so multiple Dayseam processes on one machine don't fire
    // their ticks on exactly the same wall-clock second. Using the
    // nanos of `Instant::now()` avoids pulling in `rand` for what
    // is a cosmetic de-synchronisation step.
    use std::time::{SystemTime, UNIX_EPOCH};
    let jitter_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()) % 31)
        .unwrap_or(0);
    3_600 + jitter_secs
}

async fn tick(app: &AppHandle) {
    let state = app.state::<AppState>();
    let cfg = match load_config(&state).await {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "scheduler: config load failed; skipping tick");
            return;
        }
    };
    if !cfg.enabled {
        return;
    }

    let schedule_state = match build_schedule_state(&state).await {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(%err, "scheduler: state build failed; skipping tick");
            return;
        }
    };

    let offset: FixedOffset = Local::now().offset().fix();
    let now = Local::now().with_timezone(&offset);
    let actions = plan_next_actions(now, &cfg, &schedule_state);
    drop(schedule_state);

    for action in actions {
        match action {
            dayseam_orchestrator::ScheduledAction::RunToday(date)
            | dayseam_orchestrator::ScheduledAction::FinalPassYesterday(date) => {
                if let Err(err) = execute_run(app, &cfg, date, match_trigger(&action)).await {
                    tracing::warn!(%date, error = %err, "scheduler tick: run failed");
                }
            }
            dayseam_orchestrator::ScheduledAction::CatchUp(dates) => {
                if dates.is_empty() {
                    continue;
                }
                let iso: Vec<String> = dates.iter().map(|d| d.to_string()).collect();
                if let Err(err) = app.emit(CATCH_UP_EVENT, iso) {
                    tracing::warn!(%err, "scheduler: failed to emit catch-up event");
                }
            }
        }
    }
}

fn match_trigger(
    action: &dayseam_orchestrator::ScheduledAction,
) -> dayseam_core::SchedulerTriggerKind {
    use dayseam_core::SchedulerTriggerKind as K;
    use dayseam_orchestrator::ScheduledAction::*;
    match action {
        RunToday(_) => K::InDay,
        FinalPassYesterday(_) => K::FinalPass,
        CatchUp(_) => K::CatchUp,
    }
}

async fn execute_run(
    app: &AppHandle,
    cfg: &ScheduleConfig,
    date: chrono::NaiveDate,
    trigger: dayseam_core::SchedulerTriggerKind,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let Some(sink_id) = cfg.sink_id else {
        return Err("no sink configured".into());
    };
    let sink = SinkRepo::new(state.pool.clone())
        .get(&sink_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("sink {sink_id} not found"))?;
    let request = build_scheduler_request(&state, cfg, date)
        .await
        .map_err(|e| e.to_string())?;
    run_scheduled_action(&state.orchestrator, request, &sink, trigger)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn load_config(state: &AppState) -> Result<ScheduleConfig, String> {
    SettingsRepo::new(state.pool.clone())
        .get::<ScheduleConfig>(SCHEDULE_CONFIG_KEY)
        .await
        .map(Option::unwrap_or_default)
        .map_err(|e| e.to_string())
}

async fn build_schedule_state(state: &AppState) -> Result<ScheduleState, String> {
    let runs = SyncRunRepo::new(state.pool.clone())
        .list_recent(RECENT_RUNS_LOOKBACK_LIMIT)
        .await
        .map_err(|e| e.to_string())?;

    let local_offset: FixedOffset = Local::now().offset().fix();
    let rows = runs.iter().map(|r| SchedulerRunRow {
        status: r.status,
        trigger: r.trigger,
        started_at: r.started_at.with_timezone(&Local),
        _phantom: std::marker::PhantomData,
    });
    let mut sched_state = ScheduleState::from_sync_runs(rows, local_offset);

    // Merge the session-scoped skip set into the planner input so
    // banner dismissals don't re-fire until the next app restart.
    let skipped = state.scheduler_skip.snapshot().await;
    sched_state.skipped_this_session.extend(skipped);

    // Defer whenever the orchestrator currently has any live user
    // run — the planner will skip RunToday / FinalPass while this
    // flag is true.
    let runs_guard = state.runs.read().await;
    sched_state.user_run_in_flight = !runs_guard.is_empty();

    Ok(sched_state)
}
