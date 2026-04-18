//! Typed repositories. One file per table. Every repo owns a clone of
//! the `SqlitePool` (cheap — it's `Arc`-backed) and exposes only the
//! shapes we commit to as a public API.
//!
//! Helpers shared across repos live in `helpers`.

pub mod activity_events;
pub mod drafts;
pub mod helpers;
pub mod identities;
pub mod local_repos;
pub mod logs;
pub mod raw_payloads;
pub mod settings;
pub mod sources;
