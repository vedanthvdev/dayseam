//! The `generate_report` lifecycle.
//!
//! [`start`] is the single entry point, called by
//! [`crate::Orchestrator::generate_report`]. It:
//!
//! 1. Builds a fresh [`RunId`] and the matching
//!    [`RunStreams`](dayseam_events::RunStreams).
//! 2. Honours supersede-on-retry by cancelling any older run for the
//!    same `(person_id, date, template_id)` tuple and recording the
//!    replacement in the in-flight map.
//! 3. Inserts the `sync_runs` row (status `Running`).
//! 4. Spawns a background task that fans out over the requested
//!    sources with bounded parallelism, collects per-source results,
//!    renders the draft, persists it, and transitions the run to
//!    `Completed` / `Cancelled` / `Failed`.
//! 5. Returns a [`GenerateHandle`] carrying the receiver halves of
//!    the per-run streams plus a [`tokio::task::JoinHandle`] that
//!    resolves to a [`GenerateOutcome`] when the run terminates.
//!
//! Every transition goes through [`dayseam_db::SyncRunRepo`] so the
//! state machine lives in one place. The persist-time guard ("drop
//! late writes") reads the in-flight map by key before every mutation;
//! if the entry no longer matches the task's own `run_id`, the task
//! exits its persistence path and lets the newer task own the final
//! state.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use connectors_sdk::{ConnCtx, SyncRequest, SyncResult};
use dayseam_core::{
    error_codes, ActivityEvent, Artifact, DayseamError, PerSourceState, Person, ProgressPhase,
    RunId, RunStatus, SourceId, SourceIdentity, SourceRunState, SyncRun, SyncRunCancelReason,
    SyncRunStatus, SyncRunTrigger,
};
use dayseam_events::{LogReceiver, ProgressReceiver, ProgressSender, RunStreams};
use dayseam_report::{pipeline, MergeRequestArtifact, ReportInput, DEV_EOD_TEMPLATE_ID};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::orchestrator::{
    GenerateOutcome, GenerateRequest, InFlightEntry, InFlightKey, Orchestrator, SourceHandle,
};

/// Returned synchronously from [`start`] so the caller can forward
/// per-run streams to the UI and await the terminal outcome on its
/// own terms.
#[derive(Debug)]
pub struct GenerateHandle {
    /// The id the orchestrator allocated for this run. Every
    /// [`dayseam_events::ProgressEvent`] and
    /// [`dayseam_events::LogEvent`] on the returned receivers carries
    /// the same id.
    pub run_id: RunId,
    pub progress_rx: ProgressReceiver,
    pub log_rx: LogReceiver,
    /// Shared cancel token. Firing it has the same effect as calling
    /// [`Orchestrator::cancel`](crate::Orchestrator::cancel); exposing
    /// it directly lets the caller cancel without looking the run up
    /// in the in-flight map first.
    pub cancel: CancellationToken,
    /// Resolves once the run terminates (one of
    /// `Completed | Cancelled | Failed`). Never panics — background
    /// errors are captured in [`GenerateOutcome`].
    pub completion: JoinHandle<GenerateOutcome>,
}

/// Per-source result collected during fan-out. Separate from
/// [`SourceRunState`] so the orchestrator can carry the (potentially
/// very large) `events` / `artifacts` vectors through the render
/// stage without reshaping them into a DTO.
#[derive(Debug)]
struct PerSourceResult {
    source_id: SourceId,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    outcome: Result<SyncResult, DayseamError>,
}

