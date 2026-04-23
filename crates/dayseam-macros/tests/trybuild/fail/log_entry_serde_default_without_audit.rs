//! DAY-109 TST-v0.4-01 companion fixture for `LogEntry`.
//!
//! `LogEntry` (crates/dayseam-core/src/types/report.rs) gained the
//! `SerdeDefaultAudit` derive in DAY-109 because the log-drawer
//! `level` filter dropdown maps a chosen severity to "this level and
//! above" — meaning the field's actual value on disk is
//! filter-load-bearing. An unaudited default on `level` (e.g.
//! `default_level = "info"` so older rows that predate the column
//! deserialise) would silently re-bucket warnings and errors as info
//! and hide them from the UI's default `Warn+`-and-above filter,
//! exactly the silent-failure shape DOG-v0.2-04 named.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct LogEntry {
    timestamp: String,
    message: String,
    #[serde(default)]
    level: Option<String>,
}

fn main() {}
