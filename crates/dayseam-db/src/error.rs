//! Database-layer error type. Deliberately local to `dayseam-db` — callers
//! map this into `DayseamError` at whichever public boundary they expose.
//! Keeping it separate means repo methods can surface SQLite-specific
//! concerns (like a UNIQUE violation) without leaking them into
//! `DayseamError` variants.

use thiserror::Error;

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, Error)]
pub enum DbError {
    /// SQLite reported a UNIQUE, NOT NULL, or FK violation. We surface this
    /// distinctly from other errors so callers can decide whether "row
    /// already exists" means retry, skip, or abort.
    #[error("conflict: {what}")]
    Conflict { what: String },

    /// Anything sqlx bubbled up that isn't a constraint violation.
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Running pending migrations failed. Always fatal at startup.
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// A JSON column failed to serialise or parse. Indicates either a bug
    /// in our model or database corruption; either way, surface it.
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),

    /// Data on disk didn't match an expected enum or union tag. Points at
    /// schema drift or an older DB that predates a required migration.
    #[error("invalid data in column `{column}`: {message}")]
    InvalidData { column: String, message: String },
}

impl DbError {
    /// Classify an arbitrary `sqlx::Error` — if it's a constraint
    /// violation, rewrap as `Conflict`; otherwise pass through.
    pub(crate) fn classify_sqlx(err: sqlx::Error, what: &str) -> Self {
        if let sqlx::Error::Database(ref db_err) = err {
            if db_err.is_unique_violation()
                || db_err.is_foreign_key_violation()
                || db_err.is_check_violation()
            {
                return DbError::Conflict {
                    what: format!("{what}: {db_err}"),
                };
            }
        }
        DbError::Sqlx(err)
    }
}
