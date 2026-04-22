//! Sink destinations — the write-only side of Dayseam. Each configured
//! sink represents one place a rendered [`ReportDraft`](super::report::ReportDraft)
//! can be written to (a folder on disk, an Obsidian vault, a future Slack
//! or email integration).
//!
//! The enum shapes mirror [`super::source`] on purpose: [`SinkKind`] is the
//! high-level category (GroupBy in the UI, dispatch key in the orchestrator),
//! and [`SinkConfig`] carries the per-kind configuration. Adding a new sink
//! is strictly additive — a new `SinkKind` variant plus a new `SinkConfig`
//! variant, never a rename of an existing one (that would be a breaking
//! change on the `config_version` axis).
//!
//! The trait side of the contract lives in `sinks-sdk` so that `dayseam-core`
//! stays free of `async_trait` / `tokio_util` dependencies.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::types::events::RunId;

/// High-level category of a sink. Used for UI grouping and for the
/// orchestrator to pick which [`SinkAdapter`](../../../sinks-sdk/index.html)
/// implementation handles a given [`Sink`] row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SinkKind {
    /// Writes the rendered report to one or two filesystem roots as a
    /// markdown file. Obsidian support is a configuration of this sink
    /// (a vault folder passed in `dest_dirs`), **not** a separate kind.
    MarkdownFile,
}

/// Per-kind configuration. Externally tagged so the on-disk JSON carries
/// the variant name, matching the shape of [`super::source::SourceConfig`].
///
/// Every variant **must** include a matching entry for the
/// [`SinkConfig::config_version`] accessor below so schema migrations have
/// a stable hook when fields change.
///
/// DAY-100 TST-v0.3-01: carries `#[derive(SerdeDefaultAudit)]` even
/// though no field is currently `#[serde(default)]`. The derive is a
/// compile-time nudge — the next person who adds a defaulting field to
/// one of the `SinkConfig` variants has to pair it with a
/// `#[serde_default_audit(...)]` annotation, keeping the DOG-v0.2-04
/// silent-failure guard extended across every persisted type in the
/// v0.4 surface. `SourceConfig` has the same shape since DAY-88.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, dayseam_macros::SerdeDefaultAudit,
)]
#[ts(export)]
pub enum SinkConfig {
    MarkdownFile {
        /// Version of the `MarkdownFile` sink config shape. Bumped when
        /// a field is added, removed, or renamed so the migration layer
        /// can upgrade old rows without guessing.
        config_version: u32,
        /// One or two destination roots. Each is an absolute directory
        /// path. Obsidian vaults are just "one of these happens to be
        /// inside a vault"; the sink does not know or care.
        dest_dirs: Vec<PathBuf>,
        /// When `true`, the rendered markdown is wrapped with YAML
        /// frontmatter (`date`, `template`, `generated_at`, …) so tools
        /// like Obsidian Dataview can index it.
        frontmatter: bool,
    },
}

impl SinkConfig {
    /// Current config version for this variant. Always returns the
    /// integer the caller should write into SQLite alongside the blob
    /// so migrations can re-read old rows without parsing them first.
    pub fn config_version(&self) -> u32 {
        match self {
            Self::MarkdownFile { config_version, .. } => *config_version,
        }
    }

    /// High-level category for this config. Mirrors the relationship
    /// between [`super::source::SourceConfig`] and [`super::source::SourceKind`]:
    /// `config.kind()` always matches the kind stored on the parent row.
    pub fn kind(&self) -> SinkKind {
        match self {
            Self::MarkdownFile { .. } => SinkKind::MarkdownFile,
        }
    }
}

/// Declarative description of what a sink is and isn't allowed to do.
/// The orchestrator consults this *before* dispatching a write, and the
/// v0.3 scheduler refuses to fire any sink whose
/// [`SinkCapabilities::safe_for_unattended`] is `false`. This is the
/// mechanism that keeps the "never auto-send without review" promise true
/// even once scheduled runs exist.
///
/// The type is declared in `dayseam-core` rather than `sinks-sdk` so the
/// persistence layer, the orchestrator, and the UI can all reference it
/// without pulling in the full trait surface (the trait itself lives in
/// `sinks-sdk`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SinkCapabilities {
    /// Writes only to the user's local filesystem. No network I/O.
    /// `sink-markdown-file` sets this to `true`.
    pub local_only: bool,
    /// Writes to a remote service (Slack, email, Notion…). Implies
    /// network I/O and a higher trust cost. Mutually exclusive with
    /// [`Self::local_only`] — enforced by [`Self::validate`].
    pub remote_write: bool,
    /// Must not run without the user present (e.g. a future
    /// "open-in-Bear" sink that surfaces a draft for confirmation).
    /// Implies `!safe_for_unattended`.
    pub interactive_only: bool,
    /// Safe to fire from a scheduled unattended run. Implies
    /// `!interactive_only` and typically `local_only`. The v0.3
    /// scheduler uses this flag as a hard gate.
    pub safe_for_unattended: bool,
}

impl SinkCapabilities {
    /// Returns the canonical capability set for a strictly local sink.
    /// The v0.1 markdown-file sink uses this verbatim.
    pub const LOCAL_ONLY: Self = Self {
        local_only: true,
        remote_write: false,
        interactive_only: false,
        safe_for_unattended: true,
    };

