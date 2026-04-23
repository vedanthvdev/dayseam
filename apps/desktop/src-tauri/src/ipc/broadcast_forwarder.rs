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

use std::time::{Duration, Instant};

use chrono::Utc;
use dayseam_core::{runtime::supervised_spawn, LogLevel, ToastEvent};
use dayseam_db::{LogRepo, LogRow};
use dayseam_events::{AppBus, ToastSubscribeError};
use serde_json::json;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::task::JoinHandle;

/// Frontend event name the [`ToastEvent`] is emitted under. Keep this
/// in sync with `apps/desktop/src/ipc/useToasts.ts` — the TS side
/// listens on the exact same string.
pub const TOAST_EVENT: &str = "toast";

/// Minimum wall-clock gap between two `log_entries` rows produced
/// by the forwarder's lag path. A flood of toasts that the forwarder
/// cannot keep up with used to amplify one-for-one into log writes;
/// Task 7.3 (PERF-08) bounds the write rate to at most one row per
/// [`LAG_WRITE_MIN_INTERVAL`], with the missed count aggregated
/// across the window and stamped onto the single row.
const LAG_WRITE_MIN_INTERVAL: Duration = Duration::from_millis(500);

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
    // DAY-113: supervised so a panic inside the `broadcast`
    // subscribe or the log-row writer cannot silently detach the
    // one process-wide toast-forwarder and leave the UI without
    // toast delivery for the rest of the session.
    supervised_spawn("broadcast_forwarder::toasts", run(handle, bus, logs))
}

async fn run<R: Runtime>(handle: AppHandle<R>, bus: AppBus, logs: LogRepo) {
    let mut rx = bus.subscribe_toasts();
    // We hold only the receiver for the lifetime of the task — the
    // `AppBus` clone itself is released immediately so that when the
    // outer process drops its last `AppBus`, the broadcast channel
    // closes and we return via `ToastSubscribeError::Closed`.
    drop(bus);
    let mut lag = LagAggregator::new(LAG_WRITE_MIN_INTERVAL);
    loop {
        // Pick a deadline for the next pending-flush check. When
        // nothing is pending we park indefinitely on `recv`; when
        // something is pending we race `recv` against the flush
        // deadline so a burst that stops cold still gets one final
        // log row instead of sitting silently in memory.
        let flush_wait = lag.wait_before_flush();
        tokio::select! {
            biased;
            result = rx.recv() => match result {
                Ok(event) => emit_toast(&handle, &event),
                Err(ToastSubscribeError::Lagged(missed)) => {
                    lag.record(missed);
                    if let Some(flush) = lag.take_if_ready() {
                        write_lag_row(&logs, flush).await;
                    }
                }
                Err(ToastSubscribeError::Closed) => {
                    // Bus dropped — only happens at shutdown. Flush
                    // any pending lag so the final window is not
                    // silently lost, then exit so Tauri can tear down.
                    if let Some(flush) = lag.take_force() {
                        write_lag_row(&logs, flush).await;
                    }
                    return;
                }
            },
            () = sleep_for(flush_wait) => {
                if let Some(flush) = lag.take_if_ready() {
                    write_lag_row(&logs, flush).await;
                }
            }
        }
    }
}

/// Accumulated lag state across a rate-limited window. Coalesces any
/// number of `Lagged(n)` errors that arrive inside
/// [`LAG_WRITE_MIN_INTERVAL`] into a single log row that carries the
/// summed missed count. The `last_flush` baseline is `None` until the
/// first lag happens, so the very first lag event is persisted
/// immediately — the bound only kicks in once there's a prior write
/// to debounce against.
#[derive(Debug)]
struct LagAggregator {
    min_interval: Duration,
    pending: u64,
    last_flush: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
struct LagFlush {
    missed: u64,
}

impl LagAggregator {
    fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            pending: 0,
            last_flush: None,
        }
    }

    fn record(&mut self, missed: u64) {
        self.pending = self.pending.saturating_add(missed);
    }

    /// Drain the pending lag *iff* the debounce window has elapsed.
    /// Returns `None` if either nothing is pending or we flushed
    /// recently enough that we must keep accumulating.
    fn take_if_ready(&mut self) -> Option<LagFlush> {
        if self.pending == 0 {
            return None;
        }
        if let Some(last) = self.last_flush {
            if last.elapsed() < self.min_interval {
                return None;
            }
        }
        self.flush()
    }

    /// Drain the pending lag regardless of how recently the last
    /// flush happened. Used on shutdown so a final burst is never
    /// silently dropped.
    fn take_force(&mut self) -> Option<LagFlush> {
        if self.pending == 0 {
            return None;
        }
        self.flush()
    }

    fn flush(&mut self) -> Option<LagFlush> {
        let missed = std::mem::take(&mut self.pending);
        self.last_flush = Some(Instant::now());
        Some(LagFlush { missed })
    }

    /// How long the caller should wait before the next
    /// `take_if_ready` can succeed. `None` means "nothing pending,
    /// park on `recv` indefinitely"; `Some(Duration::ZERO)` means
    /// "flush immediately".
    fn wait_before_flush(&self) -> Option<Duration> {
        if self.pending == 0 {
            return None;
        }
        match self.last_flush {
            None => Some(Duration::ZERO),
            Some(last) => Some(self.min_interval.saturating_sub(last.elapsed())),
        }
    }
}

