//! The user's identities. v0.1 expects a single row; the schema and API
//! support more so multi-machine / multi-email personas can land later
//! without a migration.

use dayseam_core::Identity;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

#[derive(Clone)]
pub struct IdentityRepo {
    pool: SqlitePool,
}

impl IdentityRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, identity: &Identity) -> DbResult<()> {
        let emails = serde_json::to_string(&identity.emails)?;
        let ids = serde_json::to_string(&identity.gitlab_user_ids)?;
        sqlx::query(
            "INSERT INTO identities (id, emails_json, gitlab_user_ids_json, display_name)
             VALUES (?, ?, ?, ?)",
        )
        .bind(identity.id.to_string())
        .bind(emails)
        .bind(ids)
        .bind(&identity.display_name)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::classify_sqlx(e, "identities.insert"))?;
        Ok(())
    }

    pub async fn list(&self) -> DbResult<Vec<Identity>> {
        let rows = sqlx::query(
            "SELECT id, emails_json, gitlab_user_ids_json, display_name
             FROM identities ORDER BY display_name ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_identity).collect()
    }

    pub async fn update(&self, identity: &Identity) -> DbResult<()> {
        let emails = serde_json::to_string(&identity.emails)?;
        let ids = serde_json::to_string(&identity.gitlab_user_ids)?;
        sqlx::query(
            "UPDATE identities
                SET emails_json = ?, gitlab_user_ids_json = ?, display_name = ?
              WHERE id = ?",
        )
        .bind(emails)
        .bind(ids)
        .bind(&identity.display_name)
        .bind(identity.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn row_to_identity(row: sqlx::sqlite::SqliteRow) -> DbResult<Identity> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "identities.id".into(),
        message: e.to_string(),
    })?;
    let emails_json: String = row.try_get("emails_json")?;
    let emails = serde_json::from_str(&emails_json)?;
    let ids_json: String = row.try_get("gitlab_user_ids_json")?;
    let gitlab_user_ids = serde_json::from_str(&ids_json)?;
    Ok(Identity {
        id,
        emails,
        gitlab_user_ids,
        display_name: row.try_get("display_name")?,
    })
}
