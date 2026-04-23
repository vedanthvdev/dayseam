//! DAY-109 TST-v0.4-01 companion fixture for `PerSourceState`.
//!
//! `PerSourceState` (crates/dayseam-core/src/types/run.rs) gained the
//! `SerdeDefaultAudit` derive in DAY-109 because the outer `SyncRun`
//! derive does not recurse into nested struct fields — and
//! `PerSourceState` is exactly such a nested struct, riding inside
//! `SyncRun::per_source_state` as a `Vec`. An unaudited default on a
//! field like `retried_count` (defaulting to zero so older
//! `per_source_state_json` blobs deserialise) would silently hide the
//! "this source was retried N times" signal the orchestrator
//! scoreboard depends on. This fixture is the class detector.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct PerSourceState {
    source_id: String,
    fetched_count: u32,
    #[serde(default)]
    retried_count: u32,
}

fn main() {}
