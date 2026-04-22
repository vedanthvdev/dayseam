//! The output side of Dayseam — a `ReportDraft` is what the user sees,
//! edits, and ultimately writes to a markdown sink. It carries enough
//! evidence to explain every bullet and enough per-source state that a
//! failed sync can be surfaced rather than silently dropped.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::error::DayseamError;

use super::source::SourceId;

/// One rendered report for a specific date.
///
/// DAY-100 TST-v0.3-01: carries `#[derive(SerdeDefaultAudit)]` so the
/// next author to add a `#[serde(default)]` field (e.g. a
/// `draft_version: u32` with a back-compat default for drafts written
/// before the field existed) is forced to pair it with a
/// `#[serde_default_audit(...)]` annotation. Closes the DOG-v0.2-04
/// silent-failure avenue on the draft-deserialisation layer — where
/// a defaulted field would be especially painful because drafts
/// survive across Dayseam upgrades.
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, TS, dayseam_macros::SerdeDefaultAudit,
)]
#[ts(export)]
pub struct ReportDraft {
    pub id: Uuid,
    pub date: NaiveDate,
    pub template_id: String,
    pub template_version: String,
    pub sections: Vec<RenderedSection>,
    pub evidence: Vec<Evidence>,
    pub per_source_state: HashMap<SourceId, SourceRunState>,
    pub verbose_mode: bool,
    pub generated_at: DateTime<Utc>,
}

/// A named section of the report ("Completed", "In progress", ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RenderedSection {
    /// Stable identifier used by evidence links; must be unique within a
    /// `ReportDraft`.
    pub id: String,
    pub title: String,
    pub bullets: Vec<RenderedBullet>,
}

/// A single rendered bullet, carrying a stable id so evidence links never
/// break when the user re-orders or edits prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RenderedBullet {
    pub id: String,
    pub text: String,
}

/// Traceability edge from a rendered bullet back to the `ActivityEvent`s
/// that caused it to exist. The UI surfaces these when the user clicks a
/// bullet to ask "where did this come from?".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Evidence {
    pub bullet_id: String,
    pub event_ids: Vec<Uuid>,
    /// Short human-readable explanation, e.g. "1 commit + linked !1234".
    pub reason: String,
}

/// Per-source run state captured during report generation. Kept alongside
/// the rendered output so a failed sync never turns into a silently
/// incomplete report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceRunState {
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub fetched_count: usize,
    pub error: Option<DayseamError>,
}

/// Terminal status of a single source's contribution to a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RunStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

/// One line in the in-app log drawer. Retained for troubleshooting;
/// severity drives UI colour and whether the entry is shown by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub source_id: Option<SourceId>,
    pub message: String,
}

/// Severity of a log entry surfaced in the desktop log drawer.
///
/// The ordering `Debug` → `Info` → `Warn` → `Error` is load-bearing:
/// the frontend filter dropdown maps severities to "this level and
/// above", so any reordering here must be mirrored in
/// `apps/desktop/src/ipc/useLogs.ts`. Variants are rendered verbatim
/// in the UI (no localisation yet), so renaming them is a breaking
/// change to both the IPC contract and the user-facing copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum LogLevel {
    /// Verbose diagnostic detail. Off by default in the UI filter,
    /// intended for development builds and bug reports.
    Debug,
    /// Routine progress: run started, source completed, report
    /// rendered. Always visible in the log drawer.
    Info,
    /// Recoverable problems: a single source failed but the run
    /// continued, a rate-limit backoff was triggered, etc.
    Warn,
    /// Run-terminating or data-loss-class problems the user must see.
    /// Pair with a toast when surfaced during an active run.
    Error,
}
