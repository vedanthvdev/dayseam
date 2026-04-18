//! Per-write context handed to every [`crate::SinkAdapter::write`] call.
//!
//! The shape mirrors `connectors-sdk::ConnCtx` on purpose: one context per
//! dispatch, carrying the run identity, the per-run event streams, and a
//! cancellation token the sink polls between atomic-write boundaries.
//!
//! `SinkCtx` deliberately does **not** carry an HTTP client or an
//! `AuthStrategy` in v0.1 — every shipped sink writes locally. Remote
//! sinks (Slack, email) in v0.4+ will add those fields as an additive
//! change; the struct is marked `#[non_exhaustive]` so callers can't
//! construct a context via struct literal from outside this crate,
//! keeping future field additions source-compatible.

use std::sync::Arc;

use dayseam_events::{LogSender, ProgressSender, RunId};
use tokio_util::sync::CancellationToken;

/// Per-write sink context. One instance is built by the orchestrator for
/// every [`crate::SinkAdapter::validate`] or [`crate::SinkAdapter::write`]
/// call and threaded into the sink.
#[derive(Debug)]
#[non_exhaustive]
pub struct SinkCtx {
    /// The run this write belongs to, when the write was dispatched
    /// from the orchestrator. Ad-hoc writes triggered by a manual "Save
    /// as…" click leave this `None` so the receipt can be distinguished
    /// in logs and UI history.
    pub run_id: Option<RunId>,

    /// Progress stream for this write. Sinks emit "writing…",
    /// "replacing marker block…", "copying to vault…" here so the
    /// log drawer and toasts stay honest about what the sink is doing.
    pub progress: ProgressSender,

    /// Structured log stream for warnings and informational messages.
    pub logs: LogSender,

    /// Cancellation token the sink polls between atomic-write
    /// boundaries. A well-behaved sink checks `cancel.is_cancelled()`
    /// before opening its next temp file and aborts cleanly — partial
    /// writes are always removed via the temp-file + atomic-rename
    /// pattern documented in `ARCHITECTURE.md` §9.1.
    pub cancel: CancellationToken,
}

impl SinkCtx {
    /// Convenience constructor for the hand-rolled context the
    /// orchestrator builds today. Keeping the helper here (rather than
    /// exposing a public `new`) lets us evolve the field set without
    /// churning every call site.
    pub fn new(
        run_id: Option<RunId>,
        progress: ProgressSender,
        logs: LogSender,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            run_id,
            progress,
            logs,
            cancel,
        }
    }

    /// Bail out with [`dayseam_core::DayseamError::Cancelled`] if the
    /// write has been cancelled. Sinks call this between atomic-write
    /// steps (per destination, per marker-block splice) so a scheduled
    /// unattended write aborts promptly on shutdown.
    pub fn bail_if_cancelled(&self) -> Result<(), dayseam_core::DayseamError> {
        if self.cancel.is_cancelled() {
            Err(dayseam_core::DayseamError::Cancelled {
                code: dayseam_core::error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "write cancelled".to_string(),
            })
        } else {
            Ok(())
        }
    }
}

// `Arc<SinkCtx>` is a convenience for orchestrators that thread one
// context through multiple tasks (e.g. fanning a write out to two
// destination directories). Nothing in the trait requires it.
impl SinkCtx {
    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}
