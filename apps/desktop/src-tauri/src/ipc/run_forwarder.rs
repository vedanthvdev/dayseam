//! Forwards per-run progress and log events from
//! [`dayseam_events::RunStreams`] into Tauri [`ipc::Channel<T>`]
//! instances supplied by the frontend.
//!
//! The contract is deliberately simple: two spawned tasks, one per
//! stream, each consuming receivers until they return `None` and then
//! exiting. That `None` corresponds to "every sender dropped", which
//! is how a run's end is signalled end-to-end. We never block, never
//! drop events, and never leak tasks — if the matching `Channel<T>`
//! has been closed on the frontend side, `send` returns `Err` and we
//! exit too.

use dayseam_core::{runtime::supervised_spawn, LogEvent, ProgressEvent};
use dayseam_events::{LogReceiver, ProgressReceiver};
use tauri::ipc::Channel;
use tokio::task::JoinHandle;

/// Spawn the progress-stream forwarder. The task exits when either
/// every `ProgressSender` for the run has been dropped (receiver
/// yields `None`) or the frontend's `Channel` has been closed
/// (`send` errors).
///
/// DAY-113: wrapped in `supervised_spawn` so a panic in event
/// serialisation cannot silently detach the forwarder — a panic here
/// used to drop the entire progress stream for the run, leaving the
/// UI stuck at whatever phase it last rendered. Supervision turns
/// that into a single `error`-level log line and lets the run reaper
/// close the loop cleanly.
#[must_use]
pub fn spawn_progress_forwarder(
    mut rx: ProgressReceiver,
    channel: Channel<ProgressEvent>,
) -> JoinHandle<()> {
    supervised_spawn("run_forwarder::progress", async move {
        while let Some(event) = rx.recv().await {
            if channel.send(event).is_err() {
                tracing::debug!("progress channel closed — stopping forwarder");
                break;
            }
        }
    })
}

/// Spawn the log-stream forwarder. Same termination rules as
/// [`spawn_progress_forwarder`] and the same DAY-113 supervision
/// reasoning.
#[must_use]
pub fn spawn_log_forwarder(mut rx: LogReceiver, channel: Channel<LogEvent>) -> JoinHandle<()> {
    supervised_spawn("run_forwarder::log", async move {
        while let Some(event) = rx.recv().await {
            if channel.send(event).is_err() {
                tracing::debug!("log channel closed — stopping forwarder");
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use dayseam_core::{ProgressPhase, RunId};
    use dayseam_events::RunStreams;

    // We cannot build a real `tauri::ipc::Channel` outside the
    // `tauri::Builder` context (it wraps a callback registered with
    // the runtime). So we only exercise the drain-to-completion path
    // here by closing the receiver after the senders drop. Full
    // round-trip coverage lives in the `ipc::commands` integration
    // test under the dev-commands feature.

    #[tokio::test]
    async fn progress_receiver_drains_to_completion_when_senders_drop() {
        let mut streams = RunStreams::new(RunId::new());
        let tx = streams.progress_tx.clone();
        for i in 0..3 {
            tx.send(
                None,
                ProgressPhase::InProgress {
                    completed: i,
                    total: Some(3),
                    message: format!("{i}/3"),
                },
            );
        }
        drop(tx);
        drop(streams.progress_tx);

        let mut count = 0;
        while streams.progress_rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 3);
    }
}
