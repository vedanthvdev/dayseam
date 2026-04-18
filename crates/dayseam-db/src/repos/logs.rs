//! The in-app log drawer. Every structured log event that eventually
//! shows up in the UI lands here first. Retention is short (see design
//! §5.4) because this table is for humans, not auditors.

use chrono::{DateTime, Utc};
use dayseam_core::{LogLevel, SourceId};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::{log_level_from_db, log_level_to_db, parse_rfc3339};

/// Storage-shape analogue of `dayseam_core::LogEntry`, with the `source`
/// column exposed so callers can stash either a `SourceId` or the
/// free-form "system" literal in one row. The stored layout is:
///
/// * `source_id = None`        → `source = "system"`
/// * `source_id = Some(uuid)`  → `source = uuid.to_string()`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub ts: DateTime<Utc>,
    pub level: LogLevel,
    pub source_id: Option<SourceId>,
    pub message: String,
    pub context: Option<serde_json::Value>,
}

const SYSTEM_SOURCE: &str = "system";

#[derive(Clone)]
pub struct LogRepo {
    pool: SqlitePool,
}

impl LogRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn append(&self, row: &LogRow) -> DbResult<()> {
        let level = log_level_to_db(&row.level);
        let source = match row.source_id {
            Some(id) => id.to_string(),
            None => SYSTEM_SOURCE.to_string(),
        };
        let context_json = match &row.context {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        sqlx::query(
            "INSERT INTO log_entries (ts, level, source, message, context_json)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(row.ts.to_rfc3339())
        .bind(level)
        .bind(source)
        .bind(&row.message)
        .bind(context_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return up to `limit` rows with `ts >= since`, newest first. Use
    /// `since = DateTime::<Utc>::MIN_UTC` to read every retained row.
    ///
    /// The "newest first" ordering is the one the log drawer actually
    /// needs: once the table has a few thousand rows, sorting ASC and
    /// applying `LIMIT` would hand the UI the oldest N rows — the
    /// opposite of a tail.
    pub async fn tail(&self, since: DateTime<Utc>, limit: u32) -> DbResult<Vec<LogRow>> {
        let rows = sqlx::query(
            "SELECT ts, level, source, message, context_json
             FROM log_entries
             WHERE ts >= ?
             ORDER BY ts DESC
             LIMIT ?",
        )
        .bind(since.to_rfc3339())
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_log).collect()
    }

    pub async fn prune_older_than(&self, cutoff: DateTime<Utc>) -> DbResult<u64> {
        let res = sqlx::query("DELETE FROM log_entries WHERE ts < ?")
            .bind(cutoff.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

fn row_to_log(row: sqlx::sqlite::SqliteRow) -> DbResult<LogRow> {
    let ts_str: String = row.try_get("ts")?;
    let ts = parse_rfc3339(&ts_str, "log_entries.ts")?;
    let level_str: String = row.try_get("level")?;
    let level = log_level_from_db(&level_str)?;
    let source: String = row.try_get("source")?;
    let source_id = if source == SYSTEM_SOURCE {
        None
    } else {
        Some(Uuid::parse_str(&source).map_err(|e| DbError::InvalidData {
            column: "log_entries.source".into(),
            message: e.to_string(),
        })?)
    };
    let context_json: Option<String> = row.try_get("context_json")?;
    let context = match context_json {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    };
    Ok(LogRow {
        ts,
        level,
        source_id,
        message: row.try_get("message")?,
        context,
    })
}
