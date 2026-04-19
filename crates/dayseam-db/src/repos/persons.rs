//! Canonical humans. Phase 2 only cares about the single `is_self`
//! row; the full multi-person machinery lands later. The partial
//! unique index `idx_persons_single_self` enforces the "at most one
//! self" invariant at the DB layer, so `bootstrap_self` can be
//! naive-`INSERT`-and-tolerate-the-`UniqueViolation`-retry without a
//! race.

use dayseam_core::{Identity, Person};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

#[derive(Clone)]
pub struct PersonRepo {
    pool: SqlitePool,
}

impl PersonRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Idempotent. If a self-`Person` already exists, returns it.
    /// Otherwise derives the display name from the first legacy
    /// `identities` row (if any), inserts a fresh self-`Person`, and
    /// returns the new row. The DB migration already runs a best-effort
    /// backfill; this function is the app-side safety net for brand
    /// new databases that never had `identities` in them.
    pub async fn bootstrap_self(&self, fallback_display_name: &str) -> DbResult<Person> {
        if let Some(existing) = self.get_self().await? {
            return Ok(existing);
        }

        let display_name = match sqlx::query("SELECT display_name FROM identities LIMIT 1")
            .fetch_optional(&self.pool)
            .await?
        {
            Some(row) => row
                .try_get::<String, _>("display_name")
                .unwrap_or_else(|_| fallback_display_name.to_string()),
            None => fallback_display_name.to_string(),
        };

        let person = Person::new_self(display_name);
        match self.insert(&person).await {
            Ok(()) => Ok(person),
            Err(DbError::Conflict { .. }) => {
                self.get_self().await?.ok_or_else(|| DbError::InvalidData {
                    column: "persons.is_self".into(),
                    message: "conflict on bootstrap_self but no self row on re-read".into(),
                })
            }
            Err(other) => Err(other),
        }
    }

    pub async fn insert(&self, person: &Person) -> DbResult<()> {
        sqlx::query("INSERT INTO persons (id, display_name, is_self) VALUES (?, ?, ?)")
            .bind(person.id.to_string())
            .bind(&person.display_name)
            .bind(if person.is_self { 1_i64 } else { 0_i64 })
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::classify_sqlx(e, "persons.insert"))?;
        Ok(())
    }

    pub async fn get_self(&self) -> DbResult<Option<Person>> {
        let row = sqlx::query("SELECT id, display_name, is_self FROM persons WHERE is_self = 1")
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_person).transpose()
    }

    pub async fn get(&self, id: Uuid) -> DbResult<Option<Person>> {
        let row = sqlx::query("SELECT id, display_name, is_self FROM persons WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_person).transpose()
    }

    pub async fn list(&self) -> DbResult<Vec<Person>> {
        let rows =
            sqlx::query("SELECT id, display_name, is_self FROM persons ORDER BY display_name ASC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(row_to_person).collect()
    }

    /// Utility: promote a legacy `identities` row to the canonical
    /// self-`Person` if (and only if) no self-row exists. Used by
    /// setup wizards that want a single call at startup.
    pub async fn bootstrap_from_identity(&self, identity: &Identity) -> DbResult<Person> {
        if let Some(existing) = self.get_self().await? {
            return Ok(existing);
        }
        let person = Person {
            id: identity.id,
            display_name: identity.display_name.clone(),
            is_self: true,
        };
        self.insert(&person).await?;
        Ok(person)
    }
}

fn row_to_person(row: sqlx::sqlite::SqliteRow) -> DbResult<Person> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "persons.id".into(),
        message: e.to_string(),
    })?;
    let display_name: String = row.try_get("display_name")?;
    let is_self: i64 = row.try_get("is_self")?;
    Ok(Person {
        id,
        display_name,
        is_self: is_self != 0,
    })
}
