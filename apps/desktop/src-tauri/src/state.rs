//! Process-wide state held by the Tauri runtime.
//!
//! Everything the IPC layer needs to serve a command lives here: the
//! single SQLite pool, the app-wide broadcast bus, the secret store,
//! and the per-run registry that tracks which sync runs are currently
//! streaming events to the frontend.
//!
//! `AppState` is owned by Tauri via [`tauri::Manager::manage`] and is
//! accessed from every `#[tauri::command]` through
//! `tauri::State<'_, AppState>`. That's the only way state leaks out
//! of this module.

use std::collections::HashMap;
use std::sync::Arc;

use connectors_sdk::HttpClient;
use dayseam_core::RunId;
use dayseam_events::AppBus;
use dayseam_orchestrator::Orchestrator;
use dayseam_secrets::SecretStore;
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Per-run bookkeeping used by the IPC layer: the cancellation token
/// that aborts the run and the reaper task that waits for every
/// forwarder + producer to finish before deregistering the run.
///
/// Keeping only the reaper (rather than every spawned task
/// individually) means a shutdown path that wants to wait for runs to
/// drain only has to await one `JoinHandle` per active run.
#[derive(Debug)]
pub struct RunHandle {
    pub run_id: RunId,
    pub cancel: CancellationToken,
    /// Reaper task — completes once all of the run's spawned tasks
    /// have finished and the registry entry has been removed. `None`
    /// is only used by unit tests that construct a handle without a
    /// real reaper.
    pub reaper: Option<JoinHandle<()>>,
}

/// Registry of currently-live sync runs keyed by [`RunId`].
///
/// Registration happens when a command that starts a run (for Phase 1
/// that's `dev_start_demo_run`; Phase 2 adds `run_start`) allocates a
/// fresh `RunStreams` and spawns forwarder tasks. Deregistration
/// happens when those forwarders observe their receivers returning
/// `None`, which is how run completion is signalled end-to-end.
#[derive(Debug, Default)]
pub struct RunRegistry {
    runs: HashMap<RunId, RunHandle>,
}

