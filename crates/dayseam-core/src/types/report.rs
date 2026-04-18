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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}
