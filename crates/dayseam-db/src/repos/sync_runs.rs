//! Sync-run durability. Every orchestrator run gets exactly one row,
//! and this repo enforces the state machine:
//!
//!   Running → Completed
//!   Running → Cancelled
//!   Running → Failed
//!
//! Every other transition is rejected at the repo layer so a buggy
//! caller cannot silently corrupt the history. The `superseded_by`
//! column may only be set while marking a run `Cancelled` with
//! `SyncRunCancelReason::SupersededBy`.

use chrono::{DateTime, Utc};
use dayseam_core::{
    PerSourceState, RunId, SyncRun, SyncRunCancelReason, SyncRunStatus, SyncRunTrigger,
};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::{parse_rfc3339, sync_run_status_from_db, sync_run_status_to_db};

#[derive(Clone)]
pub struct SyncRunRepo {
    pool: SqlitePool,
}

impl SyncRunRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Inserts a freshly-minted run row. Callers always start runs in
    /// `Running`; the terminal transition goes through one of the
    /// `mark_*` methods below.
    pub async fn insert(&self, run: &SyncRun) -> DbResult<()> {
        if run.status != SyncRunStatus::Running {
            return Err(DbError::InvalidData {
                column: "sync_runs.status".into(),
                message: "new runs must start in status `Running`".into(),
            });
        }
        if run.finished_at.is_some() {
            return Err(DbError::InvalidData {
                column: "sync_runs.finished_at".into(),
                message: "new runs must have `finished_at == None`".into(),
            });
        }
        let trigger_json = serde_json::to_string(&run.trigger)?;
        let cancel_json = run
            .cancel_reason
            .map(|r| serde_json::to_string(&r))
            .transpose()?;
        let per_source_json = serde_json::to_string(&run.per_source_state)?;
        sqlx::query(
            "INSERT INTO sync_runs
                (id, started_at, finished_at, trigger_json, status,
                 cancel_reason_json, superseded_by, per_source_state_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(run.id.to_string())
        .bind(run.started_at.to_rfc3339())
        .bind(run.finished_at.map(|t| t.to_rfc3339()))
        .bind(trigger_json)
        .bind(sync_run_status_to_db(&run.status))
        .bind(cancel_json)
        .bind(run.superseded_by.map(|r| r.to_string()))
        .bind(per_source_json)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::classify_sqlx(e, "sync_runs.insert"))?;
        Ok(())
    }

    /// `Running → Completed` with final per-source state.
    pub async fn mark_finished(
        &self,
        id: &RunId,
        finished_at: DateTime<Utc>,
        per_source_state: &[PerSourceState],
    ) -> DbResult<()> {
        self.transition(
            id,
            SyncRunStatus::Completed,
            Some(finished_at),
            None,
            None,
            Some(per_source_state),
        )
        .await
    }

    /// `Running → Cancelled { reason }`. The repo enforces that
    /// `superseded_by` is `Some` iff `reason == SupersededBy`.
    pub async fn mark_cancelled(
        &self,
        id: &RunId,
        finished_at: DateTime<Utc>,
        reason: SyncRunCancelReason,
        per_source_state: &[PerSourceState],
    ) -> DbResult<()> {
        let superseded_by = match reason {
            SyncRunCancelReason::SupersededBy { run_id } => Some(run_id),
            _ => None,
        };
        self.transition(
            id,
            SyncRunStatus::Cancelled,
            Some(finished_at),
            Some(reason),
            superseded_by,
            Some(per_source_state),
        )
        .await
    }

    /// `Running → Failed` (partial per-source state is preserved so
    /// the UI can still surface which sources fetched what).
    pub async fn mark_failed(
        &self,
        id: &RunId,
        finished_at: DateTime<Utc>,
        per_source_state: &[PerSourceState],
    ) -> DbResult<()> {
        self.transition(
            id,
            SyncRunStatus::Failed,
            Some(finished_at),
            None,
            None,
            Some(per_source_state),
        )
        .await
    }

    async fn transition(
        &self,
        id: &RunId,
        to_status: SyncRunStatus,
        finished_at: Option<DateTime<Utc>>,
        cancel_reason: Option<SyncRunCancelReason>,
        superseded_by: Option<RunId>,
        per_source_state: Option<&[PerSourceState]>,
    ) -> DbResult<()> {
        let current = self.get(id).await?.ok_or_else(|| DbError::InvalidData {
            column: "sync_runs.id".into(),
            message: format!("no sync_runs row for id {id}"),
        })?;
        if current.status != SyncRunStatus::Running {
            return Err(DbError::InvalidData {
                column: "sync_runs.status".into(),
                message: format!(
                    "illegal transition from {:?} to {:?}",
                    current.status, to_status
                ),
            });
        }
        let cancel_json = cancel_reason
            .map(|r| serde_json::to_string(&r))
            .transpose()?;
        let per_source_json = match per_source_state {
            Some(state) => Some(serde_json::to_string(&state)?),
            None => None,
        };
        sqlx::query(
            "UPDATE sync_runs SET
                status = ?,
                finished_at = ?,
                cancel_reason_json = ?,
                superseded_by = ?,
                per_source_state_json = COALESCE(?, per_source_state_json)
             WHERE id = ?",
        )
        .bind(sync_run_status_to_db(&to_status))
        .bind(finished_at.map(|t| t.to_rfc3339()))
        .bind(cancel_json)
        .bind(superseded_by.map(|r| r.to_string()))
        .bind(per_source_json)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &RunId) -> DbResult<Option<SyncRun>> {
        let row = sqlx::query(
            "SELECT id, started_at, finished_at, trigger_json, status,
                    cancel_reason_json, superseded_by, per_source_state_json
             FROM sync_runs WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_sync_run).transpose()
    }

    pub async fn list_recent(&self, limit: i64) -> DbResult<Vec<SyncRun>> {
        let rows = sqlx::query(
            "SELECT id, started_at, finished_at, trigger_json, status,
                    cancel_reason_json, superseded_by, per_source_state_json
             FROM sync_runs
             ORDER BY started_at DESC
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_sync_run).collect()
    }
}

