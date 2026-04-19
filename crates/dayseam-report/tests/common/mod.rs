//! Shared fixture builders for the report engine integration tests.
//!
//! These helpers mirror the scenarios exercised by
//! `connector-local-git/tests/sync.rs` (Phase 2 Task 2) so the engine
//! is tested against inputs the real connector would actually emit.
//! Events and artifacts are built from plain Rust values — no git2,
//! no tempfiles — because the engine is pure and the connector's
//! behaviour is already proven by its own suite.

// Integration tests each compile this module into their own binary,
// and rust warns about helpers that are used by some binaries but
// not others. Silence `dead_code` at the module level so adding a
// helper for one test never requires teaching the others about it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, Artifact, ArtifactId, ArtifactKind, ArtifactPayload,
    EntityRef, Person, Privacy, RawRef, RunStatus, SourceId, SourceIdentity, SourceIdentityKind,
    SourceRunState,
};
use dayseam_report::ReportInput;
use uuid::Uuid;

pub const FIXTURE_DATE_STR: &str = "2026-04-18";
pub const REPORT_ID_STR: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
pub const GENERATED_AT_STR: &str = "2026-04-18T23:00:00Z";

pub fn fixture_date() -> NaiveDate {
    NaiveDate::parse_from_str(FIXTURE_DATE_STR, "%Y-%m-%d").unwrap()
}

pub fn report_id() -> Uuid {
    Uuid::parse_str(REPORT_ID_STR).unwrap()
}

pub fn generated_at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(GENERATED_AT_STR)
        .unwrap()
        .with_timezone(&Utc)
}

pub fn source_id(byte: u8) -> SourceId {
    let mut bytes = [0u8; 16];
    bytes[0] = byte;
    Uuid::from_bytes(bytes)
}

pub fn self_person() -> Person {
    Person {
        id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
        display_name: "Self".into(),
        is_self: true,
    }
}

pub fn self_git_identity(source_id: SourceId, email: &str) -> SourceIdentity {
    SourceIdentity {
        id: Uuid::new_v4(),
        person_id: self_person().id,
        source_id: Some(source_id),
        kind: SourceIdentityKind::GitEmail,
        external_actor_id: email.into(),
    }
}

pub fn commit_event(
    source_id: SourceId,
    sha: &str,
    repo_path: &str,
    author_email: &str,
    occurred_at_hour: u32,
    title: &str,
    privacy: Privacy,
) -> ActivityEvent {
    ActivityEvent {
        id: ActivityEvent::deterministic_id(&source_id.to_string(), sha, "CommitAuthored"),
        source_id,
        external_id: sha.into(),
        kind: ActivityKind::CommitAuthored,
        occurred_at: Utc
            .with_ymd_and_hms(2026, 4, 18, occurred_at_hour, 0, 0)
            .unwrap(),
        actor: Actor {
            display_name: "Self".into(),
            email: Some(author_email.into()),
            external_id: None,
        },
        title: title.into(),
        body: None,
        links: vec![],
        entities: vec![EntityRef {
            kind: "repo".into(),
            external_id: repo_path.into(),
            label: None,
        }],
        parent_external_id: None,
        metadata: serde_json::Value::Null,
        raw_ref: RawRef {
            storage_key: format!("git-commit:{sha}"),
            content_type: "application/x-git-commit".into(),
        },
        privacy,
    }
}

pub fn commit_set_artifact(
    source_id: SourceId,
    repo_path: &str,
    events: &[&ActivityEvent],
) -> Artifact {
    let date = fixture_date();
    let external_id = format!("{repo_path}::{date}");
    let event_ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
    let commit_shas: Vec<String> = events.iter().map(|e| e.external_id.clone()).collect();
    Artifact {
        id: ArtifactId::deterministic(&source_id, ArtifactKind::CommitSet, &external_id),
        source_id,
        kind: ArtifactKind::CommitSet,
        external_id,
        payload: ArtifactPayload::CommitSet {
            repo_path: PathBuf::from(repo_path),
            date,
            event_ids,
            commit_shas,
        },
        created_at: generated_at(),
    }
}

/// Start a fresh [`ReportInput`] with the standard fixture metadata.
/// Callers push events / artifacts / identities before passing it in.
pub fn fixture_input() -> ReportInput {
    ReportInput {
        id: report_id(),
        date: fixture_date(),
        template_id: dayseam_report::DEV_EOD_TEMPLATE_ID.into(),
        template_version: dayseam_report::DEV_EOD_TEMPLATE_VERSION.into(),
        person: self_person(),
        source_identities: Vec::new(),
        events: Vec::new(),
        artifacts: Vec::new(),
        per_source_state: HashMap::new(),
        verbose_mode: false,
        generated_at: generated_at(),
    }
}

pub fn succeeded_state(fetched: usize) -> SourceRunState {
    SourceRunState {
        status: RunStatus::Succeeded,
        started_at: generated_at(),
        finished_at: Some(generated_at()),
        fetched_count: fetched,
        error: None,
    }
}
