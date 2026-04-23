//! DAY-109 TST-v0.4-01 positive companion to
//! `fail/activity_event_serde_default_without_audit.rs`. An
//! `ActivityEvent`-shaped struct with a `#[serde(default)]` field
//! paired with a `repair = "..."` annotation (naming a registered
//! `SerdeDefaultRepair` impl) should compile cleanly — this is the
//! shape a real backfill PR would land in: a default *plus* a named
//! migration helper that runs at startup to populate the field on
//! pre-v0.5 rows.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct ActivityEvent {
    id: String,
    #[serde(default)]
    #[serde_default_audit(repair = "activity_event_dedup_token_backfill")]
    dedup_token: String,
}

fn main() {}
