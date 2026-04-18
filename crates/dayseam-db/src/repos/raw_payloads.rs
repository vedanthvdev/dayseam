//! Raw upstream payloads, kept for replay and debugging only. The report
//! engine never reads from this table — if a retention sweep blows it
//! away, reports are unaffected.

use chrono::{DateTime, Utc};
use dayseam_core::SourceId;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::parse_rfc3339;

/// One stored raw payload. Not part of `dayseam-core` because it is a
/// storage-layer concern — the normalised `ActivityEvent` is what the
/// rest of the system consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPayload {
    pub id: Uuid,
    pub source_id: SourceId,
    pub endpoint: String,
    pub fetched_at: DateTime<Utc>,
    /// The verbatim upstream JSON blob. Opaque to the DB layer.
    pub payload_json: String,
    /// SHA-256 of `payload_json` (hex). Used to deduplicate identical
    /// payloads across polls without parsing them.
    pub payload_sha256: String,
}

#[derive(Clone)]
pub struct RawPayloadRepo {
    pool: SqlitePool,
}

impl RawPayloadRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, rp: &RawPayload) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO raw_payloads (id, source_id, endpoint, fetched_at, payload_json, payload_sha256)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(rp.id.to_string())
        .bind(rp.source_id.to_string())
        .bind(&rp.endpoint)
        .bind(rp.fetched_at.to_rfc3339())
        .bind(&rp.payload_json)
        .bind(&rp.payload_sha256)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::classify_sqlx(e, "raw_payloads.insert"))?;
        Ok(())
    }

    pub async fn get(&self, id: &Uuid) -> DbResult<Option<RawPayload>> {
        let row = sqlx::query(
            "SELECT id, source_id, endpoint, fetched_at, payload_json, payload_sha256
             FROM raw_payloads WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_raw_payload).transpose()
    }

    /// Prune every payload fetched strictly before `cutoff`. Returns the
    /// number of rows removed so startup sweeps can log it.
    pub async fn prune_older_than(&self, cutoff: DateTime<Utc>) -> DbResult<u64> {
        let res = sqlx::query("DELETE FROM raw_payloads WHERE fetched_at < ?")
            .bind(cutoff.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

fn row_to_raw_payload(row: sqlx::sqlite::SqliteRow) -> DbResult<RawPayload> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "raw_payloads.id".into(),
        message: e.to_string(),
    })?;
    let src_str: String = row.try_get("source_id")?;
    let source_id = Uuid::parse_str(&src_str).map_err(|e| DbError::InvalidData {
        column: "raw_payloads.source_id".into(),
        message: e.to_string(),
    })?;
    let fetched_str: String = row.try_get("fetched_at")?;
    let fetched_at = parse_rfc3339(&fetched_str, "raw_payloads.fetched_at")?;
    Ok(RawPayload {
        id,
        source_id,
        endpoint: row.try_get("endpoint")?,
        fetched_at,
        payload_json: row.try_get("payload_json")?,
        payload_sha256: row.try_get("payload_sha256")?,
    })
}
