//! A `#[serde(default)]` field paired with a `repair = "…"` audit
//! annotation should compile cleanly — this is the "I registered a
//! SerdeDefaultRepair for this field" shape, mirroring how v0.2.1's
//! Confluence email backfill is expected to look after DAY-88 + DAY-90.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct ConfluenceRow {
    #[serde(default)]
    #[serde_default_audit(repair = "confluence_email")]
    email: String,
    workspace_url: String,
}

fn main() {}
