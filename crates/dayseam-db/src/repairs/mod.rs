//! Boot-time serde-default repairs.
//!
//! # Not `sqlx::migrate!` migrations
//!
//! Two separate concepts share the word "migration" in this crate;
//! confusing them is the #1 failure mode when adding a new one:
//!
//! * `crates/dayseam-db/migrations/*.sql` — the **schema** migrations
//!   run automatically by `sqlx::migrate!` when a pool opens. These
//!   are SQL DDL only (table shapes, indexes, column additions).
//! * `crates/dayseam-db/src/repairs/` (this module) — **data-shape**
//!   repairs for rows whose SQL schema is already current but whose
//!   serde-encoded JSON payload carries stale / empty fields because
//!   the row was written on an older app version. These run at
//!   startup after schema migrations, via the registry exposed
//!   below. They never alter table DDL.
//!
//! Adding a new `.sql` migration does **not** imply adding a repair,
//! and vice versa. If you need both, add the SQL migration first
//! (schema) then the repair (data) — opening the pool in the repair
//! impl's `run` must be legal against the new schema.
//!
//! # The `SerdeDefaultRepair` shape
//!
//! CORR-v0.2-08 generalised the v0.2.1 `backfill_atlassian_confluence_email`
//! one-off into this trait so future serde-default-recovery passes
//! don't each reinvent "listen on pool, iterate source rows, patch
//! config, log outcome". Each repair is an isolated `impl` that:
//!
//! 1. knows *which* rows it needs to touch (its own query);
//! 2. knows *how* to patch them (its own field logic);
//! 3. is idempotent — running twice produces the same database state
//!    as running once. A repair that fails idempotency is a bug.
//!
//! The registry returns boxed trait objects so startup can iterate
//! `for repair in registered_repairs() { repair.run(&pool).await }`
//! without caring which concrete impls exist on any given release.
//!
//! # Why `async_trait`
//!
//! `async fn` in a trait is stable as of 1.75 (RPITIT), but
//! [object-safe][] (`Box<dyn SerdeDefaultRepair>`) trait methods
//! that return `impl Future` still require `#[async_trait]`. The
//! registry pattern in this module depends on object safety to
//! return a heterogeneous `Vec<Box<dyn …>>`, so we take the
//! `async_trait` macro hit.
//!
//! [object-safe]: https://doc.rust-lang.org/reference/items/traits.html#object-safety

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::DbResult;

pub mod confluence_email;

/// A boot-time data-shape repair.
///
/// The contract is small on purpose: one "run this against the
/// pool" entry point, one "what do you call yourself in logs"
/// identifier. Anything fancier (dry-run, progress events,
/// per-row callbacks) belongs in the impl, not on the trait.
#[async_trait]
pub trait SerdeDefaultRepair: Send + Sync {
    /// Stable snake_case name, used as a log target and as the
    /// primary key when / if we persist a run history. Each
    /// registered impl must return a unique name.
    fn name(&self) -> &'static str;
    /// Perform the repair against `pool`.
    ///
    /// Implementations must be **idempotent**: running this twice
    /// in a row must leave the database in the same state as
    /// running it once. Startup re-runs the registry on every boot
    /// — a non-idempotent impl would re-patch the same rows
    /// forever.
    ///
    /// A repair that encounters a row it cannot safely fix should
    /// log and continue; returning `Err` is reserved for
    /// catastrophic conditions (pool unavailable, schema missing)
    /// that should surface to the caller.
    async fn run(&self, pool: &SqlitePool) -> DbResult<()>;
}

/// The concrete list of repairs startup iterates. Ordering matters
/// only when two repairs touch the same rows — keep each impl
/// independent when possible so ordering doesn't become a second
/// coupling point between them.
///
/// Register new repairs here. The accompanying unit test
/// [`registered_repairs_has_no_duplicate_names`] pins that two
/// impls don't accidentally pick the same `name()`.
pub fn registered_repairs() -> Vec<Box<dyn SerdeDefaultRepair>> {
    vec![Box::new(confluence_email::ConfluenceEmailRepair)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Names are the logging key and the future run-history key.
    /// Two repairs with the same name would silently clobber each
    /// other in both. Catch that at compile-adjacent time rather
    /// than in production logs.
    #[test]
    fn registered_repairs_has_no_duplicate_names() {
        let repairs = registered_repairs();
        let mut names = HashSet::new();
        for repair in &repairs {
            assert!(
                names.insert(repair.name()),
                "duplicate repair name: {}",
                repair.name()
            );
        }
    }

    /// Belt-and-braces: the registry is never empty while at least
    /// one repair ships in this crate. If every repair is ever
    /// retired, delete this assertion in the same PR as the last
    /// removal — don't let the registry silently go quiet.
    #[test]
    fn registered_repairs_includes_confluence_email() {
        let repairs = registered_repairs();
        assert!(
            repairs.iter().any(|r| r.name() == "confluence_email"),
            "confluence_email repair must be registered; DAY-88 CORR-v0.2-08 \
             moved the v0.2.1 one-off backfill into the registry",
        );
    }
}
