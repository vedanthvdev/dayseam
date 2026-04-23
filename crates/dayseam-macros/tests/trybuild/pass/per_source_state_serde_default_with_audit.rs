//! DAY-109 TST-v0.4-01 positive companion to
//! `fail/per_source_state_serde_default_without_audit.rs`. A
//! `PerSourceState`-shaped struct with a `#[serde(default)]` field
//! paired with a documented `repair = "..."` annotation should compile
//! cleanly. The `repair` shape is the right one here because retried
//! counts cannot be reconstructed from disk after the fact — a real
//! backfill PR would either name a `SerdeDefaultRepair` impl that
//! replays the run-history audit log, or accept the data loss with a
//! `no_repair = "..."` waiver instead.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct PerSourceState {
    source_id: String,
    fetched_count: u32,
    #[serde(default)]
    #[serde_default_audit(repair = "per_source_state_retried_count_backfill")]
    retried_count: u32,
}

fn main() {}
