//! The DAY-130 report scheduler.
//!
//! ## Model
//!
//! A single global [`ScheduleConfig`] is persisted as a JSON blob
//! under [`SCHEDULE_CONFIG_KEY`] in the `settings` key/value table.
//! The frontend edits it through the Preferences dialog; the
//! orchestrator's hourly background tick reads it on every fire so
//! edits take effect without a restart.
//!
//! ## Planner
//!
//! [`plan_next_actions`] is a pure function: given `now`, a
//! [`ScheduleConfig`], and the [`ScheduleState`] derived from
//! `sync_runs`, it returns the set of [`ScheduledAction`]s that
//! should run this tick. Keeping the decision logic pure is what
//! lets the 7-day integration test (`tests/scheduler.rs`) drive the
//! same code over a simulated clock without touching Tauri.
//!
//! The rules are:
//!
//! 1. **In-day**: if today is a scheduled weekday and `now` is inside
//!    `[earliest_start, target_time]`, emit `RunToday(today)`
//!    unless today is already satisfied in-day.
//! 2. **Final pass**: on the *first* tick of a new calendar day,
//!    re-run yesterday if yesterday was a scheduled day that was
//!    never final-passed. This is what catches a 4:45 pm change
//!    made right before the laptop closed.
//! 3. **Catch-up**: every scheduled day within `catch_up_days` that
//!    has no `InDay` or `FinalPass` satisfaction is surfaced to the
//!    UI as a `CatchUp` action. The orchestrator deliberately does
//!    not execute these silently — the UI owns the prompt.
//!
//! ## Runner
//!
//! [`run_scheduled_action`] turns a planned [`ScheduledAction`] into
//! an actual report + save cycle. It's gated on
//! [`SinkCapabilities::safe_for_unattended`]: any attempt to fire a
//! scheduled run at a sink without that bit returns
//! [`ScheduleRunError::SinkNotSafeForUnattended`] without touching
//! the orchestrator. That property is the "never auto-send without
//! review" promise v0.1 made and v0.3 has to keep.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, NaiveDate};
use uuid::Uuid;

use dayseam_core::{
    DayseamError, ScheduleConfig, SchedulerTriggerKind, Sink, SyncRunCancelReason, SyncRunStatus,
    SyncRunTrigger, WriteReceipt,
};

use crate::orchestrator::{GenerateRequest, Orchestrator};

// `ScheduleConfig` and `SCHEDULE_CONFIG_KEY` live in `dayseam-core`
// so the Tauri IPC layer and the frontend can share the shape via
// `ts-rs`; the planner, state, and runner below stay here because
// they depend on `Orchestrator` and would otherwise pull all of
// `dayseam-orchestrator` into the core crate.
pub use dayseam_core::SCHEDULE_CONFIG_KEY;

/// Maximum days of catch-up the scheduler will ever surface, even
/// if the user configures a larger value. Mirrors the UI slider
/// cap so the backend is resilient to a hand-edited `settings` row.
pub const CATCH_UP_DAYS_HARD_CAP: u32 = 30;

/// Whether a scheduled day has already been produced, and by what
/// action. Derived at runtime from the `sync_runs` table — see
/// [`ScheduleState::from_recent_runs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SatisfactionKind {
    /// A run landed while `now < target_time` on the scheduled day.
    InDay,
    /// A re-run landed on a later calendar day (the "final pass").
    FinalPass,
}

/// Runtime view the planner consumes.
#[derive(Clone, Debug, Default)]
pub struct ScheduleState {
    /// Per-date satisfaction kind. A `FinalPass` entry wins over an
    /// `InDay` entry for the same date.
    pub satisfied: std::collections::BTreeMap<NaiveDate, SatisfactionKind>,
    /// Set of dates the current session has already asked the user
    /// about and been told to skip. Non-persistent — recomputed on
    /// every app boot — so a user who says "skip" on Tuesday is
    /// asked again on Wednesday.
    pub skipped_this_session: BTreeSet<NaiveDate>,
    /// True iff the orchestrator has a user-initiated run actively
    /// in flight. The planner defers in-day and final-pass actions
    /// while this is true to avoid fighting with the user.
    pub user_run_in_flight: bool,
}

