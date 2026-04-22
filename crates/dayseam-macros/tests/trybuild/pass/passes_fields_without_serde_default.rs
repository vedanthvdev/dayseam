//! A field without `#[serde(default)]` must compile cleanly — the
//! audit is scoped exclusively to the `#[serde(default)]` anti-pattern.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct Plain {
    required_field: String,
    other_required: Vec<String>,
}

fn main() {}
