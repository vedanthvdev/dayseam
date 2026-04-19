//! [`SyncRun`] — the orchestrator-level record of a single `generate`
//! invocation. Distinct from [`super::report::SourceRunState`], which
//! captures *one source's* contribution to a rendered
//! [`super::report::ReportDraft`]. A `SyncRun` fans out to many sources
//! and owns the cross-source lifecycle: supersede-on-retry,
//! cancellation, crash-recovery on the next startup.
//!
//! The `run_id` threaded through every Phase-1 `ProgressEvent` /
//! `LogEvent` is this row's primary key; a run in the DB and a run on
//! the per-run IPC streams are the same run seen through two different
//! lenses.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::DayseamError;

use super::events::RunId;
use super::report::RunStatus;
use super::source::SourceId;

/// One orchestrator run. Persisted to `sync_runs` so the next startup
/// can recover from a crash: any row found with
/// `status == Running && finished_at IS NULL` is swept to [`SyncRunStatus::Failed`]
/// with a stable error code before the UI ever sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SyncRun {
    pub id: RunId,
    pub started_at: DateTime<Utc>,
    /// Populated exactly when `status` transitions out of
    /// [`SyncRunStatus::Running`]. Enforced by
    /// [`SyncRunRepo::mark_finished`](../../../../dayseam-db/src/repos/sync_runs.rs)
    /// and friends.
    pub finished_at: Option<DateTime<Utc>>,
    pub trigger: SyncRunTrigger,
    pub status: SyncRunStatus,
    /// Always `Some` when `status == Cancelled`; always `None` otherwise.
    /// The repo layer rejects any mix that violates this.
    pub cancel_reason: Option<SyncRunCancelReason>,
    /// Set exactly when `cancel_reason == SupersededBy(_)`. Stored as a
    /// separate column so the orchestrator can join newer → older rows
    /// without deserialising `cancel_reason`.
    pub superseded_by: Option<RunId>,
    /// Per-source view of this run. Serialised as one JSON array on
    /// disk (see `sync_runs.per_source_state_json`); kept here as a
    /// `Vec` rather than a map so `ts-rs` renders a list in the
    /// frontend and the ordering is deterministic.
    pub per_source_state: Vec<PerSourceState>,
}

/// What started this run. Phase 2 only ever emits
/// [`SyncRunTrigger::User`] and [`SyncRunTrigger::Retry`]; scheduler and
/// startup triggers land when the scheduler ships (v0.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum SyncRunTrigger {
    /// User clicked Generate. The common case in Phase 2.
    User,
    /// User retried a previous run from the UI. Carries the prior
    /// `run_id` so the orchestrator can link the two for audit.
    Retry { previous_run_id: RunId },
}

/// Terminal lifecycle status for a [`SyncRun`]. The state machine
/// (enforced at the repo layer — see
/// [invariant 4 of the Phase 2 plan](../../../../docs/plan/2026-04-18-v0.1-phase-2-local-git.md))
/// only allows:
///
/// - `Running → Completed`
/// - `Running → Cancelled`
/// - `Running → Failed`
///
/// Any other transition is a repo-level rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SyncRunStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// Why a run was cancelled. Stored alongside the `Cancelled` status so
/// the UI can say "we cancelled your run because you hit Cancel" vs
/// "…because you clicked Generate again".
///
/// The two variants map 1:1 to error codes `RUN_CANCELLED_BY_USER`
/// and `RUN_CANCELLED_BY_SUPERSEDED` from [`crate::error_codes`]. A
/// `Shutdown` variant existed in Phase 1 in anticipation of a
/// graceful-shutdown flow, but Phase 2 Task 8 removed it after the
/// cross-cutting review (LCY-01) confirmed no orchestrator code path
/// ever produces the value and no persisted rows carried it. A
/// future graceful-shutdown implementation can re-introduce a
/// dedicated variant at that time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum SyncRunCancelReason {
    User,
    /// Cancelled because a newer run for the same
    /// `(person_id, date, template_id)` tuple superseded it.
    SupersededBy {
        run_id: RunId,
    },
}

/// One source's view of a [`SyncRun`]. Shape mirrors
/// [`super::report::SourceRunState`] on purpose — same fields, carried
/// by a `Vec` at the run level and a `HashMap<SourceId, _>` at the
/// report level. Keeping them as two types means the orchestrator (pre-
/// render) and the report engine (post-render) can evolve
/// independently; the values themselves agree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PerSourceState {
    pub source_id: SourceId,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Number of events the connector persisted for this source in
    /// this run. Used by the UI to show "12 new events" as the run
    /// terminates, and by the retention sweep to decide when an
    /// empty day is still a completed day.
    pub fetched_count: u32,
    pub error: Option<DayseamError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_run_trigger_serializes_tagged() {
        let user = SyncRunTrigger::User;
        let json = serde_json::to_string(&user).unwrap();
        assert_eq!(json, r#"{"kind":"user"}"#);

        let prev = RunId::new();
        let retry = SyncRunTrigger::Retry {
            previous_run_id: prev,
        };
        let json = serde_json::to_string(&retry).unwrap();
        assert!(json.contains(r#""kind":"retry""#));
        assert!(json.contains(&prev.to_string()));
    }

    #[test]
    fn sync_run_cancel_reason_serializes_tagged() {
        let run = RunId::new();
        let reason = SyncRunCancelReason::SupersededBy { run_id: run };
        let json = serde_json::to_string(&reason).unwrap();
        assert!(json.contains(r#""kind":"superseded_by""#));
        assert!(json.contains(&run.to_string()));
    }
}