impl ScheduleState {
    /// Build a [`ScheduleState`] from the raw `sync_runs` rows
    /// recorded by the scheduler.
    ///
    /// A row counts as satisfaction iff its `status == Completed`.
    /// DAY-170: both scheduler-triggered **and** user-triggered
    /// completed runs now satisfy a day. The earlier cut
    /// (scheduler-only) matched the design intent "we only promise
    /// reports via the scheduler path", but it produced a concrete
    /// user-facing bug: a user who opened Dayseam at 10am every
    /// morning and manually generated the day's report would still
    /// be nagged on the next open with "Catch up 3 missed reports"
    /// — the catch-up planner counted only the scheduler's own
    /// runs, not the reports the user had already generated (and
    /// likely saved). Surfacing a catch-up prompt for days the
    /// user has already produced a report is the discoverability
    /// equivalent of a false positive; the planner now treats any
    /// completed run as satisfying the day so the prompt only
    /// fires when the day really is empty of output.
    ///
    /// `InDay` and `FinalPass` both register; the latter wins when
    /// both exist for the same date (catches the "retried the same
    /// day" corner case). User-triggered rows register as `InDay`
    /// because they carry no "final pass" semantics — the trigger
    /// kind distinction only meaningfully ranks scheduler rows.
    pub fn from_sync_runs<'a>(
        rows: impl Iterator<Item = SchedulerRunRow<'a>>,
        local_tz: FixedOffset,
    ) -> Self {
        let mut state = Self::default();
        for row in rows {
            if row.status != SyncRunStatus::Completed {
                continue;
            }
            // The run's wall-clock start gives us the "which
            // scheduled day satisfied" for in-day and user-triggered
            // rows; scheduler final-pass rows carry the *previous*
            // scheduled day as their target, which the scheduler
            // records by re-using the `request.date` the planner
            // emitted.
            let started_local = row.started_at.with_timezone(&local_tz).date_naive();
            let (target_date, kind) = match row.trigger {
                SyncRunTrigger::Scheduler { action } => match action {
                    SchedulerTriggerKind::InDay | SchedulerTriggerKind::CatchUp => {
                        (started_local, SatisfactionKind::InDay)
                    }
                    SchedulerTriggerKind::FinalPass => (
                        started_local - Duration::days(1),
                        SatisfactionKind::FinalPass,
                    ),
                },
                // Any non-scheduler trigger (User, API, future
                // kinds) counts as satisfaction for `started_local`
                // with `InDay` strength. We don't try to attribute
                // a user-initiated run to "yesterday's final pass"
                // — if the user ran at 00:30 on the 18th for the
                // 17th, the run's `request.date` will be 17th and
                // the row recorded for a separate scheduler pass
                // would override this with `FinalPass` if one ever
                // lands, but the pragmatic case (user runs during
                // the current day) is handled correctly here.
                _ => (started_local, SatisfactionKind::InDay),
            };
            state
                .satisfied
                .entry(target_date)
                .and_modify(|existing| {
                    if kind == SatisfactionKind::FinalPass {
                        *existing = kind;
                    }
                })
                .or_insert(kind);
        }
        state
    }
}

/// Minimal projection of a `sync_runs` row the scheduler needs.
/// Borrowed lifetime so the call site can pass rows from an sqlx
/// query without cloning.
#[derive(Clone, Copy, Debug)]
pub struct SchedulerRunRow<'a> {
    pub status: SyncRunStatus,
    pub trigger: SyncRunTrigger,
    pub started_at: DateTime<Local>,
    /// Unused in v1 but kept on the projection so a future test
    /// that cares about "which row won" can assert on it without
    /// re-shaping the projection. The `'a` lifetime is a forward
    /// hedge.
    pub _phantom: std::marker::PhantomData<&'a ()>,
}

