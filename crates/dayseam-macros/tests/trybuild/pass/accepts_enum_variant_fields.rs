//! The derive must audit enum variants the same way it audits
//! structs, because `dayseam_core::SourceConfig` is exactly this
//! shape — one struct-like variant per source kind. A
//! `#[serde(default)]` field on any variant should need a paired
//! audit annotation; an audited field should pass.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
enum SourceConfig {
    LocalGit {
        scan_roots: Vec<String>,
    },
    Confluence {
        workspace_url: String,
        #[serde(default)]
        #[serde_default_audit(repair = "confluence_email")]
        email: String,
    },
}

fn main() {}
