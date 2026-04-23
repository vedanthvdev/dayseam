//! DAY-109 TST-v0.4-01 companion fixture for `ActivityEvent`.
//!
//! `ActivityEvent` (crates/dayseam-core/src/types/activity.rs) gained
//! the `SerdeDefaultAudit` derive in DAY-109 because every connector
//! emits `ActivityEvent` rows on every sync; an unaudited default added
//! to a field — for example a `dedup_token` defaulting to an empty
//! string so pre-v0.5 rows deserialise cleanly — is exactly the
//! DOG-v0.2-04 silent-failure shape (the field appears populated to
//! downstream code that never sees the original was missing).
//!
//! The shape mirrors the production struct's prefix (`id` + a defaulted
//! field) deliberately, so the error message names a path the
//! production struct would surface verbatim.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct ActivityEvent {
    id: String,
    #[serde(default)]
    dedup_token: String,
}

fn main() {}