/// What the planner wants the caller to do this tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduledAction {
    /// Produce today's report. Emitted during the in-window hours
    /// of a scheduled weekday.
    RunToday(NaiveDate),
    /// Re-run the previous scheduled day as a final pass. Emitted
    /// on the first tick of a new calendar day.
    FinalPassYesterday(NaiveDate),
    /// List of scheduled days that are still unsatisfied — the UI
    /// should prompt the user before running these. The planner
    /// never produces a `CatchUp` for the current day; use
    /// `RunToday` for that.
    CatchUp(Vec<NaiveDate>),
}

/// Pure planner. Returns every action that should fire right now,
/// in the order the caller should execute them. Kept deterministic
/// so the table-driven tests can pin the exact sequence.
///
/// The `now` argument is a local `DateTime<FixedOffset>` so DST
/// shifts behave the way a user would read them: "18:00" is always
/// 18:00 regardless of which offset the local clock wears today.
pub fn plan_next_actions(
    now: DateTime<FixedOffset>,
    cfg: &ScheduleConfig,
    state: &ScheduleState,
) -> Vec<ScheduledAction> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut actions = Vec::new();

    let today = now.date_naive();
    let yesterday = today - Duration::days(1);
    let wallclock = now.time();

    // Rule 2 (FinalPass) runs first because a tick that crosses
    // midnight should finalise yesterday before it considers running
    // today. Skipped when a user run is in flight.
    if !state.user_run_in_flight
        && cfg.days_of_week.contains(&yesterday.weekday())
        && state.satisfied.get(&yesterday).copied() != Some(SatisfactionKind::FinalPass)
    {
        actions.push(ScheduledAction::FinalPassYesterday(yesterday));
    }

    // Rule 1 (RunToday): only when today is scheduled, we're in
    // window, no user run is in flight, and today isn't already
    // satisfied in-day. (A FinalPass for today can only land on a
    // future tick; this branch runs only while we're still in-day.)
    if !state.user_run_in_flight
        && cfg.days_of_week.contains(&today.weekday())
        && wallclock >= cfg.earliest_start
        && wallclock <= cfg.target_time
        && !state.satisfied.contains_key(&today)
    {
        actions.push(ScheduledAction::RunToday(today));
    }

    // Rule 3 (CatchUp): every scheduled day in the past window
    // (excluding yesterday when we already emitted a final pass
    // for it) that has no satisfaction and wasn't skipped this
    // session.
    let catch_up_days = cfg.catch_up_days.min(CATCH_UP_DAYS_HARD_CAP);
    let mut missed: Vec<NaiveDate> = Vec::new();
    for offset in 1..=i64::from(catch_up_days) {
        let d = today - Duration::days(offset);
        if !cfg.days_of_week.contains(&d.weekday()) {
            continue;
        }
        if state.satisfied.contains_key(&d) {
            continue;
        }
        if state.skipped_this_session.contains(&d) {
            continue;
        }
        // Yesterday is handled by the FinalPass arm above iff it's
        // unsatisfied — don't double-count it as a catch-up.
        if d == yesterday
            && actions
                .iter()
                .any(|a| matches!(a, ScheduledAction::FinalPassYesterday(dt) if *dt == yesterday))
        {
            continue;
        }
        missed.push(d);
    }
    if !missed.is_empty() {
        // Oldest first so the UI label reads "from Mon 14 …" not
        // "… to Mon 14".
        missed.sort();
        actions.push(ScheduledAction::CatchUp(missed));
    }

    actions
}