pub async fn start(orch: Orchestrator, request: GenerateRequest) -> GenerateHandle {
    let run_id = RunId::new();
    // Canonical orchestrator stream construction (ARC-03): both
    // `generate_report` and `save_report` obtain their four channel
    // halves through `RunStreams::with_progress`. The grep test in
    // `tests/no_inline_run_streams_construction.rs` holds the line
    // so a future refactor can't silently reintroduce
    // `RunStreams::new` + `.split()` boilerplate that drifts between
    // the two paths.
    let (progress_tx, log_tx, progress_rx, log_rx) = RunStreams::with_progress(run_id);
    let cancel = CancellationToken::new();

    // Honour supersede-on-retry and install the new in-flight entry
    // atomically so an older task that wakes from its cancel token
    // always observes the newer entry (never an empty slot) when it
    // reaches its persist-time guard.
    //
    // Ordering matters on two axes:
    // * the new entry goes into the map first (under the mutex)
    //   so the older task's `superseded_by_now` check always sees
    //   us and reports `SupersededBy(new_run_id)` instead of
    //   `User`;
    // * the `sync_runs.superseded_by` FK forces us to insert the
    //   new `sync_runs` row *before* the older row can be
    //   transitioned to `Cancelled { SupersededBy(new_run_id) }`.
    //
    // We therefore swap the in-flight map first, fire the older
    // task's cancel, insert the new row, and only then write the
    // older row's terminal transition.
    let key = InFlightKey {
        person_id: request.person.id,
        date: request.date,
        template_id: request.template_id.clone(),
    };
    let prior_in_flight = swap_in_flight_and_cancel(&orch, &key, run_id, cancel.clone()).await;

    // Insert the `sync_runs` row synchronously before the
    // background task starts so a caller that awaits
    // `in_flight_count` immediately after `generate_report` can
    // always see the run. The repo rejects any non-`Running`
    // initial status, so the orchestrator never writes any other.
    let started_at = Utc::now();
    let syncrun = SyncRun {
        id: run_id,
        started_at,
        finished_at: None,
        trigger: SyncRunTrigger::User,
        status: SyncRunStatus::Running,
        cancel_reason: None,
        superseded_by: None,
        per_source_state: Vec::new(),
    };
    let syncrun_repo = dayseam_db::SyncRunRepo::new(orch.pool.clone());
    // Insert the new row first so the FK on `sync_runs.superseded_by`
    // holds when we mark the older row `SupersededBy(run_id)`.
    if let Err(e) = syncrun_repo.insert(&syncrun).await {
        // A DB failure at the very first mutation is a hard error.
        // Drain the in-flight entry we inserted a moment ago so a
        // retry for the same tuple can proceed, and return a handle
        // whose `completion` future resolves immediately to
        // `Failed`. If we also fired an older run's cancel token as
        // part of supersede, we leave that row alone — the older
        // task's own terminate_cancelled path will mark it
        // `Cancelled { User }` because `superseded_by_now` now sees
        // an empty slot.
        orch.in_flight.lock().await.remove(&key);
        // Drop the senders so the returned receivers observe a closed
        // channel immediately: the caller's UI renders the `Failed`
        // terminal state without waiting on a stream that will never
        // emit.
        drop(progress_tx);
        drop(log_tx);
        // DAY-113: the early-insert-failure path still returns a
        // JoinHandle<GenerateOutcome>, which `supervised_spawn` cannot
        // model because it constrains `Output = ()`. The future here
        // is trivially panic-free (a `tracing::error!` plus a struct
        // literal), so we retain a bare `tokio::spawn` with the
        // marker comment the CI gate recognises. If the body ever
        // grows any call that could panic (e.g. a serde map), move it
        // into a wrapper that produces `()` and emits the outcome via
        // a oneshot channel so supervision can be reintroduced.
        // bare-spawn: intentional — typed JoinHandle required, body is panic-free, see DAY-113.
        let completion = tokio::spawn(async move {
            tracing::error!(
                run_id = %run_id,
                error = %e,
                "failed to insert sync_runs row — run terminating as Failed"
            );
            GenerateOutcome {
                run_id,
                status: SyncRunStatus::Failed,
                draft_id: None,
                cancel_reason: None,
            }
        });
        return GenerateHandle {
            run_id,
            progress_rx,
            log_rx,
            cancel,
            completion,
        };
    }

    // Now that the new row exists, we can safely transition the
    // older run's row to `Cancelled { SupersededBy(run_id) }` —
    // the FK on `sync_runs.superseded_by` is satisfied.
    let superseded = if let Some(prior_run_id) = prior_in_flight {
        mark_prior_superseded(&orch, prior_run_id, run_id).await;
        Some(prior_run_id)
    } else {
        None
    };

    // The senders go to the background task (it owns progress
    // emission end-to-end); the receivers are handed back to the
    // caller via `GenerateHandle` further down.

    // Snapshot the state the background task needs. `orch` is
    // already cheap to clone.
    let orch_bg = orch.clone();
    let key_bg = key.clone();
    let cancel_bg = cancel.clone();
    let progress_bg = progress_tx.clone();

    // DAY-113: the caller (`GenerateHandle.completion`) awaits a
    // `JoinHandle<GenerateOutcome>`, so this site cannot use
    // `supervised_spawn` (which returns `JoinHandle<()>`). A panic
    // inside `run_background` still surfaces to the caller as
    // `JoinError { is_panic: true }` via
    // `apps/desktop/src-tauri/src/ipc/commands.rs::dev_generate_report`,
    // where the completion-task supervisor *is* in place and logs the
    // panic with `context = "commands::report_completion"`. That
    // means the panic is observable end-to-end even without
    // supervision here; what supervision here would add is a
    // log line *before* the `JoinError` surfaces to the caller,
    // which duplicates what the caller's supervisor already does.
    // bare-spawn: intentional — typed JoinHandle<GenerateOutcome> required, caller supervises, see DAY-113.
    let completion = tokio::spawn(async move {
        let outcome = run_background(
            orch_bg.clone(),
            request,
            run_id,
            started_at,
            key_bg,
            cancel_bg,
            progress_bg,
            log_tx,
            superseded,
        )
        .await;
        // Opportunistic retention sweep on every terminal transition.
        // The debounce guard inside `maybe_sweep_after_terminal`
        // coalesces a cancel storm to a single `DELETE` (Task 7.4).
        // The sweep itself is spawned detached, so this await is
        // cheap — it only records the debounce bookkeeping.
        orch_bg.maybe_sweep_after_terminal().await;
        outcome
    });

    GenerateHandle {
        run_id,
        progress_rx,
        log_rx,
        cancel,
        completion,
    }
}

