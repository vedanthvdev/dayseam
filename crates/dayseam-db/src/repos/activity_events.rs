//! The main evidence table. Every bullet in every report ultimately
//! resolves to one or more rows here.
//!
//! The UNIQUE constraint on `(source_id, external_id, kind)` — combined
//! with deterministic UUIDv5 ids from `ActivityEvent::deterministic_id` —
//! means re-syncing the same upstream record two days in a row is a
//! no-op that returns a `Conflict`, which callers typically swallow with
//! an `INSERT OR IGNORE` semantic.

use chrono::NaiveDate;
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityRef, Link, Privacy, RawRef, SourceId,
};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::{
    activity_kind_from_db, activity_kind_to_db, parse_rfc3339, privacy_from_db, privacy_to_db,
};

#[derive(Clone)]
pub struct ActivityRepo {
    pool: SqlitePool,
}

impl ActivityRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a batch of events in a single transaction. Any duplicate
    /// (by the UNIQUE constraint) aborts the whole batch with
    /// `DbError::Conflict`. Callers that want "skip duplicates" semantics
    /// should partition first.
    pub async fn insert_many(&self, events: &[ActivityEvent]) -> DbResult<()> {
        if events.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for e in events {
            let kind = activity_kind_to_db(&e.kind);
            let actor_json = serde_json::to_string(&e.actor)?;
            let links_json = serde_json::to_string(&e.links)?;
            let entities_json = serde_json::to_string(&e.entities)?;
            let metadata_json = serde_json::to_string(&e.metadata)?;
            let raw_ref_json = serde_json::to_string(&e.raw_ref)?;
            let privacy = privacy_to_db(&e.privacy);

            sqlx::query(
                "INSERT INTO activity_events
                    (id, source_id, external_id, kind, occurred_at, actor_json, title, body,
                     links_json, entities_json, parent_external_id, metadata_json, raw_ref,
                     privacy)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(e.id.to_string())
            .bind(e.source_id.to_string())
            .bind(&e.external_id)
            .bind(kind)
            .bind(e.occurred_at.to_rfc3339())
            .bind(actor_json)
            .bind(&e.title)
            .bind(e.body.as_deref())
            .bind(links_json)
            .bind(entities_json)
            .bind(e.parent_external_id.as_deref())
            .bind(metadata_json)
            .bind(raw_ref_json)
            .bind(privacy)
            .execute(&mut *tx)
            .await
            .map_err(|err| DbError::classify_sqlx(err, "activity_events.insert_many"))?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// List every event for a given source whose UTC date component
    /// matches `date`. The caller is responsible for translating the
    /// user's local date into the UTC window it wants to cover.
    pub async fn list_by_source_date(
        &self,
        source_id: &SourceId,
        date: NaiveDate,
    ) -> DbResult<Vec<ActivityEvent>> {
        let date_str = date.format("%Y-%m-%d").to_string();
        let rows = sqlx::query(
            "SELECT id, source_id, external_id, kind, occurred_at, actor_json, title, body,
                    links_json, entities_json, parent_external_id, metadata_json, raw_ref, privacy
             FROM activity_events
             WHERE source_id = ? AND substr(occurred_at, 1, 10) = ?
             ORDER BY occurred_at ASC",
        )
        .bind(source_id.to_string())
        .bind(date_str)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_event).collect()
    }

    pub async fn delete_for_source(&self, source_id: &SourceId) -> DbResult<()> {
        sqlx::query("DELETE FROM activity_events WHERE source_id = ?")
            .bind(source_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_event(row: sqlx::sqlite::SqliteRow) -> DbResult<ActivityEvent> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "activity_events.id".into(),
        message: e.to_string(),
    })?;
    let source_id_str: String = row.try_get("source_id")?;
    let source_id = Uuid::parse_str(&source_id_str).map_err(|e| DbError::InvalidData {
        column: "activity_events.source_id".into(),
        message: e.to_string(),
    })?;
    let kind_str: String = row.try_get("kind")?;
    let kind: ActivityKind = activity_kind_from_db(&kind_str)?;
    let occurred_str: String = row.try_get("occurred_at")?;
    let occurred_at = parse_rfc3339(&occurred_str, "activity_events.occurred_at")?;
    let actor_json: String = row.try_get("actor_json")?;
    let actor: Actor = serde_json::from_str(&actor_json)?;
    let links_json: String = row.try_get("links_json")?;
    let links: Vec<Link> = serde_json::from_str(&links_json)?;
    let entities_json: String = row.try_get("entities_json")?;
    let entities: Vec<EntityRef> = serde_json::from_str(&entities_json)?;
    let metadata_json: String = row.try_get("metadata_json")?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_json)?;
    let raw_ref_json: String = row.try_get("raw_ref")?;
    let raw_ref: RawRef = serde_json::from_str(&raw_ref_json)?;
    let privacy_str: String = row.try_get("privacy")?;
    let privacy: Privacy = privacy_from_db(&privacy_str)?;

    Ok(ActivityEvent {
        id,
        source_id,
        external_id: row.try_get("external_id")?,
        kind,
        occurred_at,
        actor,
        title: row.try_get("title")?,
        body: row.try_get("body")?,
        links,
        entities,
        parent_external_id: row.try_get("parent_external_id")?,
        metadata,
        raw_ref,
        privacy,
    })
}
