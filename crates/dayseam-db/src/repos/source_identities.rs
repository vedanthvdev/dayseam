//! Per-source actor → `Person` mappings. The authorship filter a
//! connector applies at normalisation time is "does this event's
//! actor match any `SourceIdentity` row whose `person_id == self` and
//! `source_id == ctx.source_id` (or `NULL` for source-agnostic
//! identities)?" Every helper on this repo exists to answer that
//! question efficiently.

use dayseam_core::{SourceId, SourceIdentity, SourceIdentityKind};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::{source_identity_kind_from_db, source_identity_kind_to_db};

#[derive(Clone)]
pub struct SourceIdentityRepo {
    pool: SqlitePool,
}

impl SourceIdentityRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, identity: &SourceIdentity) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO source_identities
                (id, person_id, source_id, kind, external_actor_id)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(identity.id.to_string())
        .bind(identity.person_id.to_string())
        .bind(identity.source_id.map(|s| s.to_string()))
        .bind(source_identity_kind_to_db(&identity.kind))
        .bind(&identity.external_actor_id)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::classify_sqlx(e, "source_identities.insert"))?;
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> DbResult<()> {
        sqlx::query("DELETE FROM source_identities WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_for_person(&self, person_id: Uuid) -> DbResult<Vec<SourceIdentity>> {
        let rows = sqlx::query(
            "SELECT id, person_id, source_id, kind, external_actor_id
             FROM source_identities
             WHERE person_id = ?
             ORDER BY kind ASC, external_actor_id ASC",
        )
        .bind(person_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_source_identity).collect()
    }

    /// Every identity relevant to events normalised for `source_id` —
    /// that's both the source-scoped rows for this source and every
    /// source-agnostic row (`source_id IS NULL`, used for bare git
    /// emails that match regardless of which repo is configured).
    pub async fn list_for_source(
        &self,
        person_id: Uuid,
        source_id: &SourceId,
    ) -> DbResult<Vec<SourceIdentity>> {
        let rows = sqlx::query(
            "SELECT id, person_id, source_id, kind, external_actor_id
             FROM source_identities
             WHERE person_id = ?
               AND (source_id = ? OR source_id IS NULL)
             ORDER BY kind ASC, external_actor_id ASC",
        )
        .bind(person_id.to_string())
        .bind(source_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_source_identity).collect()
    }

    /// Resolve a concrete `(kind, external_actor_id)` observed on a
    /// source back to the `person_id` it belongs to. Returns `None`
    /// if the actor is unknown — callers treat that as "not me".
    pub async fn resolve_person_id(
        &self,
        source_id: Option<&SourceId>,
        kind: SourceIdentityKind,
        external_actor_id: &str,
    ) -> DbResult<Option<Uuid>> {
        let source_bind = source_id.map(|s| s.to_string());
        let row = sqlx::query(
            "SELECT person_id FROM source_identities
             WHERE kind = ?
               AND external_actor_id = ?
               AND (source_id = ? OR source_id IS NULL)
             LIMIT 1",
        )
        .bind(source_identity_kind_to_db(&kind))
        .bind(external_actor_id)
        .bind(source_bind)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => {
                let s: String = row.try_get("person_id")?;
                let id = Uuid::parse_str(&s).map_err(|e| DbError::InvalidData {
                    column: "source_identities.person_id".into(),
                    message: e.to_string(),
                })?;
                Ok(Some(id))
            }
            None => Ok(None),
        }
    }
}

fn row_to_source_identity(row: sqlx::sqlite::SqliteRow) -> DbResult<SourceIdentity> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "source_identities.id".into(),
        message: e.to_string(),
    })?;
    let person_id_str: String = row.try_get("person_id")?;
    let person_id = Uuid::parse_str(&person_id_str).map_err(|e| DbError::InvalidData {
        column: "source_identities.person_id".into(),
        message: e.to_string(),
    })?;
    let source_id: Option<String> = row.try_get("source_id")?;
    let source_id = match source_id {
        Some(s) => Some(Uuid::parse_str(&s).map_err(|e| DbError::InvalidData {
            column: "source_identities.source_id".into(),
            message: e.to_string(),
        })?),
        None => None,
    };
    let kind_str: String = row.try_get("kind")?;
    let kind = source_identity_kind_from_db(&kind_str)?;
    let external_actor_id: String = row.try_get("external_actor_id")?;
    Ok(SourceIdentity {
        id,
        person_id,
        source_id,
        kind,
        external_actor_id,
    })
}