/// `None` parks forever; `Some(d)` sleeps for `d`. Factored out so
/// the `tokio::select!` branch stays readable.
async fn sleep_for(wait: Option<Duration>) {
    match wait {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending::<()>().await,
    }
}

async fn write_lag_row(logs: &LogRepo, flush: LagFlush) {
    let missed = flush.missed;
    let _ = logs
        .append(&LogRow {
            ts: Utc::now(),
            level: LogLevel::Warn,
            source_id: None,
            message: format!("toast broadcast lagged — {missed} event(s) dropped; resubscribing"),
            context: Some(json!({ "missed": missed, "channel": TOAST_EVENT })),
        })
        .await;
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

    /// Task 7 invariant #3 (PERF-08 broadcast-amplification).
    ///
    /// A sustained burst of toasts that the forwarder cannot keep
    /// up with must not fan out into one `LogRepo::append` call per
    /// `Lagged(n)` error. The shipping bound is: **at most one lag
    /// row per 500 ms, with the missed count aggregated across the
    /// window**.
    ///
    /// This test reproduces the amplification by publishing five
    /// 100-toast bursts separated by short yields onto a tiny-capacity
    /// bus. Each burst overflows the ring buffer, so the forwarder's
    /// next `recv` returns `Lagged(_)` every cycle. Without a
    /// debounce the forwarder writes one log row per burst; with the
    /// debounce, bursts that fall inside the same 500 ms window are
    /// coalesced to a single row.
    ///
    /// The assertion is deliberately loose (≤3 rows over ~1.1 s of
    /// wall-clock activity). That tolerates one timer slip on loaded
    /// CI runners while still being tight enough to catch a regression
    /// back to "one row per burst".
    #[tokio::test]
    async fn broadcast_forwarder_bounds_writes_under_lag() {
        let (logs, _dir) = open_log_repo().await;
        let bus = AppBus::with_capacity(2);
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();

        let task = spawn(handle.clone(), bus.clone(), logs.clone());
        // Let the forwarder subscribe before we start publishing, so
        // the first burst races against a ready receiver rather than
        // against channel construction.
        tokio::task::yield_now().await;

        let start = tokio::time::Instant::now();
        for burst in 0..5u32 {
            for i in 0..100u32 {
                bus.publish_toast(make_toast(&format!("b{burst}-{i}")));
            }
            // Let the forwarder call `recv` and observe the Lagged
            // error before we publish the next burst — without this
            // yield the sender just fills the buffer without giving
            // the forwarder a chance to see each overflow.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Drop the bus so the forwarder eventually exits and flushes
        // any pending lag — without this, a tail-end burst whose
        // window hasn't expired would stay accumulated in memory.
        drop(bus);
        // Bounded wait for the forwarder task to exit; a hang here
        // would mean the `Closed` branch stopped returning.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
        let elapsed = start.elapsed();

        let rows = logs
            .tail(chrono::DateTime::<Utc>::MIN_UTC, 10_000)
            .await
            .expect("tail");
        let lag_rows = rows.iter().filter(|r| r.message.contains("lagged")).count();

        assert!(
            lag_rows <= 3,
            "broadcast forwarder wrote {lag_rows} lag rows over {elapsed:?} of burst \
             activity; expected ≤3 at one-per-500ms cadence",
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
