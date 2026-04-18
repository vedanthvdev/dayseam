//! Per-run ordered streams for [`ProgressEvent`] and [`LogEvent`].
//!
//! [`RunStreams::new`] is the single call site per sync run: it produces
//! cloneable sender handles for producers (connectors, the orchestrator)
//! and matching receiver halves for the consumer (the Tauri command that
//! forwards the stream to the frontend). When every sender is dropped
//! the receiver observes `None`, which is how the run's end is signalled
//! to the UI.
//!
//! Senders never block and never drop events: the underlying channel is
//! [`tokio::sync::mpsc::unbounded_channel`]. Unbounded is acceptable
//! here because (a) a single run is short-lived, (b) the consumer is
//! expected to drain the channel eagerly, and (c) dropping progress
//! events silently would violate our "never fail silently" principle.
//! If back-pressure becomes necessary in a later release, the public
//! API below is already shaped so the internal channel kind can change
//! without disturbing callers.

use chrono::Utc;
use dayseam_core::{LogEvent, LogLevel, ProgressEvent, ProgressPhase, RunId, SourceId};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Paired sender / receiver halves for a single sync run.
#[derive(Debug)]
pub struct RunStreams {
    pub run_id: RunId,
    pub progress_tx: ProgressSender,
    pub log_tx: LogSender,
    pub progress_rx: ProgressReceiver,
    pub log_rx: LogReceiver,
}

impl RunStreams {
    /// Open a fresh pair of streams for `run_id`. Hold onto the `*_rx`
    /// halves on the consumer side; clone the `*_tx` halves freely to
    /// producers that need to emit.
    #[must_use]
    pub fn new(run_id: RunId) -> Self {
        let (progress_raw_tx, progress_raw_rx) = unbounded_channel();
        let (log_raw_tx, log_raw_rx) = unbounded_channel();
        Self {
            run_id,
            progress_tx: ProgressSender {
                run_id,
                inner: progress_raw_tx,
            },
            log_tx: LogSender {
                run_id,
                inner: log_raw_tx,
            },
            progress_rx: ProgressReceiver {
                inner: progress_raw_rx,
            },
            log_rx: LogReceiver { inner: log_raw_rx },
        }
    }

    /// Split the bundle into (sender pair, receiver pair). Useful when
    /// a caller wants to hand the senders off to a task and keep the
    /// receivers for themselves without destructuring manually.
    #[must_use]
    pub fn split(self) -> ((ProgressSender, LogSender), (ProgressReceiver, LogReceiver)) {
        (
            (self.progress_tx, self.log_tx),
            (self.progress_rx, self.log_rx),
        )
    }
}

/// Cheap, cloneable handle for emitting [`ProgressEvent`]s on a specific
/// run's stream.
#[derive(Debug, Clone)]
pub struct ProgressSender {
    run_id: RunId,
    inner: UnboundedSender<ProgressEvent>,
}

impl ProgressSender {
    /// The run this sender is bound to.
    #[must_use]
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Send a pre-built [`ProgressEvent`]. Returns `false` when the
    /// receiver has been dropped — typically because the run was
    /// cancelled or the consumer closed early — in which case the
    /// caller may choose to abort further work.
    pub fn emit(&self, event: ProgressEvent) -> bool {
        self.inner.send(event).is_ok()
    }

    /// Convenience: build and send a [`ProgressEvent`] at the current
    /// wall-clock time, stamped with this sender's `run_id`.
    pub fn send(&self, source_id: Option<SourceId>, phase: ProgressPhase) -> bool {
        self.emit(ProgressEvent {
            run_id: self.run_id,
            source_id,
            phase,
            emitted_at: Utc::now(),
        })
    }

    /// `true` once the matching receiver has been dropped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
}

/// Cheap, cloneable handle for emitting [`LogEvent`]s on a specific
/// run's stream.
#[derive(Debug, Clone)]
pub struct LogSender {
    run_id: RunId,
    inner: UnboundedSender<LogEvent>,
}

impl LogSender {
    #[must_use]
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Send a pre-built [`LogEvent`]. Returns `false` if the receiver
    /// has been dropped.
    pub fn emit(&self, event: LogEvent) -> bool {
        self.inner.send(event).is_ok()
    }

    /// Convenience: build and send a [`LogEvent`] at the current
    /// wall-clock time, stamped with this sender's `run_id`.
    pub fn send(
        &self,
        level: LogLevel,
        source_id: Option<SourceId>,
        message: impl Into<String>,
        context: serde_json::Value,
    ) -> bool {
        self.emit(LogEvent {
            run_id: Some(self.run_id),
            source_id,
            level,
            message: message.into(),
            context,
            emitted_at: Utc::now(),
        })
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
}

/// Consumer side of the per-run progress stream. `recv` yields `None`
/// when every sender has been dropped.
#[derive(Debug)]
pub struct ProgressReceiver {
    inner: UnboundedReceiver<ProgressEvent>,
}

impl ProgressReceiver {
    pub async fn recv(&mut self) -> Option<ProgressEvent> {
        self.inner.recv().await
    }

    /// Non-blocking read, returning whatever's immediately available.
    pub fn try_recv(&mut self) -> Result<ProgressEvent, tokio::sync::mpsc::error::TryRecvError> {
        self.inner.try_recv()
    }

    /// Close the channel from the receiver side, causing all senders
    /// to observe a closed state on their next send.
    pub fn close(&mut self) {
        self.inner.close();
    }
}

/// Consumer side of the per-run log stream.
#[derive(Debug)]
pub struct LogReceiver {
    inner: UnboundedReceiver<LogEvent>,
}

impl LogReceiver {
    pub async fn recv(&mut self) -> Option<LogEvent> {
        self.inner.recv().await
    }

    pub fn try_recv(&mut self) -> Result<LogEvent, tokio::sync::mpsc::error::TryRecvError> {
        self.inner.try_recv()
    }

    pub fn close(&mut self) {
        self.inner.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn progress_events_arrive_in_fifo_order() {
        let run_id = RunId::new();
        let mut streams = RunStreams::new(run_id);
        let tx = streams.progress_tx.clone();

        for i in 0..5 {
            assert!(tx.send(
                None,
                ProgressPhase::InProgress {
                    completed: i,
                    total: Some(5),
                    message: format!("{i}/5"),
                },
            ));
        }
        drop(tx);
        drop(streams.progress_tx);

        let mut seen = Vec::new();
        while let Some(evt) = streams.progress_rx.recv().await {
            if let ProgressPhase::InProgress { completed, .. } = evt.phase {
                seen.push(completed);
            }
        }
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn run_id_is_stamped_on_every_event() {
        let run_id = RunId::new();
        let mut streams = RunStreams::new(run_id);
        streams.progress_tx.send(
            None,
            ProgressPhase::Starting {
                message: "hello".into(),
            },
        );
        drop(streams.progress_tx);
        let evt = streams.progress_rx.recv().await.expect("one event");
        assert_eq!(evt.run_id, run_id);
    }

    #[tokio::test]
    async fn closed_receiver_makes_sends_fail() {
        let run_id = RunId::new();
        let streams = RunStreams::new(run_id);
        let tx = streams.progress_tx.clone();
        drop(streams.progress_rx);
        drop(streams.progress_tx);
        assert!(tx.is_closed());
        assert!(!tx.send(
            None,
            ProgressPhase::Starting {
                message: "ignored".into()
            }
        ));
    }
}
