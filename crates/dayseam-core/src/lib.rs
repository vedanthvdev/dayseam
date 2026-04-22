//! `dayseam-core` — domain types, error taxonomy, and stable error codes
//! shared across every other Dayseam crate.
//!
//! Everything that crosses a crate boundary or flows over IPC lives here.
//! The TypeScript definitions generated from these Rust types via `ts-rs`
//! are committed to `packages/ipc-types/src/generated/`, and the
//! `ts_types_generated` integration test fails CI if the two ever drift.

pub mod error;
pub mod error_codes;
pub mod types;

pub use error::DayseamError;
pub use types::{
    activity::{ActivityEvent, ActivityKind, Actor, EntityKind, EntityRef, Link, Privacy, RawRef},
    artifact::{Artifact, ArtifactId, ArtifactKind, ArtifactPayload, MergeRequestProvider},
    events::{
        LogEvent, ProgressEvent, ProgressPhase, ReportCompletedEvent, RunId, ToastEvent,
        ToastSeverity,
    },
    identity::{Identity, Person, SourceIdentity, SourceIdentityKind},
    repo::LocalRepo,
    report::{
        Evidence, LogEntry, LogLevel, RenderedBullet, RenderedSection, ReportDraft, RunStatus,
        SourceRunState,
    },
    run::{PerSourceState, SyncRun, SyncRunCancelReason, SyncRunStatus, SyncRunTrigger},
    settings::{Settings, SettingsPatch, ThemePreference},
    sink::{CapabilityConflict, Sink, SinkCapabilities, SinkConfig, SinkKind, WriteReceipt},
    source::{
        AtlassianValidationResult, GitlabValidationResult, SecretRef, Source, SourceConfig,
        SourceHealth, SourceId, SourceKind, SourcePatch,
    },
};
