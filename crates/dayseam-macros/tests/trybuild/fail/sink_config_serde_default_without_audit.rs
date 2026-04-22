//! DAY-100 TST-v0.3-01 companion fixture.
//!
//! `SinkConfig` (crates/dayseam-core/src/types/sink.rs) grew a
//! `SerdeDefaultAudit` derive in DAY-100 so the next author who adds
//! a `#[serde(default)]` field to one of its variants has to pair it
//! with a `#[serde_default_audit(...)]` annotation. This fixture is
//! the class-detector test: a `SinkConfig`-shaped enum with a
//! defaulted field and no paired audit annotation must fail to
//! compile, just like `SourceConfig::Confluence::email` would without
//! its `repair = "confluence_email"` annotation.
//!
//! The `.stderr` snapshot next to this file pins the exact error
//! message the derive emits for enum-variant fields, mirroring the
//! existing `missing_audit_annotation.rs` (struct-field) companion.

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
enum SinkConfig {
    MarkdownFile {
        config_version: u32,
        dest_dirs: Vec<std::path::PathBuf>,
        #[serde(default)]
        frontmatter: bool,
    },
}

fn main() {}
