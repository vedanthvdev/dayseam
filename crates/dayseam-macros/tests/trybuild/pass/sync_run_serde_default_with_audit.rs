//! DAY-109 TST-v0.4-01 positive companion to
//! `fail/sync_run_serde_default_without_audit.rs`. A `SyncRun`-shaped
//! struct with a `#[serde(default)]` field paired with a documented
//! `no_repair = "..."` waiver should compile cleanly. `superseded_by`
//! is the canonical example: pre-v0.4 rows genuinely never carried a
//! supersession link, the field's `None` default is correct for those
//! rows, and the rationale is recorded in source so a future reviewer
//! does not have to re-derive the reasoning.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct SyncRun {
    id: String,
    started_at: String,
    #[serde(default)]
    #[serde_default_audit(
        no_repair = "pre-v0.4 sync_runs rows predate the supersession-tracking column; None is the correct value for those rows because no superseding run exists for them in the table"
    )]
    superseded_by: Option<String>,
}

fn main() {}
