//! Approved local git repositories. One row per discovered-and-approved
//! path; the `local-git` connector only reads from paths that appear here.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Approved local git repository. `is_private` drives the redaction path
/// so content from a private repo is never written into a generated
/// report body by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LocalRepo {
    pub path: PathBuf,
    pub label: String,
    pub is_private: bool,
    pub discovered_at: DateTime<Utc>,
}
