//! `dayseam-orchestrator` — the coordinator that threads source
//! connectors, the report engine, and sinks into a single `generate`
//! lifecycle on behalf of the Tauri layer.
//!
//! Everything the Tauri layer needs to produce a report for a day
//! lives behind the [`Orchestrator`] handle:
//!
//! * [`Orchestrator::generate_report`] — fan out over every requested
//!   source, collect events and artifacts, render the draft through
//!   `dayseam-report`, persist the row, and stream progress onto the
//!   per-run [`dayseam_events::RunStreams`].
//! * [`Orchestrator::cancel`] — propagate a cancel signal into every
//!   live connector for a run and emit a terminal
//!   [`dayseam_core::ProgressPhase::Cancelled`].
//!
//! The crate is deliberately Tauri-agnostic: it depends on
//! `dayseam-db`, `dayseam-events`, `connectors-sdk`, `sinks-sdk`, and
//! the shipping connector / sink crates, but nothing in it is bound to
//! a Tauri `AppHandle`. That is enforced by
//! `tests/no_cross_crate_leak.rs` so a future CLI can drive the same
//! orchestrator without code-level re-export gymnastics.
//!
//! ### Invariants proven by tests (PR-A scope)
//!
//! 1. **Fan-out.** `generate_report` runs every requested source
//!    concurrently with bounded parallelism `min(sources.len(), 4)`;
//!    every source's progress stream is stamped with the same
//!    [`dayseam_core::RunId`]. See `tests/generate.rs`.
//! 2. **Supersede-on-retry.** A second `generate_report` for the same
//!    `(person_id, date, template_id)` tuple cancels the older run,
//!    transitions its `SyncRun` row to `Cancelled` with
//!    [`dayseam_core::SyncRunCancelReason::SupersededBy`], and drops the
//!    older run's results at persist time so a late writer cannot
//!    overwrite the newer draft. See `tests/supersede.rs`.
//! 3. **Cancellation propagates.** [`Orchestrator::cancel`] fires
//!    `ctx.cancel.cancel()` for every live connector in the run,
//!    transitions the `SyncRun` row to `Cancelled` with
//!    [`dayseam_core::SyncRunCancelReason::User`], and emits a
//!    terminal [`dayseam_core::ProgressPhase::Cancelled`] on the
//!    per-run stream. See `tests/cancel.rs`.
//! 4. **Partial failure surfaces, does not abort.** One source
//!    returning `Err(DayseamError)` does not abort the other sources:
//!    the orchestrator persists the failure in the failing source's
//!    `per_source_state` row and renders the draft from the healthy
//!    sources. See `tests/generate.rs`.
//! 5. **`SyncRun` transitions are durable.** Every state change
//!    (`Running → Completed|Cancelled|Failed`) is persisted through
//!    [`dayseam_db::SyncRunRepo`] *before* the next transition can
//!    occur. Crash recovery (the "sweep Running rows at startup" half
//!    of this invariant) lands in PR-B. See `tests/generate.rs`.
//!
//! ### Scope deferred to PR-B
//!
//! * `save_report` lifecycle (invariant #7).
//! * Retention sweep (invariant #6).
//! * Crash-recovery sweep on startup (invariant #5 second half).
//! * `AppState.orchestrator` wiring (step 5.7).

pub mod generate;
pub mod orchestrator;
pub mod registries;
pub mod retention;
pub mod save;
pub mod schedule;
pub mod startup;

pub use orchestrator::{
    GenerateOutcome, GenerateRequest, Orchestrator, OrchestratorBuilder, SourceHandle,
};
pub use registries::{default_registries, ConnectorRegistry, DefaultRegistryConfig, SinkRegistry};
pub use retention::{
    resolve_cutoff, sweep as retention_sweep, sweep_with_resolved_cutoff, RetentionSchedule,
    SweepReport, DEFAULT_RETENTION_DAYS, POST_RUN_SWEEP_MIN_INTERVAL, RETENTION_DAYS_SETTING_KEY,
};
pub use schedule::{
    plan_next_actions, run_scheduled_action, SatisfactionKind, ScheduleRunError, ScheduleState,
    ScheduledAction, SchedulerRunRow, CATCH_UP_DAYS_HARD_CAP, SCHEDULE_CONFIG_KEY,
};
pub use startup::StartupReport;
