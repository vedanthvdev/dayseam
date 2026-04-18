//! Phase-1 Tauri command surface.
//!
//! Five commands ship from Phase 1 — three real (`settings_get`,
//! `settings_update`, `logs_tail`) and two dev-only helpers gated
//! behind the `dev-commands` Cargo feature (`dev_emit_toast`,
//! `dev_start_demo_run`). The dev commands exist so the frontend
//! event-stream UI can be exercised end-to-end before any real
//! connector or orchestrator lands.
//!
//! Every command here is also named in
//! `apps/desktop/src-tauri/capabilities/default.json`; Tauri 2 denies
//! any command whose identifier is not listed in the active
//! capability. Keeping this file, the capability file, and
//! `packages/ipc-types/src/index.ts::Commands` in sync on every
//! change is an invariant of the IPC review checklist.

use chrono::{DateTime, Utc};
use dayseam_core::{DayseamError, LogEntry, Settings, SettingsPatch};
use dayseam_db::{LogRepo, LogRow, SettingsRepo};
use tauri::State;

use crate::state::AppState;

/// Settings key used by [`settings_get`] / [`settings_update`]. One
/// row for the whole app is enough for Phase 1; per-scope settings
/// (per source, per project) can land alongside them in a later phase
/// without changing this key.
const APP_SETTINGS_KEY: &str = "app";

/// Default `limit` applied to [`logs_tail`] when the frontend does not
/// supply one. Matches the design doc's "last 100 persisted entries".
const DEFAULT_LOGS_LIMIT: u32 = 100;
/// Upper bound so a badly-behaved caller cannot drain the entire log
/// table on a single IPC round-trip.
const MAX_LOGS_LIMIT: u32 = 1_000;

fn internal(code: &str, err: impl std::fmt::Display) -> DayseamError {
    DayseamError::Internal {
        code: code.to_string(),
        message: err.to_string(),
    }
}

/// Read the currently-stored [`Settings`]. Falls back to
/// [`Settings::default`] when nothing has been persisted yet, so the
/// frontend never has to deal with a "settings missing" empty state.
#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> Result<Settings, DayseamError> {
    let repo = SettingsRepo::new(state.pool.clone());
    let stored = repo
        .get::<Settings>(APP_SETTINGS_KEY)
        .await
        .map_err(|e| internal("settings.read", e))?;
    Ok(stored.unwrap_or_default())
}

/// Apply a partial update and return the resulting full [`Settings`].
/// The Rust side is the source of truth for the merge semantics; see
/// [`Settings::with_patch`].
#[tauri::command]
pub async fn settings_update(
    patch: SettingsPatch,
    state: State<'_, AppState>,
) -> Result<Settings, DayseamError> {
    let repo = SettingsRepo::new(state.pool.clone());
    let current = repo
        .get::<Settings>(APP_SETTINGS_KEY)
        .await
        .map_err(|e| internal("settings.read", e))?
        .unwrap_or_default();
    let next = current.with_patch(patch);
    repo.set(APP_SETTINGS_KEY, &next)
        .await
        .map_err(|e| internal("settings.write", e))?;
    Ok(next)
}

