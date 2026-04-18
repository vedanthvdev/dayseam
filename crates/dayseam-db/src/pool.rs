//! Connection-pool builder. Every Dayseam process opens exactly one pool
//! at startup and keeps it alive for the lifetime of the app.
//!
//! Invariants this module enforces on every connection:
//!
//!   * `journal_mode = WAL`   — concurrent readers alongside a writer.
//!   * `synchronous = NORMAL` — WAL-safe durability without full fsync.
//!   * `foreign_keys = ON`    — we rely on `ON DELETE CASCADE`.
//!
//! Migrations in `./migrations` are applied to the pool before it is
//! returned, so callers can assume the schema is at v1 or higher.

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::error::{DbError, DbResult};

/// Open the Dayseam database at `path`. Creates the file if absent, runs
/// all pending migrations, and returns a pool ready for repository use.
///
/// Calling this twice against the same path is safe and idempotent: the
/// migrator records which versions have been applied and skips them on
/// subsequent runs.
pub async fn open(path: &Path) -> DbResult<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .map_err(DbError::Sqlx)?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(DbError::Migrate)?;

    Ok(pool)
}