/// Atomically replace any older in-flight entry for `key` with the
/// newer one and fire the older run's cancel token. Returns the
/// older run's id so [`mark_prior_superseded`] can later write its
/// terminal row once the newer `sync_runs` row is on disk (the FK
/// on `sync_runs.superseded_by` requires that ordering).
///
/// Atomicity matters: the newer entry MUST be visible in the
/// in-flight map before the older task's cancel token resolves,
/// otherwise the older task's `superseded_by_now` check sees an
/// empty slot and concludes it was user-cancelled. We install the
/// new entry under the map mutex and fire the old cancel only after
/// the lock is dropped.
async fn swap_in_flight_and_cancel(
    orch: &Orchestrator,
    key: &InFlightKey,
    new_run_id: RunId,
    new_cancel: CancellationToken,
) -> Option<RunId> {
    let prior = {
        let mut guard = orch.in_flight.lock().await;
        let prior = guard.remove(key);
        guard.insert(
            key.clone(),
            InFlightEntry {
                run_id: new_run_id,
                cancel: new_cancel,
            },
        );
        prior
    };
    let prior = prior?;
    prior.cancel.cancel();
    Some(prior.run_id)
}

/// Mark the prior (now-superseded) run's `sync_runs` row
/// `Cancelled { SupersededBy(new_run_id) }`. Idempotent-ish: if the
/// older task has already rewritten the row (unlikely but possible),
/// the repo refuses the transition and we log at debug.
async fn mark_prior_superseded(orch: &Orchestrator, prior_run_id: RunId, new_run_id: RunId) {
    let repo = dayseam_db::SyncRunRepo::new(orch.pool.clone());
    let reason = SyncRunCancelReason::SupersededBy { run_id: new_run_id };
    if let Err(e) = repo
        .mark_cancelled(&prior_run_id, Utc::now(), reason, &[])
        .await
    {
        tracing::debug!(
            old_run_id = %prior_run_id,
            new_run_id = %new_run_id,
            error = %e,
            "superseded run could not be marked cancelled \
             (likely already terminal); continuing"
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_background(
    orch: Orchestrator,
    request: GenerateRequest,
    run_id: RunId,
    started_at: DateTime<Utc>,
    key: InFlightKey,
    cancel: CancellationToken,
    progress: ProgressSender,
    logs: dayseam_events::LogSender,
    _superseded_prior: Option<RunId>,
) -> GenerateOutcome {
    progress.send(
        None,
        ProgressPhase::Starting {
            message: format!(
                "Generating report for {} ({} source{})",
                request.date,
                request.sources.len(),
                if request.sources.len() == 1 { "" } else { "s" },
            ),
        },
    );

    // Fan out. Each source runs concurrently but the pool is capped
    // at `min(n, 4)` to avoid stampeding the underlying system on
    // days with many sources. `JoinSet` plus a `Semaphore` keeps
    // this bounded without reaching for an external dep.
    let per_source = fan_out(
        orch.clone(),
        request.person.clone(),
        run_id,
        request.date,
        request.sources.clone(),
        progress.clone(),
        logs.clone(),
        cancel.clone(),
    )
    .await;

    // Summarise fan-out progress on the run-wide stream.
    let total = per_source.len() as u32;
    progress.send(
        None,
        ProgressPhase::InProgress {
            completed: total,
            total: Some(total),
            message: format!(
                "Fetched from {total} source{}",
                if total == 1 { "" } else { "s" }
            ),
        },
    );

    // Was the run cancelled while fan-out was running? The terminal
    // path is different: we mark the row `Cancelled`, emit the
    // matching terminal progress phase, and never render or persist
    // a draft. Distinguish supersede-cancel from user-cancel by
    // checking the in-flight map: if a newer run owns the key, the
    // supersede path already wrote our terminal row with
    // `SupersededBy`, and we must neither retransition nor emit
    // `User` on the outcome we hand back to the caller.
    if cancel.is_cancelled() {
        let superseded_by = superseded_by_now(&orch, &key, run_id).await;
        return terminate_cancelled(&orch, run_id, &key, &progress, per_source, superseded_by)
            .await;
    }

    // Build the report input. Every source's `SourceRunState` is
    // carried through even if its fetch failed: the report engine
    // surfaces failed sources inline so the user sees "GitLab
    // failed, local git is still complete" rather than a silently
    // incomplete report.
    let (events, artifacts, per_source_state, per_source_syncrun) = split_fan_out(per_source);

    // DAY-78 pipeline: dedup → extract ticket keys → annotate
    // Jira transitions with their triggering MR → annotate
    // rolled-into-MR. All four passes are pure and run before
    // `activity_events` is persisted so the on-disk table never
    // carries two rows for the same SHA (which would re-inflate
    // the bullet count on a later regen via the
    // `INSERT OR IGNORE` path). Single-source inputs (v0.1
    // local-git-only deployments) walk every pass as a no-op:
    // dedup emits input unchanged when no SHA collides, ticket-key
    // extraction is gated by title content, transition-annotation
    // is gated by the presence of `JiraIssueTransitioned` events,
    // and rolled-into-MR is gated by a non-empty MR list. See
    // [`dayseam_report::pipeline`] for the full contract.
    let mrs = collect_mr_artifacts(&events);
    let events = pipeline(events, &mrs);

    // Persist the raw `activity_events` to disk before render. The
    // evidence popover in the UI hydrates `ReportDraft::evidence`
    // event ids via `activity_events_get`; without this call the
    // ids land in the persisted draft but point at rows that were
    // never written, and every bullet shows "events are no longer
    // on disk". Pre-DAY-52 this was a silent Phase 2 bug because the
    // rendered draft already carried the visible bullet text; DAY-52
    // made every bullet clickable so the gap is now surfaced as a
    // UX regression. `insert_many` is `INSERT OR IGNORE`, so a
    // regeneration of the same day against the same source is a
    // no-op rather than a constraint violation (connectors assign
    // deterministic ids keyed off `external_id`). A failure here
    // falls through to `Failed` the same way a draft insert would —
    // a draft whose evidence can't be hydrated is not a draft.
    if !events.is_empty() {
        let activity_repo = dayseam_db::ActivityRepo::new(orch.pool.clone());
        if let Err(e) = activity_repo.insert_many(&events).await {
            tracing::error!(
                run_id = %run_id,
                error = %e,
                "activity_events insert failed — run terminating as Failed",
            );
            return terminate_failed(
                &orch,
                run_id,
                &key,
                &progress,
                per_source_syncrun,
                &format!("activity_events insert failed: {e}"),
            )
            .await;
        }
    }

    let source_identities: Vec<SourceIdentity> = request
        .sources
        .iter()
        .flat_map(|s| s.source_identities.clone())
        .collect();

    // DAY-104: materialise the `SourceId → SourceKind` map the
    // render engine uses to stamp each bullet with its forge. The
    // handles already carry both — we're just shaping them into
    // the lookup the engine expects so it doesn't have to walk
    // `request.sources` linearly per bullet. Collision is
    // impossible: `SourceHandle.source_id` is a `Uuid` primary
    // key, unique by construction.
    let source_kinds: std::collections::HashMap<_, _> = request
        .sources
        .iter()
        .map(|s| (s.source_id, s.kind))
        .collect();

    let draft_id = Uuid::new_v4();
    let generated_at = Utc::now();
    let input = ReportInput {
        id: draft_id,
        date: request.date,
        template_id: request.template_id.clone(),
        template_version: request.template_version.clone(),
        person: request.person,
        source_identities,
        events,
        artifacts,
        per_source_state,
        source_kinds,
        verbose_mode: request.verbose_mode,
        generated_at,
    };

    let draft = match dayseam_report::render(input) {
        Ok(draft) => draft,
        Err(e) => {
            tracing::error!(
                run_id = %run_id,
                error = %e,
                "report engine rejected input — run terminating as Failed",
            );
            return terminate_failed(
                &orch,
                run_id,
                &key,
                &progress,
                per_source_syncrun,
                &format!("render failed: {e}"),
            )
            .await;
        }
    };

    // Persist-time guard (Invariant #2): if our in-flight entry
    // has been replaced while we were rendering, a newer run has
    // superseded us. Drop the draft, skip the `Completed`
    // transition, and return `Cancelled{SupersededBy(new_run_id)}`
    // so the caller can surface "you started a newer run".
    if let Some(new_run_id) = superseded_by_now(&orch, &key, run_id).await {
        progress.send(
            None,
            ProgressPhase::Cancelled {
                message: format!("Superseded by a newer run ({new_run_id}); results dropped"),
            },
        );
        return GenerateOutcome {
            run_id,
            status: SyncRunStatus::Cancelled,
            draft_id: None,
            cancel_reason: Some(SyncRunCancelReason::SupersededBy { run_id: new_run_id }),
        };
    }

    // Persist the draft. If the row write fails, fall through to
    // `Failed` rather than `Completed` — a draft that isn't on disk
    // is not a draft.
    let draft_repo = dayseam_db::DraftRepo::new(orch.pool.clone());
    if let Err(e) = draft_repo.insert(&draft).await {
        tracing::error!(
            run_id = %run_id,
            error = %e,
            "draft insert failed — run terminating as Failed"
        );
        return terminate_failed(
            &orch,
            run_id,
            &key,
            &progress,
            per_source_syncrun,
            &format!("draft insert failed: {e}"),
        )
        .await;
    }

    // Durable terminal transition. The repo rejects any move out
    // of a non-`Running` state, so this is the last write we make
    // for this run's row.
    let syncrun_repo = dayseam_db::SyncRunRepo::new(orch.pool.clone());
    let finished_at = Utc::now();
    if let Err(e) = syncrun_repo
        .mark_finished(&run_id, finished_at, &per_source_syncrun)
        .await
    {
        tracing::error!(
            run_id = %run_id,
            error = %e,
            "sync_runs mark_finished failed after draft persisted; \
             surfacing as Failed to the caller"
        );
        // NB: the draft is persisted; the run-state row is not.
        // This is the one window where a retry by the caller would
        // produce a duplicate draft, but leaving `Running` stuck
        // would be worse. The crash-recovery sweep in PR-B picks
        // up any Running rows the next time the app starts.
    }

    // Remove our in-flight entry (if it still matches us). This is
    // idempotent w.r.t. supersede: if a newer run has already
    // replaced our entry, we leave the newer entry alone.
    drop_in_flight_if_ours(&orch, &key, run_id).await;

    // Run-wide terminal progress event. The UI drains the stream
    // and renders "Generate report — done" from this.
    progress.send(
        None,
        ProgressPhase::Completed {
            message: format!(
                "Report for {} ready ({draft_id} generated_at {generated_at})",
                request.date
            ),
        },
    );

    // Dropping `progress` / `logs` last closes the per-run streams
    // so the forwarder on the other end observes `None`. We are
    // explicit because silent leaks of per-run streams are exactly
    // the defect the RunRegistry reaper would have to work around.
    drop(progress);
    drop(logs);

    let _started_at_unused = started_at; // captured above; kept for future diagnostics

    GenerateOutcome {
        run_id,
        status: SyncRunStatus::Completed,
        draft_id: Some(draft_id),
        cancel_reason: None,
    }
}

/// Run every source's `sync` concurrently with a parallelism cap of
/// `min(n, 4)`. Returns one [`PerSourceResult`] per source in the
/// order they completed (not the order they were requested); the
/// caller re-indexes by `source_id` before building the report
/// input.
#[allow(clippy::too_many_arguments)]
async fn fan_out(
    orch: Orchestrator,
    person: Person,
    run_id: RunId,
    date: chrono::NaiveDate,
    sources: Vec<SourceHandle>,
    progress: ProgressSender,
    logs: dayseam_events::LogSender,
    cancel: CancellationToken,
) -> Vec<PerSourceResult> {
    let cap = sources.len().clamp(1, 4);
    let semaphore = Arc::new(Semaphore::new(cap));
    let results: Arc<Mutex<Vec<PerSourceResult>>> = Arc::new(Mutex::new(Vec::new()));

    let mut set = tokio::task::JoinSet::new();

    for source in sources {
        let sem = semaphore.clone();
        let results = results.clone();
        let connector = orch.connectors.get(source.kind);
        let http = orch.http.clone();
        let clock = orch.clock.clone();
        let raw_store = orch.raw_store.clone();
        let progress_tx = progress.clone();
        let log_tx = logs.clone();
        let cancel_clone = cancel.clone();
        let person_clone = person.clone();

        set.spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                // Semaphore is closed only if it was dropped, which
                // happens when the orchestrator itself is dropped.
                // Treat as cancellation.
                Err(_) => return,
            };

            let started_at = Utc::now();
            let Some(connector) = connector else {
                let err = DayseamError::InvalidConfig {
                    code: error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST.to_string(),
                    message: format!("no connector registered for kind {:?}", source.kind),
                };
                let mut guard = results.lock().await;
                guard.push(PerSourceResult {
                    source_id: source.source_id,
                    started_at,
                    finished_at: Utc::now(),
                    outcome: Err(err),
                });
                return;
            };

            let ctx = ConnCtx {
                run_id,
                source_id: source.source_id,
                person: person_clone,
                source_identities: source.source_identities.clone(),
                auth: source.auth.clone(),
                progress: progress_tx,
                logs: log_tx,
                raw_store,
                clock,
                http,
                cancel: cancel_clone,
            };

            let outcome = connector.sync(&ctx, SyncRequest::Day(date)).await;
            let finished_at = Utc::now();

            let mut guard = results.lock().await;
            guard.push(PerSourceResult {
                source_id: source.source_id,
                started_at,
                finished_at,
                outcome,
            });
        });
    }

    while set.join_next().await.is_some() {
        // Drain every spawned task. Panics inside a task are
        // already surfaced through `DayseamError::Internal` at the
        // connector layer, so a clean join here means all tasks
        // finished one way or the other.
    }

    Arc::try_unwrap(results)
        .expect("JoinSet drained so the Arc is the last holder")
        .into_inner()
}

