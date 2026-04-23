//! DAY-109 TST-v0.4-01 positive companion to
//! `fail/artifact_payload_serde_default_without_audit.rs`. An
//! `ArtifactPayload`-shaped enum variant with a `#[serde(default)]`
//! field paired with a documented `no_repair = "..."` waiver should
//! compile cleanly — the audit's job is to surface unaudited defaults,
//! not to forbid every default.
//!
//! Realistic shape: a future `event_count` field on
//! `ArtifactPayload::CommitSet` whose `0` default is genuinely safe
//! (an empty rolled-up day is a well-defined initial state) and where
//! the rationale is recorded in source.

use std::path::PathBuf;

use dayseam_macros::SerdeDefaultAudit;

#[derive(SerdeDefaultAudit, serde::Deserialize)]
#[allow(dead_code)]
enum ArtifactPayload {
    CommitSet {
        repo_path: PathBuf,
        #[serde(default)]
        #[serde_default_audit(
            no_repair = "pre-v0.5 CommitSet rows predate the rolled-up event count; an empty count is semantically equivalent to today's per-event walker output for those rows"
        )]
        event_count: u32,
    },
}

fn main() {}
