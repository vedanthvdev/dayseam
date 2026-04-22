//! A typo in the audit key (e.g. `repairs` instead of `repair`) must
//! fail loudly rather than silently treating the field as unaudited.
//! Without this guard a rename regression would re-introduce the
//! DOG-v0.2-04 class bug.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
struct Row {
    #[serde(default)]
    #[serde_default_audit(repairs = "confluence_email")]
    email: String,
}

fn main() {}
