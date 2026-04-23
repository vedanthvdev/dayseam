//! DAY-109 TST-v0.4-01 companion fixture for `ArtifactPayload`.
//!
//! `ArtifactPayload` (crates/dayseam-core/src/types/artifact.rs) gained
//! the `SerdeDefaultAudit` derive in DAY-109 so the next author who
//! adds a back-compat `#[serde(default)]` field to one of the
//! variants — e.g. an `event_count: u32` defaulting to zero on
//! pre-v0.5 rows so the new walker can read them — is forced to
//! pair the default with a `#[serde_default_audit(...)]` annotation.
//! This fixture is the class detector: an `ArtifactPayload`-shaped
//! enum (struct-style variants, externally tagged) with a defaulted
//! field and no paired audit must fail to compile.
//!
//! The shape mirrors `ArtifactPayload::CommitSet` deliberately
//! (`repo_path` + a defaulted `event_count`) so the error message
//! visibly names the variant + field path the production type would
//! surface.

use std::path::PathBuf;

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
enum ArtifactPayload {
    CommitSet {
        repo_path: PathBuf,
        #[serde(default)]
        event_count: u32,
    },
}

fn main() {}
