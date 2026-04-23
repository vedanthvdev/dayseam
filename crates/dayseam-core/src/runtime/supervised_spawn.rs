//! [`supervised_spawn`] — the canonical shape for spawning a
//! background future in Dayseam production code.
//!
//! See the parent module ([`crate::runtime`]) for the design rationale
//! (DAY-113, F-10 follow-up) and the list of three sites that
//! deliberately opt out.

use std::future::Future;

use tokio::task::{JoinError, JoinHandle};

/// Spawn `future` under supervision. Returns a `JoinHandle<()>` whose
/// `await` **never** observes a panic from the inner future — a
/// panic turns into a single `tracing::error!(context, error = ?e, …)`
/// log line and the outer handle completes cleanly.
///
/// `context` is a caller-supplied static string that names the
/// spawn site; it appears on every log line the supervisor emits and
/// is what makes a post-mortem readable when several supervised tasks
/// fail simultaneously. Use a short snake_case label that a reader can
/// grep for (e.g. `"orphan_secret_audit"`, `"run_forwarder::progress"`,
/// `"orchestrator::retention_sweep"`), not a full sentence.
///
/// The supervisor pattern uses two nested `tokio::spawn`s: the inner
/// one runs the caller's future and is the only one whose `JoinError`
/// carries `is_panic()`; the outer one awaits the inner and translates
/// the `JoinError` into a `tracing` event via [`log_join_result`].
/// Callers that need the outer `JoinHandle` can `.await` it or feed
/// it to a reaper without fear that an inner-future panic will
/// propagate.
///
/// # Logging shape
///
/// - Clean completion → `tracing::debug!(context, "supervised task completed")`.
/// - Inner future panicked → `tracing::error!(context, error = ?e, "supervised task panicked")`.
/// - Inner future cancelled (runtime shutdown or explicit abort of
///   the inner handle) → `tracing::warn!(context, error = ?e, "supervised task cancelled")`.
///
/// # Return type constraint: `Output = ()`
///
/// Every current Dayseam spawn site returns `()` (the caller either
/// emits via an IPC channel / app bus and has no value to hand back,
/// or the value is irrelevant because the task is fire-and-forget).
/// Constraining the helper to `Output = ()` keeps the supervisor's
/// return-value drop path trivial — a future that wanted to return a
/// `Result` for the caller to inspect is almost certainly a
/// foreground await, not a supervised background task, and using
/// `supervised_spawn` there would silently eat the `Err`. If a future
/// use case needs a non-unit return, introduce a sibling helper
/// (e.g. `supervised_spawn_with<T>`) rather than widening this one.
pub fn supervised_spawn<F>(context: &'static str, future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let inner = tokio::spawn(future);
        log_join_result(context, inner.await);
    })
}