/// Typed error the runner returns. Surfaced to the caller so the
/// Tauri layer can toast, log, or ignore on its own terms.
#[derive(Debug, thiserror::Error)]
pub enum ScheduleRunError {
    #[error("scheduler configuration does not have a sink_id set")]
    NoSinkConfigured,
    #[error("sink {sink_id} is not safe_for_unattended; scheduled writes are refused")]
    SinkNotSafeForUnattended { sink_id: Uuid },
    #[error("sink {sink_id} not found")]
    SinkNotFound { sink_id: Uuid },
    /// The sink's adapter is not registered. Should only happen if a
    /// sink row refers to a `SinkKind` the current binary does not
    /// ship (e.g. a future release downgrade).
    #[error("sink kind {sink_kind:?} is not registered in this build")]
    SinkKindNotRegistered { sink_kind: dayseam_core::SinkKind },
    /// The generate step did not complete successfully (cancelled or
    /// failed). Scheduled runs do not surface a recovery UI so the
    /// runner just logs + returns this to the caller.
    #[error(
        "scheduled generate did not complete: status={status:?}, cancel_reason={cancel_reason:?}"
    )]
    GenerateDidNotComplete {
        status: SyncRunStatus,
        cancel_reason: Option<SyncRunCancelReason>,
    },
    /// The completion task panicked / was aborted before it could
    /// report a terminal status. Wrapped as a plain string because
    /// `JoinError` is not `Clone` and we want the error type to
    /// survive an `Arc`-share without special handling.
    #[error("scheduled generate completion task did not return a status: {0}")]
    GenerateJoinError(String),
    /// The save step failed. The draft is still persisted so a user
    /// can retry via the normal UI path; the scheduler just refuses
    /// to claim satisfaction for this date.
    #[error(transparent)]
    Save(#[from] DayseamError),
}

