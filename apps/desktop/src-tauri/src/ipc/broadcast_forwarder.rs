//! Pumps app-wide broadcasts from [`dayseam_events::AppBus`] into
//! Tauri's frontend event bus via [`tauri::Manager::emit`].
//!
//! One forwarder per process. It subscribes to the toast channel on
//! startup and lives for the lifetime of the app. If the subscriber
//! falls behind (slow frontend, UI thread blocked), `tokio::broadcast`
//! reports `RecvError::Lagged(n)`: we record a persistent log entry so
//! the user can see *why* a toast was missed, then resubscribe from
//! the newest position. We never crash the forwarder on a missed
//! broadcast.
//!
//! `AppBus` is also what Phase 2 uses for settings-changed and
//! update-available signals — those will get their own forwarder
//! methods in this module when they land.

use chrono::Utc;
use dayseam_core::{LogLevel, ToastEvent};
use dayseam_db::{LogRepo, LogRow};
use dayseam_events::{AppBus, ToastSubscribeError};
use serde_json::json;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::task::JoinHandle;

/// Frontend event name the [`ToastEvent`] is emitted under. Keep this
/// in sync with `apps/desktop/src/ipc/useToasts.ts` — the TS side
/// listens on the exact same string.
pub const TOAST_EVENT: &str = "toast";

/// Spawn the long-lived forwarder task.
///
/// Returns its [`JoinHandle`] so the caller (currently `AppState`
/// setup) can hold it for the lifetime of the app. Note that in
/// `tokio`, dropping a `JoinHandle` **detaches** the task — it does
/// *not* abort it. The forwarder instead exits naturally when the
/// last [`AppBus`] clone is dropped, because `broadcast::Receiver::recv`
/// then returns [`ToastSubscribeError::Closed`] and the `run` loop
/// returns. On shutdown the caller should drop `AppState` (which
/// holds the last `AppBus`) and, if it needs a deterministic join,
/// explicitly `.await` this handle or call `.abort()`.
#[must_use]
pub fn spawn<R: Runtime>(handle: AppHandle<R>, bus: AppBus, logs: LogRepo) -> JoinHandle<()> {
    tokio::spawn(run(handle, bus, logs))
}

async fn run<R: Runtime>(handle: AppHandle<R>, bus: AppBus, logs: LogRepo) {
    let mut rx = bus.subscribe_toasts();
    // We hold only the receiver for the lifetime of the task — the
    // `AppBus` clone itself is released immediately so that when the
    // outer process drops its last `AppBus`, the broadcast channel
    // closes and we return via `ToastSubscribeError::Closed`.
    drop(bus);
    loop {
        match rx.recv().await {
            Ok(event) => {
                emit_toast(&handle, &event);
            }
            Err(ToastSubscribeError::Lagged(missed)) => {
                // A slow frontend dropped broadcasts. Persist an
                // explanation so the log drawer shows it, then carry
                // on from the current tail. Never fail silently — the
                // whole point of this forwarder is observable IPC.
                let _ = logs
                    .append(&LogRow {
                        ts: Utc::now(),
                        level: LogLevel::Warn,
                        source_id: None,
                        message: format!(
                            "toast broadcast lagged — {missed} event(s) dropped; resubscribing"
                        ),
                        context: Some(json!({ "missed": missed, "channel": TOAST_EVENT })),
                    })
                    .await;
            }
            Err(ToastSubscribeError::Closed) => {
                // Bus dropped — only happens at shutdown. Stop the
                // loop so the spawned task exits cleanly and Tauri
                // can tear down.
                return;
            }
        }
    }
}

fn emit_toast<R: Runtime>(handle: &AppHandle<R>, event: &ToastEvent) {
    // `emit` returns an `Err` if every window has been closed. That's
    // not a programmer error — a toast published during shutdown is
    // fine to drop. Log at debug so it's discoverable if someone
    // looks.
    if let Err(err) = handle.emit(TOAST_EVENT, event) {
        tracing::debug!(?err, toast_id = %event.id, "dropping toast: no windows to receive");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use dayseam_core::{ToastEvent, ToastSeverity};
    use dayseam_db::{open, LogRepo};
    use std::sync::{Arc, Mutex};
    use tauri::Listener;
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn open_log_repo() -> (LogRepo, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let pool = open(&dir.path().join("state.db")).await.expect("open db");
        (LogRepo::new(pool), dir)
    }

    fn make_toast(title: &str) -> ToastEvent {
        ToastEvent {
            id: Uuid::new_v4(),
            severity: ToastSeverity::Info,
            title: title.to_string(),
            body: None,
            emitted_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn emit_toast_reaches_tauri_listeners() {
        // Tests the leaf emit path directly so we don't depend on the
        // broadcast-subscribe race in the spawned task. The round-trip
        // through the AppBus is covered by `dayseam-events` tests; this
        // test only cares that what the forwarder hands to Tauri lands
        // back out through `listen`.
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_for_listener = captured.clone();
        handle.listen(TOAST_EVENT, move |event| {
            let parsed: ToastEvent =
                serde_json::from_str(event.payload()).expect("toast payload is valid JSON");
            captured_for_listener.lock().unwrap().push(parsed.title);
        });

        emit_toast(&handle, &make_toast("one"));
        emit_toast(&handle, &make_toast("two"));

        // Give the Tauri event dispatcher a chance to run listeners.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            *captured.lock().unwrap(),
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[tokio::test]
    async fn task_exits_cleanly_when_bus_drops() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let bus = AppBus::new();
        let (logs, _dir) = open_log_repo().await;

        let task = spawn(handle.clone(), bus.clone(), logs);
        // Give the spawned task a chance to run `subscribe_toasts`
        // and release its own `AppBus` clone before we drop the
        // caller-held one.
        tokio::task::yield_now().await;
        drop(bus);

        // If the task does not exit on `RecvError::Closed`, the
        // timeout fires and the test fails loudly.
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
        assert!(joined.is_ok(), "forwarder should exit when the bus drops");
        assert!(joined.unwrap().is_ok(), "task panicked");
    }
}