/// Translate the inner task's `JoinResult` into a single
/// `tracing` event.
///
/// Factored out of [`supervised_spawn`] because the cancellation
/// branch cannot be exercised end-to-end from a test — the outer
/// `JoinHandle` aborts the *supervisor*, not the inner task, and no
/// caller has access to the inner handle. Synthesising a real
/// `JoinError` of each variant via tiny helper spawns and feeding it
/// through this function lets all three branches be asserted directly.
fn log_join_result(context: &'static str, result: Result<(), JoinError>) {
    match result {
        Ok(()) => tracing::debug!(context, "supervised task completed"),
        Err(e) if e.is_panic() => {
            tracing::error!(context, error = ?e, "supervised task panicked");
        }
        Err(e) => {
            tracing::warn!(context, error = ?e, "supervised task cancelled");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;
    use tracing_test::traced_test;

    /// Build a real `JoinError` of the `is_cancelled()` variant by
    /// spawning a parked task and aborting it. Constructing a
    /// `JoinError` directly is impossible — tokio's `JoinError` is
    /// `#[non_exhaustive]` and its constructor is crate-private — so
    /// we have to round-trip through a real abort to get one.
    async fn cancelled_join_error() -> JoinError {
        let h: JoinHandle<()> = tokio::spawn(std::future::pending());
        h.abort();
        h.await
            .expect_err("aborted task must yield a cancelled JoinError")
    }

    /// Same shape for the panic variant — spawn a task that panics,
    /// await its handle, take the `Err` side.
    async fn panicked_join_error() -> JoinError {
        let h: JoinHandle<()> = tokio::spawn(async { panic!("synthetic panic") });
        h.await
            .expect_err("panicking task must yield a panic JoinError")
    }

    /// Clean completion end-to-end: the supervised task runs to an
    /// `Ok(())`, the outer handle resolves cleanly, and the `debug`
    /// log line names the context string. Reverting the helper to a
    /// bare `tokio::spawn(future)` (dropping the supervisor wrapper)
    /// makes the `logs_contain("supervised task completed")` assertion
    /// fail, so this test pins the clean-path instrumentation as well
    /// as the shape.
    #[tokio::test]
    #[traced_test]
    async fn clean_completion_logs_at_debug_and_returns_ok() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();

        let handle = supervised_spawn("test::clean_path", async move {
            ran_clone.store(true, Ordering::SeqCst);
        });

        handle.await.expect("outer handle resolves cleanly");
        assert!(
            ran.load(Ordering::SeqCst),
            "inner future must have executed"
        );
        assert!(logs_contain("supervised task completed"));
        assert!(logs_contain("test::clean_path"));
    }

    /// Panic containment (F-10) end-to-end: a panic inside the
    /// supervised future must NOT propagate through the outer
    /// `JoinHandle`. The outer handle resolves `Ok(())`, and a
    /// `tracing::error!` line carries the context string. This is the
    /// exact invariant F-10 was filed to enforce. Reverting
    /// `supervised_spawn` to `tokio::spawn(f)` (i.e. removing the
    /// catch) makes `handle.await` return `Err(JoinError { is_panic:
    /// true, … })` and the test fails on the `.expect` line.
    #[tokio::test]
    #[traced_test]
    async fn inner_panic_is_contained_and_logged_at_error() {
        let handle = supervised_spawn("test::panic_path", async move {
            panic!("planned panic — pins F-10 invariant");
        });

        handle
            .await
            .expect("outer handle must resolve cleanly despite inner panic");

        assert!(logs_contain("supervised task panicked"));
        assert!(logs_contain("test::panic_path"));
        // The panic payload is part of the `error = ?e` field on
        // the error-level line; tracing-test's `logs_contain` scans
        // the formatted event so the panic string must appear.
        assert!(logs_contain("planned panic"));
    }

    /// Cancellation-path unit test. We cannot trigger this end-to-end
    /// (aborting the outer supervisor handle drops the whole future
    /// before the match arm runs, so no log is emitted), but
    /// [`log_join_result`] is the pure function the supervisor calls,
    /// and feeding it a real `JoinError` of `is_cancelled()` variant
    /// exercises the exact branch a runtime-shutdown cancellation
    /// would hit in production. Distinguishes cancelled-vs-panicked so
    /// a shutdown does not turn into a false-positive error stream.
    #[tokio::test]
    #[traced_test]
    async fn cancellation_logs_at_warn_not_error() {
        let err = cancelled_join_error().await;
        assert!(
            err.is_cancelled(),
            "precondition: the JoinError is cancelled"
        );
        assert!(
            !err.is_panic(),
            "precondition: the JoinError is not a panic"
        );

        log_join_result("test::cancel_path", Err(err));

        assert!(
            logs_contain("supervised task cancelled"),
            "cancellation should log at warn"
        );
        assert!(logs_contain("test::cancel_path"));
        assert!(
            !logs_contain("supervised task panicked"),
            "cancellation must not be conflated with panic"
        );
    }

    /// Direct branch test for the panic arm of [`log_join_result`].
    /// The end-to-end test above already pins the same invariant via
    /// a live panic; this one guards against a future refactor of
    /// the helper that preserves the end-to-end path but accidentally
    /// re-routes the error/warn arms (e.g. swaps the `is_panic()`
    /// match condition).
    #[tokio::test]
    #[traced_test]
    async fn log_join_result_routes_panic_to_error_level() {
        let err = panicked_join_error().await;
        assert!(
            err.is_panic(),
            "precondition: the JoinError carries a panic"
        );

        log_join_result("test::panic_direct", Err(err));

        assert!(logs_contain("supervised task panicked"));
        assert!(logs_contain("test::panic_direct"));
        assert!(
            !logs_contain("supervised task cancelled"),
            "panic must not be conflated with cancellation"
        );
    }

    /// Context-string reach: the `context` argument appears on every
    /// log path (clean, panic, cancel). Asserted in all three prior
    /// tests; this test makes the invariant explicit for a single
    /// reader who's grep-auditing the helper. A future change that
    /// forgets to thread `context` into one of the `tracing::*` calls
    /// fails one of the three prior tests' grep assertion, but this
    /// test double-covers the clean path with a distinctive context
    /// string so the guarantee has one named test.
    #[tokio::test]
    #[traced_test]
    async fn context_string_reaches_every_tracing_event() {
        let unique_ctx = "test::context_reach_invariant";
        let handle = supervised_spawn(unique_ctx, async {});
        handle.await.expect("clean path");
        assert!(
            logs_contain(unique_ctx),
            "context string must reach the log line; helper regressed"
        );
    }

    /// JoinHandle semantics preserved: awaiting the returned handle
    /// behaves the way a plain `tokio::spawn` handle would (modulo
    /// the panic-containment guarantee). This matters for
    /// `spawn_run_reaper` which `.await`s a `Vec<JoinHandle<()>>` and
    /// expects each handle to eventually resolve with `()`.
    #[tokio::test]
    async fn join_handle_is_awaitable_and_returns_unit() {
        let handle = supervised_spawn("test::handle_semantics", async {
            tokio::time::sleep(Duration::from_millis(5)).await;
        });
        // `let _: () = handle.await?;` proves at the type level that
        // the Output is `()`; actually awaiting proves the runtime
        // can drive it to completion the same way a bare spawn handle
        // would.
        let () = handle.await.expect("handle awaits to Ok(())");
    }
}
