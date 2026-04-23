//! DAY-109 TST-v0.4-01 companion fixture for `SyncRun`.
//!
//! `SyncRun` (crates/dayseam-core/src/types/run.rs) gained the
//! `SerdeDefaultAudit` derive in DAY-109 because run rows survive
//! across Dayseam upgrades and feed the crash-recovery sweep that
//! flips any orphaned `Running` row to `Failed`. An unaudited default
//! on a field like `superseded_by` — defaulting to `None` so older
//! rows that predate the column deserialise cleanly — would silently
//! mask the supersession-link the orchestrator relies on to chain
//! retried runs, and the sweep would have no way to tell apart
//! "actually nothing superseded this run" from "we lost the link to
//! the row that did". This fixture is the class detector.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct SyncRun {
    id: String,
    started_at: String,
    #[serde(default)]
    superseded_by: Option<String>,
}

fn main() {}