/// Split the fan-out vector into the four shapes the persistence
/// path wants: events, artifacts, the report-engine
/// `HashMap<SourceId, SourceRunState>`, and the SyncRun
/// `Vec<PerSourceState>`. The split is done once, eagerly, so the
/// `render` / `insert` / `mark_finished` chain operates on matching
/// slices without cloning the event vectors.
fn split_fan_out(
    per_source: Vec<PerSourceResult>,
) -> (
    Vec<ActivityEvent>,
    Vec<Artifact>,
    HashMap<SourceId, SourceRunState>,
    Vec<PerSourceState>,
) {
    let mut events = Vec::new();
    let mut artifacts = Vec::new();
    let mut per_source_state: HashMap<SourceId, SourceRunState> = HashMap::new();
    let mut per_source_syncrun: Vec<PerSourceState> = Vec::new();

    for res in per_source {
        let PerSourceResult {
            source_id,
            started_at,
            finished_at,
            outcome,
        } = res;
        match outcome {
            Ok(sync_result) => {
                let fetched_count = sync_result.events.len();
                events.extend(sync_result.events);
                artifacts.extend(sync_result.artifacts);
                per_source_state.insert(
                    source_id,
                    SourceRunState {
                        status: RunStatus::Succeeded,
                        started_at,
                        finished_at: Some(finished_at),
                        fetched_count,
                        error: None,
                    },
                );
                per_source_syncrun.push(PerSourceState {
                    source_id,
                    status: RunStatus::Succeeded,
                    started_at,
                    finished_at: Some(finished_at),
                    fetched_count: fetched_count as u32,
                    error: None,
                });
            }
            Err(err) => {
                per_source_state.insert(
                    source_id,
                    SourceRunState {
                        status: RunStatus::Failed,
                        started_at,
                        finished_at: Some(finished_at),
                        fetched_count: 0,
                        error: Some(err.clone()),
                    },
                );
                per_source_syncrun.push(PerSourceState {
                    source_id,
                    status: RunStatus::Failed,
                    started_at,
                    finished_at: Some(finished_at),
                    fetched_count: 0,
                    error: Some(err),
                });
            }
        }
    }

    (events, artifacts, per_source_state, per_source_syncrun)
}

