//! Live event types that cross the IPC boundary during a sync run.
//!
//! These are the payload shapes the Rust core writes onto per-run typed
//! streams (`ProgressEvent`, `LogEvent`) and app-wide broadcast channels
//! (`ToastEvent`). The bus machinery that delivers them lives in
//! `dayseam-events`; the types live here alongside every other IPC type
//! so `ts-rs` can generate their TypeScript equivalents in a single
//! pass.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::report::LogLevel;
use super::source::SourceId;

/// Identifier for a single synchronisation run. Threaded through every
/// per-run stream so stale events from a superseded run can never paint
/// over a newer run's UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RunId(pub Uuid);

impl RunId {
    /// Fresh random run id. Callers should generate one per `SyncRun`
    /// and pass it to every connector for the duration of that run.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Inner UUID, exposed for persistence and logging.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// One step of a run's progress as seen by the UI. Emitted on a per-run
/// ordered stream; the Tauri layer forwards these to the frontend as
/// they arrive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProgressEvent {
    pub run_id: RunId,
    /// `None` when the event is about the run as a whole (e.g. "all
    /// sources finished"). `Some` when it refers to one source's share
    /// of the work.
    pub source_id: Option<SourceId>,
    pub phase: ProgressPhase,
    pub emitted_at: DateTime<Utc>,
}

/// Where a progress-reporting unit of work currently sits. `InProgress`
/// carries an optional total so the UI can render a determinate progress
/// bar when the total is known and an indeterminate spinner otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export)]
pub enum ProgressPhase {
    Starting {
        message: String,
    },
    InProgress {
        completed: u32,
        /// `None` means "we don't know the total yet"; the UI renders
        /// an indeterminate spinner in that case.
        total: Option<u32>,
        message: String,
    },
    Completed {
        message: String,
    },
    Failed {
        /// Stable machine-readable error code (see `error_codes`).
        code: String,
        message: String,
    },
}

/// One structured log line, streamed live during a run and also
/// persisted as a `log_entries` row for post-hoc debugging. The bus is
/// for UX; the table is for "why did Tuesday's report miss those
/// commits?" three days later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogEvent {
    /// Optional because startup / shutdown log lines aren't tied to a
    /// specific run.
    pub run_id: Option<RunId>,
    pub source_id: Option<SourceId>,
    pub level: LogLevel,
    pub message: String,
    /// Free-form structured context. Serialised as a JSON object in
    /// practice but typed as `JsonValue` so consumers stay flexible.
    pub context: serde_json::Value,
    pub emitted_at: DateTime<Utc>,
}

/// App-wide toast banner shown in the corner of the window. Published
/// on a broadcast channel in `dayseam-events::AppBus` and forwarded to
/// every window via `tauri::Manager::emit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToastEvent {
    /// Unique per toast so the UI can deduplicate rapid fire-and-forget
    /// publishes.
    pub id: Uuid,
    pub severity: ToastSeverity,
    pub title: String,
    pub body: Option<String>,
    pub emitted_at: DateTime<Utc>,
}

/// Severity of a toast. Drives colour and icon in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToastSeverity {
    Info,
    Success,
    Warning,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_new_produces_distinct_values() {
        let a = RunId::new();
        let b = RunId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn run_id_default_is_new() {
        let a = RunId::default();
        let b = RunId::default();
        assert_ne!(a, b);
    }

    #[test]
    fn run_id_display_matches_uuid() {
        let inner = Uuid::new_v4();
        let rid = RunId(inner);
        assert_eq!(rid.to_string(), inner.to_string());
    }
}
