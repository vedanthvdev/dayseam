//! The [`Orchestrator`] handle and its public request / response
//! types.
//!
//! One [`Orchestrator`] lives for the lifetime of the process. It
//! holds:
//! * the [`sqlx::SqlitePool`] every repo uses,
//! * the [`dayseam_events::AppBus`] for app-wide toasts,
//! * cloneable handles to the two registries,
//! * cheap shared dependencies the orchestrator injects into
//!   every connector call ([`connectors_sdk::HttpClient`], the
//!   [`connectors_sdk::Clock`], the [`connectors_sdk::RawStore`]),
//! * and the in-flight map used by supersede-on-retry.
//!
//! The actual lifecycle logic lives in [`crate::generate`]; this
//! module only carries the types everyone else imports.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::NaiveDate;
use connectors_sdk::{AuthStrategy, Clock, HttpClient, NoopRawStore, RawStore, SystemClock};
use dayseam_core::{
    runtime::supervised_spawn, Person, RunId, SourceId, SourceIdentity, SourceKind,
};
use dayseam_events::AppBus;
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::registries::{ConnectorRegistry, SinkRegistry};
use crate::retention::RetentionSchedule;

/// A single source the orchestrator should fan out to for a run. The
/// caller assembles these from `sources` + `source_identities` rows
/// before invoking [`Orchestrator::generate_report`] so the
/// orchestrator never touches those repos directly — that keeps the
/// per-source permission check (which lives alongside the IPC
/// boundary in a later PR) in a single place.
#[derive(Clone)]
pub struct SourceHandle {
    pub source_id: SourceId,
    pub kind: SourceKind,
    /// How this source authenticates. Cheap to clone — `AuthStrategy`
    /// is always boxed.
    pub auth: Arc<dyn AuthStrategy>,
    /// Identities that resolve to the run's [`Person`] for this
    /// source. A connector keeps an event iff at least one identity's
    /// `external_actor_id` matches the event's actor key.
    pub source_identities: Vec<SourceIdentity>,
}

impl std::fmt::Debug for SourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceHandle")
            .field("source_id", &self.source_id)
            .field("kind", &self.kind)
            .field("auth", &self.auth.name())
            .field("source_identities_len", &self.source_identities.len())
            .finish()
    }
}

/// Everything a caller supplies to [`Orchestrator::generate_report`].
#[derive(Clone, Debug)]
pub struct GenerateRequest {
    pub person: Person,
    pub sources: Vec<SourceHandle>,
    pub date: NaiveDate,
    pub template_id: String,
    pub template_version: String,
    pub verbose_mode: bool,
}

/// Terminal outcome of a [`Orchestrator::generate_report`] run.
/// Returned from the completion future so a caller can branch on
/// success vs cancelled vs failed without re-reading the DB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateOutcome {
    pub run_id: RunId,
    pub status: dayseam_core::SyncRunStatus,
    /// Populated when `status == Completed`. The draft is already
    /// persisted; the id lets callers fetch it from
    /// [`dayseam_db::DraftRepo::get`].
    pub draft_id: Option<Uuid>,
    /// Populated when `status == Cancelled`.
    pub cancel_reason: Option<dayseam_core::SyncRunCancelReason>,
}

/// Key for the in-flight map used by supersede-on-retry. Two generate
/// calls for the same key race; the newer call wins and the older
/// one is cancelled with [`dayseam_core::SyncRunCancelReason::SupersededBy`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InFlightKey {
    pub person_id: Uuid,
    pub date: NaiveDate,
    pub template_id: String,
}

/// Per-run bookkeeping stored in the in-flight map. The cancel token
/// is shared with every [`connectors_sdk::ConnCtx`] this run builds so
/// firing `cancel.cancel()` unblocks every connector at once. The
/// `run_id` is what the supersede path uses to decide whether a late
/// writer is stale: after a run's in-flight entry is replaced, the old
/// run's `run_id` no longer matches the entry, and the persist-time
/// guard in [`crate::generate`] drops its results.
#[derive(Clone, Debug)]
pub(crate) struct InFlightEntry {
    pub run_id: RunId,
    pub cancel: CancellationToken,
}

