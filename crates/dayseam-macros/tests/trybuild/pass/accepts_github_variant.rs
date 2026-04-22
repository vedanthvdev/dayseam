//! `SourceConfig::GitHub { api_base_url: String }` landed in DAY-93.
//! Unlike `Confluence`, the GitHub variant carries **no**
//! `#[serde(default)]` fields — `api_base_url` is a plain required
//! string because a user connecting GitHub must pick between
//! github.com (`https://api.github.com`) and GitHub Enterprise Server
//! (`https://<host>/api/v3`), and silently defaulting one of them
//! would mis-route every request.
//!
//! This fixture proves the derive accepts a required-only variant
//! alongside the audited variant shape `accepts_enum_variant_fields.rs`
//! already covers — i.e., that adding a new variant with no
//! `#[serde(default)]` fields does not accidentally trip the audit
//! (which would force every future plain variant to carry a spurious
//! waiver annotation).
//!
//! The DAY-92 plan originally placed this fixture in DAY-93 step 2.4;
//! it moved to DAY-94 step 3.3 when `PatAuth::github` landed, so the
//! fixture and the constructor share a PR and a single hardening
//! battery.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
enum SourceConfig {
    LocalGit {
        scan_roots: Vec<String>,
    },
    GitHub {
        api_base_url: String,
    },
    Confluence {
        workspace_url: String,
        #[serde(default)]
        #[serde_default_audit(repair = "confluence_email")]
        email: String,
    },
}

fn main() {}
