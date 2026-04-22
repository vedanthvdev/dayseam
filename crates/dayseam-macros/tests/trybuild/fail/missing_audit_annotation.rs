//! `#[serde(default)]` with no `#[serde_default_audit(...)]` partner
//! is exactly the DOG-v0.2-04 anti-pattern: a field that silently
//! deserialises to its default on older rows, bypassing the
//! validation that protects live auth/config flows. The derive must
//! surface this as a compile error that names the offending field.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
struct Row {
    #[serde(default)]
    email: String,
}

fn main() {}