    /// Validate a capability combination. Returns
    /// [`CapabilityConflict`] if the flags contradict each other. The
    /// orchestrator calls this every time it registers a sink so a
    /// misdeclared sink fails loudly at startup, not silently at
    /// first write.
    pub fn validate(&self) -> Result<(), CapabilityConflict> {
        if self.local_only && self.remote_write {
            return Err(CapabilityConflict::LocalAndRemote);
        }
        if self.interactive_only && self.safe_for_unattended {
            return Err(CapabilityConflict::InteractiveAndUnattended);
        }
        if !self.local_only && !self.remote_write {
            return Err(CapabilityConflict::NeitherLocalNorRemote);
        }
        Ok(())
    }
}

/// Reason a [`SinkCapabilities`] combination is invalid. Deliberately a
/// concrete error rather than a panic so the orchestrator can degrade
/// gracefully (log, skip the sink, keep running) rather than crash the
/// app over a misconfigured third-party sink in the future.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityConflict {
    #[error("sink cannot be both local_only and remote_write")]
    LocalAndRemote,
    #[error("sink cannot be both interactive_only and safe_for_unattended")]
    InteractiveAndUnattended,
    #[error("sink must set at least one of local_only or remote_write")]
    NeitherLocalNorRemote,
}

/// The persisted record describing one configured sink. Parallel shape to
/// [`super::source::Source`] — a handful of rows in SQLite, plus the JSON
/// blob with per-kind settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Sink {
    pub id: Uuid,
    pub kind: SinkKind,
    /// Human-readable label shown in the UI ("My Obsidian vault",
    /// "Reports folder"). Not required to be unique.
    pub label: String,
    pub config: SinkConfig,
    pub created_at: DateTime<Utc>,
    pub last_write_at: Option<DateTime<Utc>>,
}

/// Structured result returned from a successful
/// [`SinkAdapter::write`](../../../sinks-sdk/index.html) call. Surfaces
/// exactly which files touched disk and how large the payload was so the
/// UI can show a trustworthy confirmation ("wrote 4.2 KB to ~/notes/…")
/// and the orchestrator can record a [`WorkItem`](-)-style receipt in the
/// activity store for future dedup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WriteReceipt {
    /// The run this write belonged to. `None` for ad-hoc writes that
    /// weren't dispatched through the run orchestrator (e.g. a manual
    /// "Save as…" click in the UI).
    pub run_id: Option<RunId>,
    pub sink_kind: SinkKind,
    /// Absolute paths the sink actually wrote to. A markdown-file sink
    /// configured with two `dest_dirs` returns two entries here; a
    /// future Slack sink would return an empty list and rely on
    /// `external_refs` instead.
    pub destinations_written: Vec<PathBuf>,
    /// Opaque per-sink strings pointing at the write (e.g. a message
    /// URL for a future Slack sink, an email thread id). Empty for
    /// local-only sinks.
    pub external_refs: Vec<String>,
    /// Total bytes written across `destinations_written`. Used for UI
    /// confirmations and for a future storage-pressure warning.
    pub bytes_written: u64,
    pub written_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_only_caps_are_canonical_markdown_sink_shape() {
        let caps = SinkCapabilities::LOCAL_ONLY;
        assert!(caps.local_only);
        assert!(!caps.remote_write);
        assert!(!caps.interactive_only);
        assert!(caps.safe_for_unattended);
        assert!(caps.validate().is_ok());
    }

    #[test]
    fn capabilities_reject_local_and_remote_together() {
        let caps = SinkCapabilities {
            local_only: true,
            remote_write: true,
            interactive_only: false,
            safe_for_unattended: true,
        };
        assert!(matches!(
            caps.validate(),
            Err(CapabilityConflict::LocalAndRemote)
        ));
    }

    #[test]
    fn capabilities_reject_interactive_and_unattended_together() {
        let caps = SinkCapabilities {
            local_only: true,
            remote_write: false,
            interactive_only: true,
            safe_for_unattended: true,
        };
        assert!(matches!(
            caps.validate(),
            Err(CapabilityConflict::InteractiveAndUnattended)
        ));
    }

    #[test]
    fn capabilities_reject_neither_local_nor_remote() {
        let caps = SinkCapabilities {
            local_only: false,
            remote_write: false,
            interactive_only: false,
            safe_for_unattended: false,
        };
        assert!(matches!(
            caps.validate(),
            Err(CapabilityConflict::NeitherLocalNorRemote)
        ));
    }

    #[test]
    fn sink_config_kind_matches_variant() {
        let cfg = SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec![PathBuf::from("/tmp/reports")],
            frontmatter: true,
        };
        assert_eq!(cfg.kind(), SinkKind::MarkdownFile);
        assert_eq!(cfg.config_version(), 1);
    }

    #[test]
    fn write_receipt_serialises_round_trip() {
        let receipt = WriteReceipt {
            run_id: Some(RunId::new()),
            sink_kind: SinkKind::MarkdownFile,
            destinations_written: vec![PathBuf::from("/tmp/reports/2026-04-17.md")],
            external_refs: Vec::new(),
            bytes_written: 1234,
            written_at: Utc::now(),
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let back: WriteReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(back, receipt);
    }
}