/// Orchestrator handle. Cheap to clone — every owned field is either
/// already `Clone` (`SqlitePool`, `AppBus`, registries) or wrapped in
/// an [`Arc`] (the in-flight map, the clock, the raw store).
#[derive(Clone)]
pub struct Orchestrator {
    pub(crate) pool: SqlitePool,
    /// App-wide toast bus. Unused in PR-A because the orchestrator
    /// only emits per-run progress on the `RunStreams` channels; PR-B
    /// adds toast emission when a run is superseded / cancelled by
    /// the user and uses this handle.
    #[allow(dead_code)]
    pub(crate) app_bus: AppBus,
    pub(crate) connectors: ConnectorRegistry,
    pub(crate) sinks: SinkRegistry,
    pub(crate) http: HttpClient,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) raw_store: Arc<dyn RawStore>,
    pub(crate) in_flight: Arc<Mutex<HashMap<InFlightKey, InFlightEntry>>>,
    /// Debounce guard for the post-run retention sweep hook. Shared
    /// across every clone so a cancel storm spread over many tasks
    /// still coalesces to one sweep per
    /// [`crate::retention::POST_RUN_SWEEP_MIN_INTERVAL`]. See Task 7.4
    /// (cancel-storm amplification) in the Phase 2 plan.
    pub(crate) retention_schedule: RetentionSchedule,
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("connector_kinds", &self.connectors.kinds())
            .field("sink_kinds", &self.sinks.kinds())
            .finish_non_exhaustive()
    }
}

/// Builder for an [`Orchestrator`]. Every collaborator has a sensible
/// default so a test can construct one in two lines; production sets
/// the ones it wants to override (usually just `connectors` and
/// `sinks`).
#[derive(Clone)]
pub struct OrchestratorBuilder {
    pool: SqlitePool,
    app_bus: AppBus,
    connectors: ConnectorRegistry,
    sinks: SinkRegistry,
    http: Option<HttpClient>,
    clock: Option<Arc<dyn Clock>>,
    raw_store: Option<Arc<dyn RawStore>>,
}

impl OrchestratorBuilder {
    #[must_use]
    pub fn new(
        pool: SqlitePool,
        app_bus: AppBus,
        connectors: ConnectorRegistry,
        sinks: SinkRegistry,
    ) -> Self {
        Self {
            pool,
            app_bus,
            connectors,
            sinks,
            http: None,
            clock: None,
            raw_store: None,
        }
    }

    #[must_use]
    pub fn http(mut self, http: HttpClient) -> Self {
        self.http = Some(http);
        self
    }

    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    #[must_use]
    pub fn raw_store(mut self, raw_store: Arc<dyn RawStore>) -> Self {
        self.raw_store = Some(raw_store);
        self
    }

    /// Finalise the builder. `http` defaults to
    /// [`HttpClient::new`]; `clock` defaults to [`SystemClock`];
    /// `raw_store` defaults to [`NoopRawStore`] because v0.1 does not
    /// persist raw payloads.
    pub fn build(self) -> Result<Orchestrator, dayseam_core::DayseamError> {
        let http = match self.http {
            Some(c) => c,
            None => HttpClient::new()?,
        };
        let clock: Arc<dyn Clock> = self.clock.unwrap_or_else(|| Arc::new(SystemClock));
        let raw_store: Arc<dyn RawStore> = self.raw_store.unwrap_or_else(|| Arc::new(NoopRawStore));
        Ok(Orchestrator {
            pool: self.pool,
            app_bus: self.app_bus,
            connectors: self.connectors,
            sinks: self.sinks,
            http,
            clock,
            raw_store,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            retention_schedule: RetentionSchedule::new(),
        })
    }
}

impl Orchestrator {
    /// Kick off a `generate_report` run. Returns immediately with a
    /// [`crate::generate::GenerateHandle`] carrying the `run_id`, the
    /// per-run [`dayseam_events::ProgressReceiver`] /
    /// [`dayseam_events::LogReceiver`], and a
    /// [`tokio::task::JoinHandle`] that resolves once the run
    /// terminates (one of the three terminal `SyncRunStatus`es).
    pub async fn generate_report(
        &self,
        request: GenerateRequest,
    ) -> crate::generate::GenerateHandle {
        crate::generate::start(self.clone(), request).await
    }

