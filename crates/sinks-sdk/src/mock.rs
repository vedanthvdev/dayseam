//! [`MockSink`] — a hermetic, in-memory sink for use in downstream
//! crates' integration tests.
//!
//! Ships behind the always-compiled crate surface (no `cfg(test)`) so
//! `dayseam-db`, `dayseam-report`, and the Tauri app can depend on
//! `sinks-sdk` as a normal dependency and still get a usable sink for
//! their own tests without pulling in a real markdown-file implementation.
//!
//! Behaviour:
//!
//! - Records every `write` call into an `Arc<Mutex<Vec<MockWrite>>>` so
//!   tests can assert what the orchestrator fanned out.
//! - Emits a `Starting → InProgress → Completed` progress sequence so
//!   the per-run event plumbing is exercised identically to a real sink.
//! - Polls the cancellation token and returns
//!   [`DayseamError::Cancelled`] if set before the write starts.
//! - Declares [`SinkCapabilities::LOCAL_ONLY`] so it is accepted by the
//!   v0.3 scheduler in tests that exercise unattended dispatch.
//!
//! `MockSink` is intentionally not `SinkKind::Mock` — it reuses
//! `SinkKind::MarkdownFile`. Keeping the single v0.1 kind means tests
//! can register `MockSink` anywhere a real markdown-file sink would
//! eventually be registered without special-casing the dispatcher.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use dayseam_core::{
    DayseamError, ProgressPhase, ReportDraft, SinkCapabilities, SinkConfig, SinkKind, WriteReceipt,
};

use crate::{adapter::SinkAdapter, ctx::SinkCtx};

/// Snapshot of a single recorded `MockSink::write` call.
#[derive(Debug, Clone, PartialEq)]
pub struct MockWrite {
    pub cfg: SinkConfig,
    pub draft_id: uuid::Uuid,
}

/// In-memory sink whose behaviour is deterministic and fully observable.
#[derive(Clone, Default)]
pub struct MockSink {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    writes: Mutex<Vec<MockWrite>>,
    fail_next: Mutex<Option<DayseamError>>,
}

impl std::fmt::Debug for MockSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let writes = self.inner.writes.lock().map(|w| w.len()).unwrap_or(0);
        f.debug_struct("MockSink")
            .field("recorded_writes", &writes)
            .finish()
    }
}

impl MockSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a snapshot of every recorded write, in FIFO order.
    pub fn writes(&self) -> Vec<MockWrite> {
        self.inner
            .writes
            .lock()
            .expect("mock sink mutex poisoned")
            .clone()
    }

    /// Arm the next `write` call to fail with `err`. One-shot — after
    /// firing once, subsequent writes succeed again.
    pub fn fail_next_with(&self, err: DayseamError) {
        *self
            .inner
            .fail_next
            .lock()
            .expect("mock sink mutex poisoned") = Some(err);
    }
}

#[async_trait]
impl SinkAdapter for MockSink {
    fn kind(&self) -> SinkKind {
        SinkKind::MarkdownFile
    }

    fn capabilities(&self) -> SinkCapabilities {
        SinkCapabilities::LOCAL_ONLY
    }

    async fn validate(&self, ctx: &SinkCtx, _cfg: &SinkConfig) -> Result<(), DayseamError> {
        ctx.bail_if_cancelled()?;
        Ok(())
    }

    async fn write(
        &self,
        ctx: &SinkCtx,
        cfg: &SinkConfig,
        draft: &ReportDraft,
    ) -> Result<WriteReceipt, DayseamError> {
        ctx.bail_if_cancelled()?;
        if let Some(err) = self
            .inner
            .fail_next
            .lock()
            .expect("mock sink mutex poisoned")
            .take()
        {
            return Err(err);
        }

        // Emit a `Starting → InProgress → Completed` sequence so the
        // event wiring is exercised the same way a real sink would.
        ctx.progress.send(
            None,
            ProgressPhase::Starting {
                message: "writing draft".to_string(),
            },
        );
        ctx.progress.send(
            None,
            ProgressPhase::InProgress {
                completed: 1,
                total: Some(1),
                message: "mock commit".to_string(),
            },
        );

        let destinations = match cfg {
            SinkConfig::MarkdownFile { dest_dirs, .. } => dest_dirs.clone(),
        };

        self.inner
            .writes
            .lock()
            .expect("mock sink mutex poisoned")
            .push(MockWrite {
                cfg: cfg.clone(),
                draft_id: draft.id,
            });

        let receipt = WriteReceipt {
            run_id: ctx.run_id,
            sink_kind: self.kind(),
            destinations_written: destinations,
            external_refs: Vec::new(),
            // The mock doesn't render bytes; the draft's section count
            // is a stable-ish stand-in for tests that want to assert a
            // non-zero payload was produced.
            bytes_written: draft.sections.len() as u64,
            written_at: Utc::now(),
        };

        ctx.progress.send(
            None,
            ProgressPhase::Completed {
                message: "mock write complete".to_string(),
            },
        );

        Ok(receipt)
    }
}