/// Extract a [`MergeRequestArtifact`] per MR event that carries a
/// `commit_shas` list in its `metadata`.
///
/// The GitLab connector's per-MR enrichment (Phase 3 follow-up)
/// stashes the branch's commit SHAs under `metadata.commit_shas` on
/// each `MrOpened` / `MrMerged` / `MrClosed` / `MrApproved` event so
/// the report engine can thread them through
/// [`annotate_rolled_into_mr`] without minting a new artifact kind.
/// Events without the field contribute nothing — a
/// local-git-only run returns an empty vec and the MR-rollup pass
/// is a no-op.
fn collect_mr_artifacts(events: &[ActivityEvent]) -> Vec<MergeRequestArtifact> {
    use dayseam_core::ActivityKind::{MrApproved, MrClosed, MrMerged, MrOpened};

    let mut by_id: std::collections::BTreeMap<String, MergeRequestArtifact> =
        std::collections::BTreeMap::new();
    for event in events {
        if !matches!(event.kind, MrOpened | MrMerged | MrClosed | MrApproved) {
            continue;
        }
        let Some(array) = event.metadata.get("commit_shas").and_then(|v| v.as_array()) else {
            continue;
        };
        let shas: Vec<String> = array
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if shas.is_empty() {
            continue;
        }
        // Latest event for a given MR wins — an `MrMerged` with a
        // fuller commit list supersedes an earlier `MrOpened`.
        by_id.insert(
            event.external_id.clone(),
            MergeRequestArtifact {
                external_id: event.external_id.clone(),
                commit_shas: shas,
            },
        );
    }
    by_id.into_values().collect()
}

