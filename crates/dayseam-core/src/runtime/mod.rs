//! Process-wide async-runtime helpers used by every crate that
//! spawns a background future.
//!
//! DAY-113 (F-10 follow-up). The v0.4 capstone review filed F-10
//! against `startup.rs`'s fire-and-forget orphan-secret audit: a
//! `tauri::async_runtime::spawn(audit(…))` discarded its `JoinHandle`
//! and a panic inside the audit future would have gone unlogged and
//! invisible. DAY-103 hand-wrote a supervisor for that one site. The
//! v0.5-drafting inventory found six *other* production spawn sites
//! with the same shape — a panic in the orchestrator completion task,
//! a panic in the run-forwarder, or a panic in the
//! broadcast-forwarder would each silently drop a user-visible slice
//! of the app without a single log line to explain why the UI stalled.
//!
//! [`supervised_spawn`] is the canonical shape for every *non*-panic-
//! tolerant site. A panic turns into a `tracing::error!` keyed by a
//! caller-supplied static `context` string, a cancellation logs at
//! `warn`, and a clean completion logs at `debug`. The outer
//! `JoinHandle<()>` the helper returns **never** observes the inner
//! panic — callers can `await` it safely from the reaper without
//! propagating the panic into the cleanup path.
//!
//! Three sites deliberately opt out of supervision and still use bare
//! `tokio::spawn` with a `// bare-spawn: intentional — …` marker
//! comment:
//!
//! - `apps/desktop/src-tauri/src/state.rs` `spawn_run_reaper` —
//!   the reaper *is* the supervisor; it already swallows each child's
//!   `JoinError` via `let _ = task.await` and must not itself be
//!   supervised (a reaper panicking inside a supervisor would create a
//!   loop where the cleanup never runs).
//! - `crates/dayseam-orchestrator/src/save.rs` progress+log
//!   drains — trivial `while rx.recv().await.is_some() {}` loops that
//!   exist only to keep the receiver's senders-count > 0 during a
//!   one-shot save; they cannot panic and wrapping them would allocate
//!   a second task per save call for no observable gain.
//!
//! A CI gate at `scripts/ci/no-bare-spawn.sh` fails the build if any
//! *other* non-test production file grows a bare `tokio::spawn` or
//! `tauri::async_runtime::spawn` without the marker comment — so the
//! pattern is enforced at PR time rather than rediscovered at review
//! time the way F-10 was.

pub mod supervised_spawn;

pub use supervised_spawn::supervised_spawn;