    /// Request that a live run be cancelled. If `run_id` is not
    /// currently in-flight (already terminated or never existed),
    /// this is a no-op — the state machine at the repo layer will
    /// keep the DB consistent regardless.
    ///
    /// This only *fires* the cancel token. The generate task itself
    /// owns the terminal transition: it observes the cancellation,
    /// marks the `SyncRun` row `Cancelled` with
    /// [`dayseam_core::SyncRunCancelReason::User`], emits the
    /// terminal [`dayseam_core::ProgressPhase::Cancelled`], and then
    /// removes its in-flight entry.
    pub async fn cancel(&self, run_id: RunId) -> bool {
        let guard = self.in_flight.lock().await;
        for entry in guard.values() {
            if entry.run_id == run_id {
                entry.cancel.cancel();
                return true;
            }
        }
        false
    }

    /// Snapshot of how many runs are currently in-flight. Used from
    /// shutdown paths and from tests.
    pub async fn in_flight_count(&self) -> usize {
        self.in_flight.lock().await.len()
    }

    /// Borrow the retention-sweep debounce guard. Tests use this to
    /// observe how many post-run sweeps the debounce actually let
    /// through; the Tauri startup path uses it to feed
    /// [`RetentionSchedule::note_external_sweep`] after the startup
    /// sweep and after a manual `retention_sweep_now`.
    #[must_use]
    pub fn retention_schedule(&self) -> &RetentionSchedule {
        &self.retention_schedule
    }

    /// Fire a retention sweep opportunistically after a terminal
    /// `generate_report` transition, subject to the debounce guard
    /// (see [`crate::retention::POST_RUN_SWEEP_MIN_INTERVAL`]).
    ///
    /// The sweep itself is spawned as a detached task so a slow
    /// `DELETE` never delays the caller's `GenerateOutcome`. Failures
    /// are logged at `warn` and otherwise swallowed — the true
    /// correctness guarantee remains the startup sweep, so a missed
    /// post-run sweep is an observability blip, not a bug.
    pub async fn maybe_sweep_after_terminal(&self) {
        let now = self.clock.now();
        if !self.retention_schedule.claim_sweep_slot(now).await {
            return;
        }
        let pool = self.pool.clone();
        // DAY-113: supervised so a panic inside
        // `sweep_with_resolved_cutoff` (e.g. an `sqlx` driver panic on a
        // corrupt row) cannot silently detach the opportunistic
        // post-run sweep. The startup sweep is still the guaranteed
        // correctness floor; supervision here just ensures the failure
        // is loud in the logs instead of invisible.
        supervised_spawn("orchestrator::retention_post_run_sweep", async move {
            if let Err(err) = crate::retention::sweep_with_resolved_cutoff(&pool, now).await {
                tracing::warn!(
                    ?err,
                    "post-run retention sweep failed; startup sweep will retry on next boot",
                );
            }
        });
    }

    /// Borrow the [`ConnectorRegistry`] this orchestrator was built
    /// with. Exposed so the Tauri layer's `sources_healthcheck`
    /// command can dispatch a probe through the same connector
    /// instance every run uses.
    #[must_use]
    pub fn connectors(&self) -> &ConnectorRegistry {
        &self.connectors
    }

    /// Borrow the [`SinkRegistry`] this orchestrator was built with.
    /// Exposed for symmetry with [`Self::connectors`]; v0.1 has no
    /// IPC command that uses it directly but adapter tooling does.
    #[must_use]
    pub fn sinks(&self) -> &SinkRegistry {
        &self.sinks
    }

    /// Borrow the shared [`HttpClient`]. Cheap to clone — every
    /// connector run already shares this instance via [`ConnCtx`].
    #[must_use]
    pub fn http_client(&self) -> &HttpClient {
        &self.http
    }

    /// Dispatch a persisted [`dayseam_core::ReportDraft`] to a
    /// configured [`dayseam_core::Sink`] and return the resulting
    /// [`dayseam_core::WriteReceipt`].
    ///
    /// Structure (Task 5 invariant #7): a failed sink write does not
    /// mutate `report_drafts.sections_json`. The orchestrator never
    /// writes to the draft row on this path; the draft stays exactly
    /// as `generate_report` left it, and any sink failure propagates
    /// to the caller unchanged.
    ///
    /// The return type is a `Vec` to match the Task 6 IPC shape
    /// (`report_save(draft_id, sink_id) -> Vec<WriteReceipt>`); in
    /// v0.1 the vec always contains exactly one element.
    pub async fn save_report(
        &self,
        draft_id: Uuid,
        sink: &dayseam_core::Sink,
    ) -> Result<Vec<dayseam_core::WriteReceipt>, dayseam_core::DayseamError> {
        crate::save::run(self, draft_id, sink).await
    }
}
