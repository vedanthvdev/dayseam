//! The [`SinkAdapter`] trait.
//!
//! The contract comes verbatim from `ARCHITECTURE.md` §9.1: a sink is a
//! `validate` method that confirms its configuration is usable, plus a
//! `write` method that atomically commits a [`ReportDraft`] to the
//! sink's destination and returns a [`WriteReceipt`] describing exactly
//! what landed on disk or was sent off-device.
//!
//! The trait deliberately has *no* notion of scheduling, retries, or
//! user consent. Those are the orchestrator's job: it consults each
//! adapter's [`SinkCapabilities`] (from `dayseam-core`) and decides
//! whether to call `write` at all. A sink that declares
//! `safe_for_unattended = true` is promising the orchestrator that it
//! is safe to fire without a user in the loop — enforcement lives one
//! level up, not inside the adapter.

use async_trait::async_trait;
use dayseam_core::{
    DayseamError, ReportDraft, SinkCapabilities, SinkConfig, SinkKind, WriteReceipt,
};

use crate::ctx::SinkCtx;

/// The single trait every sink implementation satisfies.
///
/// Object-safe: the orchestrator keeps a `HashMap<SinkKind, Arc<dyn
/// SinkAdapter>>` and dispatches by kind. Adding a new sink is purely
/// additive — register a new `SinkKind` variant in `dayseam-core`, add a
/// matching `SinkConfig` variant, and drop a crate under
/// `crates/sinks/sink-<name>/` that implements this trait.
#[async_trait]
pub trait SinkAdapter: Send + Sync {
    /// The [`SinkKind`] this adapter handles. The orchestrator uses
    /// this as the dispatch key, so each kind has exactly one adapter.
    fn kind(&self) -> SinkKind;

    /// Declarative capability set. Consulted by the orchestrator and,
    /// in v0.3+, the scheduler. The orchestrator calls
    /// [`SinkCapabilities::validate`] on whatever this returns before
    /// registering the adapter so a misdeclared capability combo
    /// (`local_only && remote_write`, etc.) fails loudly at startup.
    fn capabilities(&self) -> SinkCapabilities;

    /// Inspect `cfg` and confirm the adapter can write with it
    /// *without* performing the write. For a markdown-file sink this
    /// means: destination roots exist, are directories, and are
    /// writable. For a future Slack sink it would mean: token resolves,
    /// workspace is reachable, the chosen channel exists.
    ///
    /// `validate` is called:
    /// - When the user saves a new sink configuration in the UI.
    /// - Immediately before every `write` so stale configs (deleted
    ///   folders, revoked tokens) are caught before we render the
    ///   draft and not after.
    async fn validate(&self, ctx: &SinkCtx, cfg: &SinkConfig) -> Result<(), DayseamError>;

    /// Commit `draft` to the sink's destination. Every implementation
    /// **must** write atomically (temp file + rename for filesystem
    /// sinks; two-phase commit equivalents for remote sinks). Partial
    /// writes on cancel/crash are a correctness bug.
    ///
    /// Emits progress via `ctx.progress` and log events via
    /// `ctx.logs`. Returns a [`WriteReceipt`] that enumerates the
    /// destinations that were actually touched so the UI can show a
    /// trustworthy confirmation.
    async fn write(
        &self,
        ctx: &SinkCtx,
        cfg: &SinkConfig,
        draft: &ReportDraft,
    ) -> Result<WriteReceipt, DayseamError>;
}
