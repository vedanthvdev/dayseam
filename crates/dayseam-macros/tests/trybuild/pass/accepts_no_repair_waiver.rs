//! `#[serde(default)]` paired with a `no_repair = "…"` waiver should
//! compile: there are legitimate cases where the default really is
//! safe (e.g. an optional-by-convention list that can be empty on
//! disk) and the audit just wants the rationale recorded in source.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
struct Row {
    #[serde(default)]
    #[serde_default_audit(no_repair = "empty list is a well-defined initial state; older rows predate this field and an empty Vec is semantically correct")]
    tags: Vec<String>,
}

fn main() {}
