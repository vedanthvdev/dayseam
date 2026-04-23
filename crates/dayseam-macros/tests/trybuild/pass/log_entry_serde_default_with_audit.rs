//! DAY-109 TST-v0.4-01 positive companion to
//! `fail/log_entry_serde_default_without_audit.rs`. A `LogEntry`-shaped
//! struct with a `#[serde(default)]` field paired with a `repair = "..."`
//! annotation should compile cleanly. The `repair` shape is the right
//! one for `level` specifically: a real backfill PR for legacy log
//! rows would either re-derive the level from the `message` prefix
//! (e.g. `[WARN]` markers older builds wrote) or default to `Info` and
//! flag the row as imputed, both of which a named
//! `SerdeDefaultRepair` impl can encode.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct LogEntry {
    timestamp: String,
    message: String,
    #[serde(default)]
    #[serde_default_audit(repair = "log_entry_level_backfill_from_message_prefix")]
    level: Option<String>,
}

fn main() {}
