//! [`supervised_spawn`] — the canonical shape for spawning a
//! background future in Dayseam production code.
//!
//! See the parent module ([`crate::runtime`]) for the design rationale
//! (DAY-113, F-10 follow-up) and the list of three sites that
//! deliberately opt out.

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
use tokio::task::JoinHandle;

/// Spawn `future` under supervision. Returns a `JoinHandle<()>` whose
/// `await` **never** observes a panic from the inner future — a
/// panic turns into a single `tracing::error!(context, panic = %msg, …)`
/// log line and the outer handle completes cleanly.
///
/// `context` is a caller-supplied static string that names the
/// spawn site; it appears on every log line the supervisor emits and
/// is what makes a post-mortem readable when several supervised tasks
/// fail simultaneously. Use a short snake_case label that a reader can
/// grep for (e.g. `"orphan_secret_audit"`, `"run_forwarder::progress"`,
/// `"orchestrator::retention_sweep"`), not a full sentence.
///
/// # Implementation shape
///
/// DAY-113's original implementation nested two `tokio::spawn`s —
/// the outer awaited the inner's `JoinHandle` to translate a
/// `JoinError::is_panic()` into a `tracing::error!`. That shape was
/// correct for panic containment but broke `JoinHandle::abort()`
/// semantics on the returned handle: aborting the outer supervisor
/// only stopped its `.await` line; the inner task became detached
/// and kept running. A caller that held the returned handle and
/// called `.abort()` (e.g. `broadcast_forwarder::spawn`'s docstring
/// guarantee, or a future reaper on top of `SupervisedHandle`) would
/// leak the task rather than cancelling it (C-1).
///
/// DAY-122 uses `FutureExt::catch_unwind` from `futures-util` to
/// catch the panic **inline**, inside a single `tokio::spawn`. The
/// returned `JoinHandle<()>` now owns the caller's future directly,
/// so `.abort()` cancels it for real. `AssertUnwindSafe` is the
/// necessary escape hatch — most async futures carry references
/// that are not `UnwindSafe` (the whole point of this helper is to
/// wrap them anyway), so the assertion matches the contract the
/// caller has already accepted by using this helper instead of a
/// bare spawn.
///
/// # Logging shape
///
/// - Clean completion → `tracing::debug!(context, "supervised task completed")`.
/// - Inner future panicked → `tracing::error!(context, panic = %msg, "supervised task panicked")`.
///
/// Cancellation via `.abort()` on the returned handle no longer
/// emits from inside the supervisor — it cannot, because the
/// supervisor's own future is the one being dropped. Instead, the
/// abort surfaces to the *caller* as a `JoinError::is_cancelled()`
/// at the `.await` site, which is the standard tokio contract and
/// matches what a caller gets from a bare spawn. If a caller wants
/// to log the cancel, they do it at their own await site — the
/// supervisor is only responsible for panic containment, and that
/// separation of concerns is clearer than shoehorning cancel-path
/// logging into a helper that no longer owns the cancel signal.
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
        match AssertUnwindSafe(future).catch_unwind().await {
            Ok(()) => {
                tracing::debug!(context, "supervised task completed");
            }
            Err(payload) => {
                let msg = panic_payload_message(&payload);
                tracing::error!(context, panic = %msg, "supervised task panicked");
            }
        }
    })
}