/// `run_id` is no longer the in-flight entry for `key`. Returns the
/// [`RunId`] that replaced us if so — the caller emits
/// `Cancelled { SupersededBy(_) }`.
async fn superseded_by_now(
    orch: &Orchestrator,
    key: &InFlightKey,
    my_run_id: RunId,
) -> Option<RunId> {
    let guard = orch.in_flight.lock().await;
    match guard.get(key) {
        Some(entry) if entry.run_id != my_run_id => Some(entry.run_id),
        _ => None,
    }
}

async fn drop_in_flight_if_ours(orch: &Orchestrator, key: &InFlightKey, run_id: RunId) {
    let mut guard = orch.in_flight.lock().await;
    if let Some(entry) = guard.get(key) {
        if entry.run_id == run_id {
            guard.remove(key);
        }
    }
}

async fn terminate_cancelled(
    orch: &Orchestrator,
    run_id: RunId,
    key: &InFlightKey,
    progress: &ProgressSender,
    per_source: Vec<PerSourceResult>,
    superseded_by: Option<RunId>,
) -> GenerateOutcome {
    let (_events, _artifacts, _report_state, per_source_syncrun) = split_fan_out(per_source);
    let reason = match superseded_by {
        Some(new_run_id) => SyncRunCancelReason::SupersededBy { run_id: new_run_id },
        None => SyncRunCancelReason::User,
    };
    let message = match superseded_by {
        Some(new_run_id) => {
            format!("Run {run_id} superseded by {new_run_id}")
        }
        None => format!("Run {run_id} cancelled"),
    };
    // Only write the terminal row if we own it. In the supersede
    // case, `supersede_prior` in the newer task already wrote
    // `Cancelled { SupersededBy(_) }` for us; attempting to rewrite
    // it would be rejected by the repo and produce noise in the
    // logs for a benign ordering.
    if superseded_by.is_none() {
        let repo = dayseam_db::SyncRunRepo::new(orch.pool.clone());
        if let Err(e) = repo
            .mark_cancelled(&run_id, Utc::now(), reason, &per_source_syncrun)
            .await
        {
            tracing::debug!(
                run_id = %run_id,
                error = %e,
                "mark_cancelled rejected (likely already terminal); continuing"
            );
        }
    }
    progress.send(None, ProgressPhase::Cancelled { message });
    drop_in_flight_if_ours(orch, key, run_id).await;

    GenerateOutcome {
        run_id,
        status: SyncRunStatus::Cancelled,
        draft_id: None,
        cancel_reason: Some(reason),
    }
}

