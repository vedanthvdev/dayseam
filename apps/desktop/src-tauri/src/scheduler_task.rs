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

use chrono::{FixedOffset, Local, NaiveDate, Offset};
use dayseam_core::{ScheduleConfig, SinkConfig, SCHEDULE_CONFIG_KEY};
use dayseam_db::{SettingsRepo, SinkRepo, SyncRunRepo};
use dayseam_orchestrator::{
    plan_next_actions, run_scheduled_action, SatisfactionKind, ScheduleState, SchedulerRunRow,
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

    // DAY-170: also treat "the user has a saved markdown report
    // file for this date in a configured sink" as satisfaction for
    // that date. This is the second half of the fix to the false
    // "Catch up N missed reports" nag — together with the
    // completed-run change in `from_sync_runs`, it ensures the
    // banner never fires for a day whose report is either in the
    // database **or** already on disk. We scan the filesystem here
    // (rather than in `dayseam-orchestrator`) so the planner stays
    // pure and filesystem-free; the state we hand it already has
    // the disk view folded in.
    //
    // Conservative on purpose: any parse error, missing directory,
    // or permission denied is logged and skipped — the worst-case
    // fallback is the pre-DAY-170 behaviour (nag the user), which
    // is strictly better than silently swallowing a missing report.
    if let Err(err) = augment_with_sink_files(state, &mut sched_state).await {
        tracing::warn!(%err, "scheduler: sink file scan failed; proceeding with run-based state only");
    }

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

/// Scan every configured markdown-file sink for files named
/// `Dayseam <YYYY-MM-DD>.md` and fold their dates into
/// `sched_state.satisfied` with `SatisfactionKind::InDay`. A file
/// on disk can't tell us whether it's a "final pass" or an "in-day"
/// render, but the planner only cares about presence/absence — any
/// `satisfied` entry prevents the catch-up action from firing, so
/// the kind distinction is cosmetic here.
///
/// Only files whose date portion parses as a valid ISO date are
/// counted; the sink writes files with exactly this shape, so a
/// parse failure means the file is either a user-renamed report or
/// an unrelated `.md` the user dropped in the directory. Either
/// way, treating it as non-matching is the safe choice.
async fn augment_with_sink_files(
    state: &AppState,
    sched_state: &mut ScheduleState,
) -> Result<(), String> {
    let sinks = SinkRepo::new(state.pool.clone())
        .list()
        .await
        .map_err(|e| e.to_string())?;

    for sink in sinks {
        let SinkConfig::MarkdownFile { dest_dirs, .. } = &sink.config;
        for dir in dest_dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(it) => it,
                Err(err) => {
                    tracing::debug!(
                        sink_id = %sink.id,
                        dir = %dir.display(),
                        error = %err,
                        "scheduler: sink dir unreadable; skipping"
                    );
                    continue;
                }
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let Some(date) = date_from_report_filename(&name) else {
                    continue;
                };
                sched_state
                    .satisfied
                    .entry(date)
                    .or_insert(SatisfactionKind::InDay);
            }
        }
    }

    Ok(())
}

/// Return `Some(date)` iff `name` matches the `Dayseam <YYYY-MM-DD>.md`
/// filename pattern that the markdown-file sink writes. We reconstruct
/// the expected filename from the ISO-formatted date and compare the
/// whole string so callers can't accidentally match a partial prefix
/// (e.g. `Dayseam 2026-04-20-draft.md`).
fn date_from_report_filename(name: &str) -> Option<NaiveDate> {
    // Fast reject: the pattern is `Dayseam YYYY-MM-DD.md`, which is
    // always exactly 22 bytes. Anything else can't match.
    const EXPECTED_LEN: usize = "Dayseam YYYY-MM-DD.md".len();
    if name.len() != EXPECTED_LEN {
        return None;
    }
    // Everything after the leading "Dayseam " prefix and before the
    // trailing ".md" suffix must parse as a date.
    let without_prefix = name.strip_prefix("Dayseam ")?;
    let without_suffix = without_prefix.strip_suffix(".md")?;
    let date = NaiveDate::parse_from_str(without_suffix, "%Y-%m-%d").ok()?;
    // Cross-check against the canonical builder — if it diverges,
    // the filename isn't the one the sink would produce.
    (sink_markdown_file::report_filename_for_date(date) == name).then_some(date)
}

#[cfg(test)]
mod tests {
    use super::date_from_report_filename;
    use chrono::NaiveDate;

    #[test]
    fn parses_canonical_report_filename() {
        assert_eq!(
            date_from_report_filename("Dayseam 2026-04-20.md"),
            Some(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap())
        );
    }

    #[test]
    fn rejects_near_misses() {
        // Wrong prefix casing: the sink writes a capital D; we don't
        // accept `dayseam` / `DAYSEAM` etc., because the sink itself
        // never writes those and matching them would count unrelated
        // files the user happened to drop into the directory.
        assert!(date_from_report_filename("dayseam 2026-04-20.md").is_none());
        // Missing leading space between prefix and date.
        assert!(date_from_report_filename("Dayseam2026-04-20.md").is_none());
        // Extra trailing suffix, e.g. a user-renamed draft.
        assert!(date_from_report_filename("Dayseam 2026-04-20-draft.md").is_none());
        // Not a date at all.
        assert!(date_from_report_filename("Dayseam hello-world.md").is_none());
        // Wrong extension.
        assert!(date_from_report_filename("Dayseam 2026-04-20.txt").is_none());
        // Invalid date (February 30) — `parse_from_str` accepts the
        // *format* but rejects the *value*, which is what we want.
        assert!(date_from_report_filename("Dayseam 2026-02-30.md").is_none());
    }
}
