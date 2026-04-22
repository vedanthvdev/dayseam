//! Sibling fixture to `empty_no_repair_reason.rs` — a blank
//! `repair = ""` name defeats the other half of the audit by naming
//! no repair function for the runtime `SerdeDefaultRepair` registry
//! to resolve. The macro rejects it with a message that points at
//! the registered-repair-name invariant, preserving the symmetry
//! with `no_repair = ""`. TST-v0.4-05: the v0.4 capstone review
//! flagged this as the one audit-decision shape the fail-fixture
//! battery did not pin.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
struct Row {
    #[serde(default)]
    #[serde_default_audit(repair = "")]
    tags: Vec<String>,
}

fn main() {}