fn row_to_sync_run(row: sqlx::sqlite::SqliteRow) -> DbResult<SyncRun> {
    let id_str: String = row.try_get("id")?;
    let id = RunId(Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "sync_runs.id".into(),
        message: e.to_string(),
    })?);
    let started_at_str: String = row.try_get("started_at")?;
    let started_at = parse_rfc3339(&started_at_str, "sync_runs.started_at")?;
    let finished_at: Option<String> = row.try_get("finished_at")?;
    let finished_at = match finished_at {
        Some(s) => Some(parse_rfc3339(&s, "sync_runs.finished_at")?),
        None => None,
    };
    let trigger_json: String = row.try_get("trigger_json")?;
    let trigger: SyncRunTrigger = serde_json::from_str(&trigger_json)?;
    let status_str: String = row.try_get("status")?;
    let status = sync_run_status_from_db(&status_str)?;
    let cancel_reason_json: Option<String> = row.try_get("cancel_reason_json")?;
    let cancel_reason = match cancel_reason_json {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    };
    let superseded_by: Option<String> = row.try_get("superseded_by")?;
    let superseded_by = match superseded_by {
        Some(s) => Some(RunId(Uuid::parse_str(&s).map_err(|e| {
            DbError::InvalidData {
                column: "sync_runs.superseded_by".into(),
                message: e.to_string(),
            }
        })?)),
        None => None,
    };
    let per_source_json: String = row.try_get("per_source_state_json")?;
    let per_source_state: Vec<PerSourceState> = serde_json::from_str(&per_source_json)?;
    Ok(SyncRun {
        id,
        started_at,
        finished_at,
        trigger,
        status,
        cancel_reason,
        superseded_by,
        per_source_state,
    })
}