/// Read the persisted log drawer tail, newest first.
///
/// * `since` — only return rows with `ts >= since`; `None` means
///   "return the whole retained window".
/// * `limit` — clamp to at most `MAX_LOGS_LIMIT`; `None` uses
///   `DEFAULT_LOGS_LIMIT`.
///
/// `LogRepo::tail` already orders newest-first, which is what the log
/// drawer renders, so we pass the rows through unchanged.
#[tauri::command]
pub async fn logs_tail(
    since: Option<DateTime<Utc>>,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<Vec<LogEntry>, DayseamError> {
    let effective_since = since.unwrap_or(DateTime::<Utc>::MIN_UTC);
    let effective_limit = limit.unwrap_or(DEFAULT_LOGS_LIMIT).min(MAX_LOGS_LIMIT);
    let repo = LogRepo::new(state.pool.clone());
    let rows: Vec<LogRow> = repo
        .tail(effective_since, effective_limit)
        .await
        .map_err(|e| internal("logs.tail", e))?;
    Ok(rows.into_iter().map(log_row_to_entry).collect())
}

fn log_row_to_entry(row: LogRow) -> LogEntry {
    LogEntry {
        timestamp: row.ts,
        level: row.level,
        source_id: row.source_id,
        message: row.message,
    }
}

// ---------------------------------------------------------------------------
// Dev-only commands
// ---------------------------------------------------------------------------

#[cfg(feature = "dev-commands")]
pub use dev::*;

#[cfg(feature = "dev-commands")]
mod dev {
    use super::*;

    use dayseam_core::{LogEvent, LogLevel, ProgressEvent, ProgressPhase, RunId, ToastEvent};
    use dayseam_events::RunStreams;
    use tauri::ipc::Channel;
    use tokio_util::sync::CancellationToken;

    use crate::ipc::run_forwarder;
    use crate::state::{spawn_run_reaper, RunHandle};

    /// Fire a [`ToastEvent`] onto the app bus. The broadcast
    /// forwarder picks it up and emits it to every window — exactly
    /// the same code path a real error or success will take in Phase
    /// 2, which is what makes this useful in tests.
    #[tauri::command]
    pub async fn dev_emit_toast(
        event: ToastEvent,
        state: State<'_, AppState>,
    ) -> Result<(), DayseamError> {
        state.app_bus.publish_toast(event);
        Ok(())
    }

    /// Start a synthetic run that emits three progress events and a
    /// handful of log lines on the provided channels. Returns the
    /// `RunId` so the frontend can correlate events it receives.
    ///
    /// Exists so Task 9 can validate the per-run streaming model
    /// end-to-end before Phase 2 lands the first real connector. The
    /// event shapes it produces match what a real `SyncRun` will
    /// emit.
    #[tauri::command]
    pub async fn dev_start_demo_run(
        progress: Channel<ProgressEvent>,
        logs: Channel<LogEvent>,
        state: State<'_, AppState>,
    ) -> Result<RunId, DayseamError> {
        let streams = RunStreams::new(RunId::new());
        let run_id = streams.run_id;
        let ((progress_tx, log_tx), (progress_rx, log_rx)) = streams.split();

        let progress_task = run_forwarder::spawn_progress_forwarder(progress_rx, progress);
        let log_task = run_forwarder::spawn_log_forwarder(log_rx, logs);

        let cancel = CancellationToken::new();
        let producer_cancel = cancel.clone();

        let producer = tokio::spawn(async move {
            progress_tx.send(
                None,
                ProgressPhase::Starting {
                    message: "demo run starting".into(),
                },
            );
            log_tx.send(
                LogLevel::Info,
                None,
                "demo run starting",
                serde_json::json!({ "demo": true }),
            );

            for i in 1..=2 {
                if producer_cancel.is_cancelled() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                progress_tx.send(
                    None,
                    ProgressPhase::InProgress {
                        completed: i,
                        total: Some(2),
                        message: format!("step {i}/2"),
                    },
                );
                log_tx.send(
                    LogLevel::Debug,
                    None,
                    format!("demo step {i}/2"),
                    serde_json::json!({ "step": i }),
                );
            }

            if !producer_cancel.is_cancelled() {
                progress_tx.send(
                    None,
                    ProgressPhase::Completed {
                        message: "demo run complete".into(),
                    },
                );
                log_tx.send(
                    LogLevel::Info,
                    None,
                    "demo run complete",
                    serde_json::json!({ "demo": true }),
                );
            }
            // Senders drop here, which closes the forwarders cleanly.
        });

        // Register the run, then spawn a reaper that waits for the
        // producer and both forwarders to finish and then removes the
        // run from the registry. Without the reaper every completed
        // run would pile up — holding its `CancellationToken` and
        // three `JoinHandle`s forever (see COR-02 / PERF-03).
        let reaper = spawn_run_reaper(
            state.runs.clone(),
            run_id,
            vec![progress_task, log_task, producer],
        );
        let mut registry = state.runs.write().await;
        registry.insert(RunHandle {
            run_id,
            cancel,
            reaper: Some(reaper),
        });
        Ok(run_id)
    }
}

#[cfg(all(test, feature = "dev-commands"))]
mod tests {
    use super::*;
    use dayseam_core::LogLevel;
    use dayseam_db::open;
    use dayseam_events::AppBus;
    use dayseam_secrets::InMemoryStore;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn make_state() -> (AppState, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open db");
        let state = AppState::new(pool, AppBus::new(), Arc::new(InMemoryStore::new()));
        (state, dir)
    }

    #[tokio::test]
    async fn settings_update_then_get_round_trips() {
        let (state, _dir) = make_state().await;
        let repo = SettingsRepo::new(state.pool.clone());
        let initial = repo.get::<Settings>(APP_SETTINGS_KEY).await.expect("read");
        assert!(initial.is_none(), "settings start empty");

        // Simulate settings_update by calling with_patch directly —
        // the command wraps exactly this logic.
        let current = initial.unwrap_or_default();
        let next = current.with_patch(SettingsPatch {
            theme: Some(dayseam_core::ThemePreference::Dark),
            verbose_logs: Some(true),
        });
        repo.set(APP_SETTINGS_KEY, &next).await.expect("write");

        let stored = repo
            .get::<Settings>(APP_SETTINGS_KEY)
            .await
            .expect("read")
            .expect("row exists");
        assert_eq!(stored.theme, dayseam_core::ThemePreference::Dark);
        assert!(stored.verbose_logs);
    }

    #[tokio::test]
    async fn logs_tail_returns_newest_first() {
        let (state, _dir) = make_state().await;
        let repo = LogRepo::new(state.pool.clone());
        for i in 0..3 {
            repo.append(&LogRow {
                ts: Utc::now() + chrono::Duration::milliseconds(i as i64),
                level: LogLevel::Info,
                source_id: None,
                message: format!("entry {i}"),
                context: None,
            })
            .await
            .expect("append");
        }

        let rows = repo.tail(DateTime::<Utc>::MIN_UTC, 10).await.expect("tail");
        let messages: Vec<_> = rows.into_iter().map(|r| r.message).collect();
        assert_eq!(messages, vec!["entry 2", "entry 1", "entry 0"]);
    }
}
