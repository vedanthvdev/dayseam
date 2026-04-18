//! Generic key/value settings, stored as JSON blobs per row. Callers pick
//! their own schema for the values they put in — this repo just
//! round-trips anything that serialises.

use serde::{de::DeserializeOwned, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::DbResult;

#[derive(Clone)]
pub struct SettingsRepo {
    pool: SqlitePool,
}

impl SettingsRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Fetch the raw JSON string for `key`. Returns `None` if unset —
    /// callers decide whether that's a default or an error condition.
    pub async fn get_raw(&self, key: &str) -> DbResult<Option<String>> {
        let row = sqlx::query("SELECT value_json FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row
            .map(|r| r.try_get::<String, _>("value_json"))
            .transpose()?)
    }

    /// Typed convenience — deserialises the JSON blob into `T`.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> DbResult<Option<T>> {
        match self.get_raw(key).await? {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub async fn set<T: Serialize>(&self, key: &str, value: &T) -> DbResult<()> {
        let json = serde_json::to_string(value)?;
        sqlx::query(
            "INSERT INTO settings (key, value_json) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        )
        .bind(key)
        .bind(json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
