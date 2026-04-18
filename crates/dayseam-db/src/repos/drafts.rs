//! Report drafts. Each row is one rendered report for one date. Drafts
//! are append-only; regenerating a report creates a new row so the user
//! always has history, and retention sweeps prune the oldest rows.

use chrono::{DateTime, Utc};
use dayseam_core::ReportDraft;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::parse_rfc3339;

#[derive(Clone)]
pub struct DraftRepo {
    pool: SqlitePool,
}

impl DraftRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, draft: &ReportDraft) -> DbResult<()> {
        let sections = serde_json::to_string(&draft.sections)?;
        let evidence = serde_json::to_string(&draft.evidence)?;
        let per_source_state = serde_json::to_string(&draft.per_source_state)?;
        let verbose = if draft.verbose_mode { 1_i64 } else { 0_i64 };
        sqlx::query(
            "INSERT INTO report_drafts
                (id, date, template_id, template_version, sections_json, evidence_json,
                 per_source_state_json, verbose_mode, generated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(draft.id.to_string())
        .bind(draft.date.format("%Y-%m-%d").to_string())
        .bind(&draft.template_id)
        .bind(&draft.template_version)
        .bind(sections)
        .bind(evidence)
        .bind(per_source_state)
        .bind(verbose)
        .bind(draft.generated_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::classify_sqlx(e, "report_drafts.insert"))?;
        Ok(())
    }

    pub async fn get(&self, id: &Uuid) -> DbResult<Option<ReportDraft>> {
        let row = sqlx::query(
            "SELECT id, date, template_id, template_version, sections_json, evidence_json,
                    per_source_state_json, verbose_mode, generated_at
             FROM report_drafts WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_draft).transpose()
    }

    pub async fn list_recent(&self, limit: u32) -> DbResult<Vec<ReportDraft>> {
        let rows = sqlx::query(
            "SELECT id, date, template_id, template_version, sections_json, evidence_json,
                    per_source_state_json, verbose_mode, generated_at
             FROM report_drafts
             ORDER BY generated_at DESC
             LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_draft).collect()
    }

    pub async fn prune_older_than(&self, cutoff: DateTime<Utc>) -> DbResult<u64> {
        let res = sqlx::query("DELETE FROM report_drafts WHERE generated_at < ?")
            .bind(cutoff.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

fn row_to_draft(row: sqlx::sqlite::SqliteRow) -> DbResult<ReportDraft> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "report_drafts.id".into(),
        message: e.to_string(),
    })?;
    let date_str: String = row.try_get("date")?;
    let date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
        DbError::InvalidData {
            column: "report_drafts.date".into(),
            message: e.to_string(),
        }
    })?;
    let sections_json: String = row.try_get("sections_json")?;
    let sections = serde_json::from_str(&sections_json)?;
    let evidence_json: String = row.try_get("evidence_json")?;
    let evidence = serde_json::from_str(&evidence_json)?;
    let per_src_json: String = row.try_get("per_source_state_json")?;
    let per_source_state = serde_json::from_str(&per_src_json)?;
    let verbose_int: i64 = row.try_get("verbose_mode")?;
    let generated_str: String = row.try_get("generated_at")?;
    let generated_at = parse_rfc3339(&generated_str, "report_drafts.generated_at")?;
    Ok(ReportDraft {
        id,
        date,
        template_id: row.try_get("template_id")?,
        template_version: row.try_get("template_version")?,
        sections,
        evidence,
        per_source_state,
        verbose_mode: verbose_int != 0,
        generated_at,
    })
}