async fn terminate_failed(
    orch: &Orchestrator,
    run_id: RunId,
    key: &InFlightKey,
    progress: &ProgressSender,
    per_source_syncrun: Vec<PerSourceState>,
    message: &str,
) -> GenerateOutcome {
    let repo = dayseam_db::SyncRunRepo::new(orch.pool.clone());
    if let Err(e) = repo
        .mark_failed(&run_id, Utc::now(), &per_source_syncrun)
        .await
    {
        tracing::debug!(
            run_id = %run_id,
            error = %e,
            "mark_failed rejected (likely already terminal); continuing"
        );
    }
    progress.send(
        None,
        ProgressPhase::Failed {
            code: error_codes::ORCHESTRATOR_RUN_FAILED.to_string(),
            message: message.to_string(),
        },
    );
    drop_in_flight_if_ours(orch, key, run_id).await;
    GenerateOutcome {
        run_id,
        status: SyncRunStatus::Failed,
        draft_id: None,
        cancel_reason: None,
    }
}

/// Exposed for docs: the Dev EOD template id that PR-A tests use.
/// Re-exported here so callers don't need to depend on
/// `dayseam-report` directly just to pass the id into
/// [`GenerateRequest`].
pub const DEV_EOD_TEMPLATE_ID_REEXPORT: &str = DEV_EOD_TEMPLATE_ID;