impl RunRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a new run handle. Returns the previous handle for
    /// `run_id` if one existed — which should never happen given
    /// `RunId::new()` uses v4 UUIDs, but surfacing the collision is
    /// cheaper than silently clobbering the prior run.
    pub fn insert(&mut self, handle: RunHandle) -> Option<RunHandle> {
        self.runs.insert(handle.run_id, handle)
    }

    pub fn remove(&mut self, run_id: &RunId) -> Option<RunHandle> {
        self.runs.remove(run_id)
    }

    #[must_use]
    pub fn contains(&self, run_id: &RunId) -> bool {
        self.runs.contains_key(run_id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Request every live run to cancel. Used on app shutdown so
    /// in-flight forwarders observe the cancellation signal and exit
    /// promptly instead of stalling the quit.
    pub fn cancel_all(&self) {
        for handle in self.runs.values() {
            handle.cancel.cancel();
        }
    }
}

/// Process-wide state. Cheap to share — every field is either cheaply
/// cloneable (`SqlitePool`, `AppBus`) or behind an `Arc` / `RwLock`.
///
/// `runs` is an `Arc<RwLock<RunRegistry>>` specifically so a run's
/// reaper task can hold its own clone of the registry handle and
/// remove the run once all forwarders + producer have finished.
/// Without the `Arc`, the reaper would have no way back into the
/// registry without going through `AppHandle::state()` on every
/// wake-up.
pub struct AppState {
    pub pool: SqlitePool,
    pub app_bus: AppBus,
    pub secrets: Arc<dyn SecretStore>,
    pub runs: Arc<RwLock<RunRegistry>>,
    /// The single process-wide [`Orchestrator`] every IPC command
    /// that starts or saves a report routes through. Cheap to clone
    /// — each field inside is either already `Clone` or `Arc`-wrapped
    /// — so commands can pull a fresh handle per call without
    /// contending on `AppState`. Task 5 PR-A constructed the type;
    /// PR-B wires it onto `AppState` so Task 6's `report_generate` /
    /// `report_save` have a place to land.
    pub orchestrator: Orchestrator,
    /// Process-wide [`HttpClient`] every IPC command that hits HTTP
    /// pulls from — validate-credentials, reconnect, and future
    /// IPC-initiated probes — so the dialog path uses the same
    /// retry / jitter / cancellation contract the walker path does,
    /// and so `tests/reconnect_rebind.rs` can inject a wiremock-
    /// backed client without monkey-patching a `HttpClient::new()`
    /// call buried inside each command.
    ///
    /// `HttpClient` is not a trait — it's a concrete struct whose
    /// inner `reqwest::Client` is already `Arc`-backed, so `Clone`
    /// is cheap and both `Send + Sync + 'static`. Storing the
    /// concrete type rather than an `Arc<dyn HttpClient>` (as the
    /// DAY-111 plan originally called for) keeps dispatch monomorphic
    /// and avoids the trait-object shape there's no second
    /// implementation to justify.
    pub http: HttpClient,
}

impl AppState {
    /// Construct an [`AppState`] from its collaborators. Keep this a
    /// plain constructor — wiring the pool, keychain, HTTP client,
    /// and orchestrator is the responsibility of [`crate::startup`].
    #[must_use]
    pub fn new(
        pool: SqlitePool,
        app_bus: AppBus,
        secrets: Arc<dyn SecretStore>,
        orchestrator: Orchestrator,
        http: HttpClient,
    ) -> Self {
        Self {
            pool,
            app_bus,
            secrets,
            runs: Arc::new(RwLock::new(RunRegistry::new())),
            orchestrator,
            http,
        }
    }

    /// Test-only constructor that lets `tests/reconnect_rebind.rs`
    /// (DAY-111 / TST-v0.4-04) build an `AppState` around a
    /// wiremock-backed [`HttpClient`] without duplicating the
    /// orchestrator / registry wiring that `startup::build_app_state`
    /// owns. Gated behind the `test-helpers` feature so production
    /// binaries cannot reach it; intra-crate `#[cfg(test)]` unit
    /// tests also get access without flipping a feature. The
    /// constructor deliberately takes a ready-made `HttpClient` so
    /// each test can decide whether to reuse the default retry
    /// policy or swap in `RetryPolicy::instant()` — letting the
    /// suite cover retry behaviour without wall-clock delays.
    #[cfg(any(test, feature = "test-helpers"))]
    #[must_use]
    pub fn with_http_for_test(
        pool: SqlitePool,
        app_bus: AppBus,
        secrets: Arc<dyn SecretStore>,
        orchestrator: Orchestrator,
        http: HttpClient,
    ) -> Self {
        Self::new(pool, app_bus, secrets, orchestrator, http)
    }
}

/// Spawn a reaper task that waits for every handle in `tasks` to
/// finish and then deregisters `run_id` from `registry`.
///
/// This is how the run lifecycle closes the loop end-to-end: when the
/// producer closes its senders, the forwarders' receivers return
/// `None`, the forwarders exit, the reaper's `join_all` completes,
/// and the run disappears from the registry. Without this, finished
/// runs pile up — holding their cancellation tokens and task handles
/// forever — and the app leaks roughly one `RunHandle` per sync.
///
/// Returns the reaper's own `JoinHandle` so a future `shutdown` path
/// can await it if desired. Callers that don't need the handle can
/// drop it without panicking; the task is detached and self-contained.
pub fn spawn_run_reaper(
    registry: Arc<RwLock<RunRegistry>>,
    run_id: RunId,
    tasks: Vec<JoinHandle<()>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        for task in tasks {
            // `Err` here means the task panicked or was aborted; in
            // either case the cleanup still has to happen, so we
            // swallow the error rather than propagate it.
            let _ = task.await;
        }
        let mut guard = registry.write().await;
        guard.remove(&run_id);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(run_id: RunId) -> RunHandle {
        RunHandle {
            run_id,
            cancel: CancellationToken::new(),
            reaper: None,
        }
    }

    #[test]
    fn insert_then_remove_round_trips() {
        let mut reg = RunRegistry::new();
        let id = RunId::new();
        reg.insert(handle(id));
        assert!(reg.contains(&id));
        assert_eq!(reg.len(), 1);
        assert!(reg.remove(&id).is_some());
        assert!(reg.is_empty());
    }

    #[test]
    fn cancel_all_flips_every_token() {
        let mut reg = RunRegistry::new();
        let a = handle(RunId::new());
        let b = handle(RunId::new());
        let tok_a = a.cancel.clone();
        let tok_b = b.cancel.clone();
        reg.insert(a);
        reg.insert(b);

        reg.cancel_all();
        assert!(tok_a.is_cancelled());
        assert!(tok_b.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reaper_removes_run_when_tasks_finish() {
        let registry = Arc::new(RwLock::new(RunRegistry::new()));
        let run_id = RunId::new();
        {
            let mut guard = registry.write().await;
            guard.insert(handle(run_id));
        }

        // Three trivial tasks — the reaper waits on all of them.
        let tasks = (0..3).map(|_| tokio::spawn(async {})).collect();
        let reaper = spawn_run_reaper(registry.clone(), run_id, tasks);
        reaper.await.expect("reaper joined");

        let guard = registry.read().await;
        assert!(
            !guard.contains(&run_id),
            "reaper should have removed the run"
        );
        assert!(guard.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reaper_removes_run_even_when_a_task_panics() {
        let registry = Arc::new(RwLock::new(RunRegistry::new()));
        let run_id = RunId::new();
        {
            let mut guard = registry.write().await;
            guard.insert(handle(run_id));
        }

        let panicking = tokio::spawn(async { panic!("task panic on purpose") });
        let ok = tokio::spawn(async {});
        let reaper = spawn_run_reaper(registry.clone(), run_id, vec![panicking, ok]);
        reaper.await.expect("reaper joined");

        let guard = registry.read().await;
        assert!(
            !guard.contains(&run_id),
            "reaper must tolerate panicking children"
        );
    }
}
