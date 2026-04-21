//! Canonical-artefact storage. One row per `(source_id, kind, external_id)`
//! triple; the external id typically encodes `(repo_path, day)` for
//! `CommitSet` or the upstream iid / id for MRs / issues / threads.
//!
//! `upsert` is the only mutation downstream code needs: connectors call
//! it idempotently per sync, which makes retries a no-op at the DB
//! layer because `ArtifactId::deterministic` reuses the same primary
//! key for the same inputs.

use chrono::NaiveDate;
use dayseam_core::{Artifact, ArtifactId, ArtifactPayload, SourceId};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::{artifact_kind_from_db, artifact_kind_to_db, parse_rfc3339};

#[derive(Clone)]
pub struct ArtifactRepo {
    pool: SqlitePool,
}

impl ArtifactRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert the artefact, or replace the existing row for the same
    /// `(source_id, kind, external_id)` with a fresh payload and
    /// `created_at`. Connectors re-run the same sync after fixing a
    /// bug and rely on this being a no-op-or-overwrite, never a
    /// conflict.
    pub async fn upsert(&self, artifact: &Artifact) -> DbResult<()> {
        let kind = artifact_kind_to_db(&artifact.kind);
        let payload = serde_json::to_string(&artifact.payload)?;
        sqlx::query(
            "INSERT INTO artifacts (id, source_id, kind, external_id, payload_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(source_id, kind, external_id) DO UPDATE SET
                id = excluded.id,
                payload_json = excluded.payload_json,
                created_at = excluded.created_at",
        )
        .bind(artifact.id.to_string())
        .bind(artifact.source_id.to_string())
        .bind(kind)
        .bind(&artifact.external_id)
        .bind(payload)
        .bind(artifact.created_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::classify_sqlx(e, "artifacts.upsert"))?;
        Ok(())
    }

    pub async fn get(&self, id: &ArtifactId) -> DbResult<Option<Artifact>> {
        let row = sqlx::query(
            "SELECT id, source_id, kind, external_id, payload_json, created_at
             FROM artifacts WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_artifact).transpose()
    }

    /// All artefacts for `source_id` whose `CommitSet::date` equals
    /// `date`. For kinds without a date (Phase 3+) callers must extend
    /// this with a kind-aware helper; Phase 2 only ships `CommitSet`
    /// so the filter is enough.
    pub async fn list_for_source_date(
        &self,
        source_id: &SourceId,
        date: NaiveDate,
    ) -> DbResult<Vec<Artifact>> {
        let rows = sqlx::query(
            "SELECT id, source_id, kind, external_id, payload_json, created_at
             FROM artifacts
             WHERE source_id = ?
             ORDER BY external_id ASC",
        )
        .bind(source_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let artifact = row_to_artifact(row)?;
            if artifact_matches_date(&artifact, date) {
                out.push(artifact);
            }
        }
        Ok(out)
    }
}

fn artifact_matches_date(artifact: &Artifact, target: NaiveDate) -> bool {
    // Every artefact payload carries a `date` the same way; the
    // or-pattern keeps the match exhaustive without duplicating the
    // comparison. DAY-73 added the Jira / Confluence variants.
    match &artifact.payload {
        ArtifactPayload::CommitSet { date, .. }
        | ArtifactPayload::JiraIssue { date, .. }
        | ArtifactPayload::ConfluencePage { date, .. } => *date == target,
    }
}

fn row_to_artifact(row: sqlx::sqlite::SqliteRow) -> DbResult<Artifact> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "artifacts.id".into(),
        message: e.to_string(),
    })?;
    let source_id_str: String = row.try_get("source_id")?;
    let source_id = Uuid::parse_str(&source_id_str).map_err(|e| DbError::InvalidData {
        column: "artifacts.source_id".into(),
        message: e.to_string(),
    })?;
    let kind_str: String = row.try_get("kind")?;
    let kind = artifact_kind_from_db(&kind_str)?;
    let external_id: String = row.try_get("external_id")?;
    let payload_json: String = row.try_get("payload_json")?;
    let payload: ArtifactPayload = serde_json::from_str(&payload_json)?;
    let created_at_str: String = row.try_get("created_at")?;
    let created_at = parse_rfc3339(&created_at_str, "artifacts.created_at")?;
    Ok(Artifact {
        id: ArtifactId(id),
        source_id,
        kind,
        external_id,
        payload,
        created_at,
    })
}
