//! Domain types. Each submodule owns a cluster of related records, and every
//! public type derives `serde::Serialize`, `serde::Deserialize`, and
//! `ts_rs::TS` so the frontend always sees the same shape the Rust core does.

pub mod activity;
pub mod events;
pub mod identity;
pub mod repo;
pub mod report;
pub mod source;
