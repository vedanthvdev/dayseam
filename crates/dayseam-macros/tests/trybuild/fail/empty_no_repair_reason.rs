//! A blank `no_repair = ""` justification defeats the whole point of
//! the waiver — the audit exists to force the author to write down
//! *why* the default is safe so the reviewer can push back. An empty
//! string passes the syntactic "present" check without saying anything,
//! so the macro rejects it.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
struct Row {
    #[serde(default)]
    #[serde_default_audit(no_repair = "")]
    tags: Vec<String>,
}

fn main() {}