/// Execute one planned [`ScheduledAction`] end-to-end: generate the
/// report, wait for it to terminate, and on success save the
/// resulting draft to `sink`. The caller owns db assembly
/// (`person`, `sources`, template fields, verbose flag) so the
/// orchestrator stays agnostic about how those are sourced.
///
/// Returns the [`WriteReceipt`]s from the sink on success. Errors
/// bubble up as [`ScheduleRunError`] — the caller decides whether to
/// log + surface a toast or silently keep the schedule alive.
///
/// Safety gate: the runner refuses to touch `sink` unless the
/// registered adapter reports
/// [`dayseam_core::SinkCapabilities::safe_for_unattended`]. This is
/// the *only* place the guarantee is enforced on the scheduled
/// path; the IPC layer's `report_save` runs under a user, so it
/// doesn't share this code path.
pub async fn run_scheduled_action(
    orch: &Orchestrator,
    request: GenerateRequest,
    sink: &Sink,
    trigger_kind: SchedulerTriggerKind,
) -> Result<Vec<WriteReceipt>, ScheduleRunError> {
    let adapter = orch
        .sinks()
        .get(sink.kind)
        .ok_or(ScheduleRunError::SinkKindNotRegistered {
            sink_kind: sink.kind,
        })?;
    if !adapter.capabilities().safe_for_unattended {
        return Err(ScheduleRunError::SinkNotSafeForUnattended { sink_id: sink.id });
    }

    let handle = orch.generate_scheduled_report(request, trigger_kind).await;
    let outcome = handle
        .completion
        .await
        .map_err(|e| ScheduleRunError::GenerateJoinError(e.to_string()))?;
    if outcome.status != SyncRunStatus::Completed {
        return Err(ScheduleRunError::GenerateDidNotComplete {
            status: outcome.status,
            cancel_reason: outcome.cancel_reason,
        });
    }
    let Some(draft_id) = outcome.draft_id else {
        // A `Completed` run without a `draft_id` is a bug upstream
        // (the generate pipeline's terminal invariant), but we
        // refuse to claim satisfaction for the date anyway rather
        // than panic here.
        return Err(ScheduleRunError::GenerateDidNotComplete {
            status: outcome.status,
            cancel_reason: None,
        });
    };
    let receipts = orch.save_report(draft_id, sink).await?;
    Ok(receipts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveTime, TimeZone, Weekday};

    fn local_offset() -> FixedOffset {
        // Use a stable offset for tests so wall-clock comparisons
        // don't drift with the test machine's local TZ.
        FixedOffset::east_opt(0).expect("UTC is a valid FixedOffset")
    }

    fn at(date: NaiveDate, h: u32, m: u32) -> DateTime<FixedOffset> {
        let offset = local_offset();
        offset
            .from_local_datetime(&date.and_hms_opt(h, m, 0).expect("valid time"))
            .single()
            .expect("unambiguous")
    }

    fn weekday_cfg(days: &[Weekday]) -> ScheduleConfig {
        ScheduleConfig {
            enabled: true,
            days_of_week: days.to_vec(),
            target_time: NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
            earliest_start: NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
            catch_up_days: 7,
            sink_id: Some(Uuid::new_v4()),
            template_id: None,
        }
    }

    #[test]
    fn disabled_schedule_plans_nothing() {
        let mut cfg = weekday_cfg(&[Weekday::Mon]);
        cfg.enabled = false;
        let state = ScheduleState::default();
        let now = at(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(), 14, 0);
        assert!(plan_next_actions(now, &cfg, &state).is_empty());
    }

    #[test]
    fn in_window_emits_run_today() {
        // Zero catch-up so the planner emits only the RunToday arm.
        // The general catch-up interaction is covered by its own
        // tests below.
        let mut cfg = weekday_cfg(&[Weekday::Mon]);
        cfg.catch_up_days = 0;
        let state = ScheduleState::default();
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        assert_eq!(mon.weekday(), Weekday::Mon);
        let now = at(mon, 14, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert_eq!(actions, vec![ScheduledAction::RunToday(mon)]);
    }

    #[test]
    fn before_earliest_start_does_not_run_today() {
        let cfg = weekday_cfg(&[Weekday::Mon]);
        let state = ScheduleState::default();
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let now = at(mon, 8, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, ScheduledAction::RunToday(_))),
            "08:00 is before earliest_start=12:00: {actions:?}"
        );
    }

    #[test]
    fn after_target_on_non_scheduled_day_does_nothing() {
        // After-target on a non-scheduled weekday: nothing to do.
        // Monday is the only scheduled day.
        let cfg = weekday_cfg(&[Weekday::Mon]);
        let state = ScheduleState::default();
        let tue = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        // Tuesday at 20:00 — yesterday (Monday) has no satisfaction,
        // so the FinalPass rule fires. But the RunToday rule does
        // not because Tuesday is not scheduled.
        let now = at(tue, 20, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ScheduledAction::FinalPassYesterday(d) if *d == tue - Duration::days(1))),
            "expected FinalPassYesterday for Mon, got {actions:?}"
        );
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, ScheduledAction::RunToday(_))),
            "Tuesday is not scheduled, RunToday must not appear"
        );
    }

    #[test]
    fn already_satisfied_in_day_does_not_repeat() {
        let mut cfg = weekday_cfg(&[Weekday::Mon]);
        cfg.catch_up_days = 0;
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let mut state = ScheduleState::default();
        state.satisfied.insert(mon, SatisfactionKind::InDay);
        let now = at(mon, 17, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, ScheduledAction::RunToday(_))),
            "today is already in-day satisfied: {actions:?}"
        );
    }

    #[test]
    fn final_pass_fires_on_next_day_for_unsatisfied_yesterday() {
        let cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue]);
        let state = ScheduleState::default();
        let tue = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        // Early Tuesday, yesterday (Monday) wasn't produced.
        let now = at(tue, 9, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions.contains(&ScheduledAction::FinalPassYesterday(
                tue - Duration::days(1)
            )),
            "{actions:?}"
        );
    }

    #[test]
    fn final_pass_does_not_repeat_when_yesterday_already_final_passed() {
        let cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue]);
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let tue = mon + Duration::days(1);
        let mut state = ScheduleState::default();
        state.satisfied.insert(mon, SatisfactionKind::FinalPass);
        let now = at(tue, 9, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, ScheduledAction::FinalPassYesterday(_))),
            "{actions:?}"
        );
    }

    #[test]
    fn catch_up_lists_missed_scheduled_days_oldest_first() {
        let cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue, Weekday::Wed]);
        let state = ScheduleState::default();
        // Thursday 20:00 after 3 missed scheduled days in a row.
        let thu = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        assert_eq!(thu.weekday(), Weekday::Thu);
        let now = at(thu, 20, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        // We expect FinalPass(Wed) and CatchUp([Mon, Tue]) — Wed
        // gets the final-pass slot, not a catch-up entry.
        let catch_up: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                ScheduledAction::CatchUp(v) => Some(v.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(catch_up.len(), 1, "expected one CatchUp block: {actions:?}");
        let days = &catch_up[0];
        assert_eq!(
            days.len(),
            2,
            "Wed is final-passed, not caught up: {days:?}"
        );
        assert!(days.windows(2).all(|w| w[0] < w[1]), "oldest first");
    }

    #[test]
    fn catch_up_skips_session_dismissals() {
        let cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue, Weekday::Wed]);
        let mut state = ScheduleState::default();
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        state.skipped_this_session.insert(mon);
        let thu = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let now = at(thu, 20, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        // Tuesday is the only remaining unsatisfied-and-not-skipped
        // catch-up day (Wed is final-passed).
        for action in &actions {
            if let ScheduledAction::CatchUp(days) = action {
                assert!(!days.contains(&mon), "Mon was skipped: {days:?}");
            }
        }
    }

    #[test]
    fn user_run_in_flight_defers_in_day_and_final_pass_but_still_reports_catch_up() {
        let cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue]);
        let state = ScheduleState {
            user_run_in_flight: true,
            ..Default::default()
        };
        let tue = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let now = at(tue, 14, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions
                .iter()
                .all(|a| matches!(a, ScheduledAction::CatchUp(_))),
            "in-day / final-pass must defer while user run is active: {actions:?}"
        );
    }

    #[test]
    fn schedule_config_json_roundtrip_is_stable() {
        // Guard the on-disk shape: if a future edit renames a field
        // or drops `Vec` for `Weekday`, this test catches it before
        // we ship a migration-less shape change.
        let cfg = ScheduleConfig {
            enabled: true,
            days_of_week: vec![Weekday::Mon, Weekday::Wed, Weekday::Fri],
            target_time: NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
            earliest_start: NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
            catch_up_days: 7,
            sink_id: None,
            template_id: None,
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: ScheduleConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn past_target_time_does_not_run_today_but_still_final_passes_yesterday() {
        // today = Tue at 20:00 (past target_time=18:00). Yesterday
        // (Mon) is scheduled and unsatisfied. The planner should
        // refuse RunToday (the window closed) but still fire
        // FinalPassYesterday(Mon).
        let mut cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue]);
        cfg.catch_up_days = 0;
        let state = ScheduleState::default();
        let tue = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let now = at(tue, 20, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ScheduledAction::FinalPassYesterday(_))),
            "{actions:?}"
        );
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, ScheduledAction::RunToday(_))),
            "past target_time: RunToday must not fire: {actions:?}"
        );
    }

    #[test]
    fn catch_up_lists_zero_when_every_scheduled_day_is_satisfied() {
        let cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue, Weekday::Wed]);
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let tue = mon + Duration::days(1);
        let wed = mon + Duration::days(2);
        let thu = mon + Duration::days(3);
        let mut state = ScheduleState::default();
        state.satisfied.insert(mon, SatisfactionKind::FinalPass);
        state.satisfied.insert(tue, SatisfactionKind::FinalPass);
        state.satisfied.insert(wed, SatisfactionKind::FinalPass);
        let now = at(thu, 20, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, ScheduledAction::CatchUp(_))),
            "fully-satisfied window should emit no CatchUp: {actions:?}"
        );
    }

    #[test]
    fn catch_up_lists_exactly_one_missed_day() {
        // Mon/Tue/Wed scheduled. Only Monday missed (Tue+Wed
        // satisfied). From Friday, yesterday (Thu) is not a scheduled
        // day so the FinalPass rule doesn't consume Monday; the
        // planner should surface a CatchUp of length 1.
        let cfg = weekday_cfg(&[Weekday::Mon, Weekday::Tue, Weekday::Wed]);
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let tue = mon + Duration::days(1);
        let wed = mon + Duration::days(2);
        let fri = mon + Duration::days(4);
        let mut state = ScheduleState::default();
        state.satisfied.insert(tue, SatisfactionKind::FinalPass);
        state.satisfied.insert(wed, SatisfactionKind::FinalPass);
        let now = at(fri, 20, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        let missed: Vec<NaiveDate> = actions
            .iter()
            .filter_map(|a| match a {
                ScheduledAction::CatchUp(v) => Some(v.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            missed,
            vec![mon],
            "expected a single missed day: {actions:?}"
        );
    }

    #[test]
    fn dst_spring_forward_preserves_wallclock_semantics() {
        // On the US spring-forward day the local clock jumps
        // 02:00 → 03:00. Because the planner compares wall-clock
        // `NaiveTime`s from the supplied `DateTime<FixedOffset>`,
        // "14:00" on both sides of the jump should behave the same
        // way: an in-window RunToday. Expressed by instantiating
        // `now` with a post-shift offset.
        let mut cfg = weekday_cfg(&[Weekday::Sun]);
        cfg.catch_up_days = 0;
        let state = ScheduleState::default();
        // 2026-03-08 is the US DST-forward Sunday.
        let sunday = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        assert_eq!(sunday.weekday(), Weekday::Sun);
        let edt = FixedOffset::east_opt(-4 * 3_600).unwrap();
        let now = edt
            .from_local_datetime(&sunday.and_hms_opt(14, 0, 0).unwrap())
            .single()
            .unwrap();
        let actions = plan_next_actions(now, &cfg, &state);
        assert_eq!(actions, vec![ScheduledAction::RunToday(sunday)]);
    }

    #[test]
    fn dst_fall_back_preserves_wallclock_semantics() {
        // Fall-back: the local clock rolls 02:00 → 01:00 on the US
        // fallback Sunday. A tick at 14:00 wall-clock should still
        // behave as "in-window" regardless of which half of the
        // ambiguous hour passed earlier in the day.
        let mut cfg = weekday_cfg(&[Weekday::Sun]);
        cfg.catch_up_days = 0;
        let state = ScheduleState::default();
        // 2026-11-01 is the US DST-back Sunday.
        let sunday = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        assert_eq!(sunday.weekday(), Weekday::Sun);
        let est = FixedOffset::east_opt(-5 * 3_600).unwrap();
        let now = est
            .from_local_datetime(&sunday.and_hms_opt(14, 0, 0).unwrap())
            .single()
            .unwrap();
        let actions = plan_next_actions(now, &cfg, &state);
        assert_eq!(actions, vec![ScheduledAction::RunToday(sunday)]);
    }

    #[test]
    fn from_sync_runs_counts_user_triggered_completed_runs_as_satisfaction() {
        // DAY-170 regression: a user who opens Dayseam and manually
        // generates a report must not be prompted with a catch-up
        // banner for that same day on the next tick. Before DAY-170,
        // `from_sync_runs` filtered out `SyncRunTrigger::User`, which
        // meant the only way a day could be marked satisfied was for
        // the scheduler itself to fire. This test pins the new
        // contract: completed **user** runs count too.
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let started = local_offset()
            .from_local_datetime(&mon.and_hms_opt(10, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Local);
        let rows = std::iter::once(SchedulerRunRow {
            status: SyncRunStatus::Completed,
            trigger: SyncRunTrigger::User,
            started_at: started,
            _phantom: std::marker::PhantomData,
        });
        let state = ScheduleState::from_sync_runs(rows, local_offset());
        assert_eq!(
            state.satisfied.get(&mon),
            Some(&SatisfactionKind::InDay),
            "a completed user-triggered run satisfies its wall-clock day"
        );
    }

    #[test]
    fn from_sync_runs_still_treats_scheduler_final_pass_as_yesterday() {
        // Companion to the DAY-170 test above: scheduler final-pass
        // rows keep their pre-DAY-170 semantics of "yesterday's
        // satisfaction", even though user rows now fold into the
        // same code path. This guards against accidentally folding
        // final-pass into `started_local` when refactoring the
        // match arms.
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let tue = mon + Duration::days(1);
        // Final-pass rows land on the day *after* the target day.
        let started = local_offset()
            .from_local_datetime(&tue.and_hms_opt(9, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Local);
        let rows = std::iter::once(SchedulerRunRow {
            status: SyncRunStatus::Completed,
            trigger: SyncRunTrigger::Scheduler {
                action: SchedulerTriggerKind::FinalPass,
            },
            started_at: started,
            _phantom: std::marker::PhantomData,
        });
        let state = ScheduleState::from_sync_runs(rows, local_offset());
        assert_eq!(
            state.satisfied.get(&mon),
            Some(&SatisfactionKind::FinalPass),
            "scheduler final-pass row on Tue marks Mon as final-passed"
        );
        assert!(
            !state.satisfied.contains_key(&tue),
            "final-pass must not also mark its own start date as satisfied"
        );
    }

    #[test]
    fn from_sync_runs_final_pass_overrides_in_day_for_same_date() {
        // Two runs for the same date: an InDay (could be user or
        // scheduler-in-day) and a scheduler FinalPass. The
        // FinalPass must win regardless of iteration order.
        let mon = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let tue = mon + Duration::days(1);
        let in_day_started = local_offset()
            .from_local_datetime(&mon.and_hms_opt(14, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Local);
        let final_pass_started = local_offset()
            .from_local_datetime(&tue.and_hms_opt(9, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Local);
        let rows = vec![
            SchedulerRunRow {
                status: SyncRunStatus::Completed,
                trigger: SyncRunTrigger::User,
                started_at: in_day_started,
                _phantom: std::marker::PhantomData,
            },
            SchedulerRunRow {
                status: SyncRunStatus::Completed,
                trigger: SyncRunTrigger::Scheduler {
                    action: SchedulerTriggerKind::FinalPass,
                },
                started_at: final_pass_started,
                _phantom: std::marker::PhantomData,
            },
        ];
        let state = ScheduleState::from_sync_runs(rows.into_iter(), local_offset());
        assert_eq!(
            state.satisfied.get(&mon),
            Some(&SatisfactionKind::FinalPass),
            "final-pass wins over in-day for the same target date"
        );
    }

    #[test]
    fn catch_up_days_is_clamped_to_hard_cap() {
        let mut cfg = weekday_cfg(&[
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ]);
        cfg.catch_up_days = 365;
        let state = ScheduleState::default();
        let today = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let now = at(today, 20, 0);
        let actions = plan_next_actions(now, &cfg, &state);
        let catch_up_len: usize = actions
            .iter()
            .filter_map(|a| match a {
                ScheduledAction::CatchUp(v) => Some(v.len()),
                _ => None,
            })
            .sum();
        // 30 days hard cap minus yesterday (final-passed) = 29
        assert_eq!(catch_up_len, (CATCH_UP_DAYS_HARD_CAP as usize) - 1);
    }
}
