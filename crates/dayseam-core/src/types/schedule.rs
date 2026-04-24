//! DAY-130 scheduler configuration shared between the orchestrator
//! (planner / runner), the Tauri IPC surface, and the frontend.
//!
//! Kept in `dayseam-core` rather than `dayseam-orchestrator` so the
//! type can ride the existing `ts_types_generated` pipeline — the
//! IPC commands that expose it (`scheduler_get_config` /
//! `scheduler_set_config`) need a matching TypeScript shape, and
//! `dayseam-core` is the only crate both sides already depend on.
//!
//! The planner, `ScheduleState`, and `run_scheduled_action` all
//! continue to live in `crates/dayseam-orchestrator/src/schedule`.

use chrono::{NaiveTime, Weekday};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// `settings` key under which the serialised [`ScheduleConfig`] is
/// persisted. Versioned (`scheduler.v1`) so a future shape change
/// can migrate cleanly without clobbering older rows.
pub const SCHEDULE_CONFIG_KEY: &str = "scheduler.v1";

/// Persisted scheduler configuration. One row per install; the shape
/// is stable across upgrades thanks to the versioned
/// [`SCHEDULE_CONFIG_KEY`].
///
/// Serialised as a JSON blob in the `settings` table and surfaced
/// 1:1 to the frontend's Preferences dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduleConfig {
    /// Master switch. When `false`, the hourly timer is a no-op and
    /// no catch-up banners are emitted.
    pub enabled: bool,
    /// Days of the week the scheduler should try to produce a
    /// report for. A `Vec` (not a set) because `chrono::Weekday`
    /// does not implement `Ord`; insertion order is preserved so
    /// the JSON round-trip stays stable. Duplicates are tolerated
    /// (the planner treats the field as a set via `.contains`).
    #[ts(type = "string[]")]
    pub days_of_week: Vec<Weekday>,
    /// Wall-clock time (local) by which the final in-day tick
    /// should have run. After this time on a scheduled day the
    /// next action is a "final pass" on the following calendar
    /// day.
    #[ts(type = "string")]
    pub target_time: NaiveTime,
    /// Earliest wall-clock time (local) the scheduler is allowed to
    /// fire a first in-day tick. Hourly ticks before this are
    /// no-ops so users with late target times don't get a
    /// half-empty 4 AM report.
    #[ts(type = "string")]
    pub earliest_start: NaiveTime,
    /// How many days back the catch-up sweep looks. The planner
    /// clamps this to `CATCH_UP_DAYS_HARD_CAP` (30) regardless of
    /// what the config stores.
    pub catch_up_days: u32,
    /// UUID of the sink to write to. `None` means the scheduler is
    /// configured but has no destination yet — ticks become no-ops
    /// with a structured log entry.
    pub sink_id: Option<Uuid>,
    /// Template override; falls back to the app's default when
    /// `None`. v1 leaves this unused; reserved so a later release
    /// can ship per-schedule templates without a schema change.
    pub template_id: Option<String>,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            days_of_week: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
            target_time: NaiveTime::from_hms_opt(18, 0, 0).expect("18:00 is a valid NaiveTime"),
            earliest_start: NaiveTime::from_hms_opt(12, 0, 0).expect("12:00 is a valid NaiveTime"),
            catch_up_days: 7,
            sink_id: None,
            template_id: None,
        }
    }
}