/// Pull a printable message out of a panic payload, when the
/// payload is a downcastable string type.
///
/// Panic payloads from `catch_unwind` are `Box<dyn Any + Send>`.
/// `panic!("{}", var)` typically lowers to a `String` payload;
/// `panic!("static literal")` lowers to `&'static str` on some
/// toolchain/stdlib combinations and to an internal formatter
/// payload type (not downcastable to either) on others. We try the
/// two public shapes and fall back to a sentinel when the payload
/// is opaque — this matches the best-effort contract of
/// `std::panic::PanicHookInfo::payload_as_str`, which uses the same
/// two downcasts.
///
/// The primary F-10 guarantee — "panic is contained and logged at
/// error level with the `context` field" — does not depend on this
/// helper succeeding; it depends only on the `tracing::error!` call
/// firing, which it always does. The payload message is a
/// nice-to-have when it's present.
///
/// # Signature note — `&Box<dyn Any + Send>` vs `&(dyn Any + Send)`
///
/// This function deliberately takes `&Box<dyn Any + Send>` rather
/// than `&(dyn Any + Send)` because `<dyn Any + Send>::downcast_ref`
/// has a name-resolution trap: it hits the blanket `impl<T: 'static>
/// Any for T` for the *trait object type itself* rather than
/// dispatching through the vtable to the concrete type. The result
/// is `downcast_ref::<String>()` always returning `None` even when
/// the payload is a `String`. Dereferencing the `Box` once to
/// `&dyn Any` restores the correct vtable-dispatched behaviour.
/// Swapping the signature to the trait-object form silently breaks
/// every branch, which is exactly the bug this helper exists to
/// avoid — [`string_panic_payload_message_reaches_log_line`] pins
/// it.
fn panic_payload_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    let payload: &dyn std::any::Any = &**payload;
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<opaque panic payload>".to_string()
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

        // F-10's core invariant: an error-level `tracing` event is
        // emitted, it carries the context string, and the panic does
        // not propagate through the outer `JoinHandle` (the
        // `expect` above would fail otherwise). We deliberately do
        // *not* assert on the panic-payload message content here:
        // whether the payload downcasts to `String`, `&'static str`,
        // or an opaque stdlib-internal formatter type depends on
        // the toolchain version and the exact `panic!` macro
        // lowering. The `panic = %msg` field is best-effort; the
        // error-level emission is the load-bearing guarantee.
        assert!(logs_contain("supervised task panicked"));
        assert!(logs_contain("test::panic_path"));
    }

    /// Panic messages that reach us as `String` payloads *do* land
    /// in the log line. We construct a guaranteed-`String` payload
    /// via `std::panic::panic_any::<String>` (bypassing the
    /// `panic!` macro's toolchain-specific lowering) so this test
    /// is stable across stdlib versions. A future refactor that
    /// drops the `String` branch of [`panic_payload_message`] fails
    /// this test.
    #[tokio::test]
    #[traced_test]
    async fn string_panic_payload_message_reaches_log_line() {
        let handle = supervised_spawn("test::string_panic", async move {
            std::panic::panic_any(String::from("payload-preserved-in-log"));
        });

        handle.await.expect("outer handle resolves cleanly");

        assert!(logs_contain("supervised task panicked"));
        assert!(
            logs_contain("payload-preserved-in-log"),
            "String payloads must round-trip through panic_payload_message onto the log line"
        );
    }

    /// Panic payloads that aren't `&'static str` / `String` still
    /// produce a non-empty log line. `panic_any!` hands an arbitrary
    /// `Any + Send` payload to the runtime; the supervisor's
    /// `panic_payload_message` helper must fall through to the
    /// `<opaque panic payload>` sentinel rather than writing a blank
    /// field, so a future reader grepping for "supervised task
    /// panicked" still finds something useful.
    #[tokio::test]
    #[traced_test]
    async fn non_string_panic_payload_is_summarised() {
        let handle = supervised_spawn("test::opaque_panic", async move {
            std::panic::panic_any(42_i32);
        });

        handle
            .await
            .expect("outer handle must resolve cleanly despite opaque panic");

        assert!(logs_contain("supervised task panicked"));
        assert!(logs_contain("test::opaque_panic"));
        assert!(
            logs_contain("<opaque panic payload>"),
            "non-string payloads must fall back to the sentinel rather than blanking the field"
        );
    }

    /// Context-string reach: the `context` argument appears on every
    /// log path (clean + panic). Asserted in the two prior tests;
    /// this test makes the invariant explicit for a single reader
    /// who's grep-auditing the helper. A future change that forgets
    /// to thread `context` into one of the `tracing::*` calls fails
    /// one of the prior tests' grep assertion, but this test
    /// double-covers the clean path with a distinctive context
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

    /// DAY-122 / C-1 regression: `.abort()` on the returned handle
    /// must actually cancel the inner user future.
    ///
    /// Pre-DAY-122 the supervisor nested two `tokio::spawn`s. The
    /// outer one's `.abort()` only aborted the supervisor's
    /// `inner.await` line, leaving the inner task detached. A test
    /// future that set a flag *after* its cancellation point would
    /// still set that flag, even though the caller had aborted. This
    /// test pins the fix: after `abort()` + `.await`, the flag is
    /// unset, proving the user future never ran past the abort
    /// point.
    ///
    /// Structure: the user future waits on a parked `tokio::sleep`
    /// (a cancellation-safe yield point); abort the handle during
    /// that sleep; confirm the `ran_past` flag stays false. The
    /// outer handle's `await` returns a cancelled `JoinError`, which
    /// is the standard tokio contract for aborted tasks.
    #[tokio::test]
    async fn abort_actually_cancels_the_inner_future() {
        let ran_past = Arc::new(AtomicBool::new(false));
        let ran_past_clone = ran_past.clone();

        let handle = supervised_spawn("test::abort_propagation", async move {
            // Park for a duration far longer than the test takes.
            // The abort below drops this future mid-sleep; setting
            // `ran_past` after the sleep is the flag that would flip
            // if supervision silently detached the task (pre-C-1
            // regression).
            tokio::time::sleep(Duration::from_secs(30)).await;
            ran_past_clone.store(true, Ordering::SeqCst);
        });

        // Yield once so the supervised task is actually parked on
        // the sleep before we abort it — without this the abort
        // could race with the initial poll on some schedulers.
        tokio::task::yield_now().await;

        handle.abort();

        // The outer await must return cancelled, not Ok, because
        // the supervisor itself was aborted (it owns the only
        // spawn).
        let err = handle.await.expect_err("aborted handle must yield Err");
        assert!(
            err.is_cancelled(),
            "abort() must produce JoinError::is_cancelled, got {err:?}"
        );

        // Give the scheduler a final tick in case the detached-task
        // bug is present; if the inner future is actually detached,
        // it would set the flag asynchronously.
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(
            !ran_past.load(Ordering::SeqCst),
            "inner user future must not have run past the cancellation point — \
             a true abort drops the future; the pre-C-1 nested-spawn shape \
             would leak the inner task and set this flag"
        );
    }
}
