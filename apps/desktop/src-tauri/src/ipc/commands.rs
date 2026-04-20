//! Tauri command surface.
//!
//! Phase-1 shipped three production commands (`settings_get`,
//! `settings_update`, `logs_tail`) plus two dev-only helpers gated
//! behind the `dev-commands` Cargo feature (`dev_emit_toast`,
//! `dev_start_demo_run`). Task 6 PR-A expands the production surface
//! with the source / identity / local-repo / report / sink / retention
//! commands the frontend needs to drive a real run.
//!
//! Every command here is also named in
//! `apps/desktop/src-tauri/capabilities/default.json`, in the
//! `COMMANDS` slice in `build.rs`, and in
//! `packages/ipc-types/src/index.ts::Commands`. Tauri 2 denies any
//! command whose identifier is not listed in the active capability;
//! keeping all four surfaces in sync on every change is an invariant
//! enforced by the `ipc_capabilities_cover_every_registered_command`
//! integration test plus the matching Vitest parity test on the TS
//! side.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use connector_local_git::{discover_repos, DiscoveryConfig};
use connectors_sdk::{ConnCtx, NoneAuth, NoopRawStore, SystemClock};
use dayseam_core::{
    error_codes, ActivityEvent, DayseamError, GitlabValidationResult, LocalRepo, LogEntry,
    LogLevel, Person, ProgressEvent, ReportCompletedEvent, ReportDraft, RunId, Settings,
    SettingsPatch, Sink, SinkConfig, SinkKind, Source, SourceConfig, SourceHealth, SourceId,
    SourceIdentity, SourceKind, SourcePatch, ToastEvent, ToastSeverity, WriteReceipt,
};
use dayseam_db::{
    ActivityRepo, DraftRepo, LocalRepoRepo, LogRepo, LogRow, PersonRepo, SettingsRepo, SinkRepo,
    SourceIdentityRepo, SourceRepo,
};
use dayseam_events::RunStreams;
use dayseam_orchestrator::{resolve_cutoff, retention_sweep, GenerateRequest, SourceHandle};
use dayseam_report::{DEV_EOD_TEMPLATE_ID, DEV_EOD_TEMPLATE_VERSION};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ipc::run_forwarder;
use crate::ipc::secret::IpcSecretString;
use crate::state::{spawn_run_reaper, AppState, RunHandle};

/// Settings key used by [`settings_get`] / [`settings_update`]. One
/// row for the whole app is enough for Phase 1; per-scope settings
/// (per source, per project) can land alongside them in a later phase
/// without changing this key.
const APP_SETTINGS_KEY: &str = "app";

/// Production Tauri command identifiers. The canonical source of
/// truth for the `invoke_handler!` list in `main.rs`, the `COMMANDS`
/// slice in `build.rs`, and the `allow-*` permissions in
/// `capabilities/default.json`. Exposed (rather than hidden in
/// `build.rs`) so the capability-parity integration test can diff
/// the JSON against this array without re-parsing the source.
///
/// Keep in sync with `DEV_COMMANDS` for the dev-only surface.
pub const PROD_COMMANDS: &[&str] = &[
    "settings_get",
    "settings_update",
    "logs_tail",
    "persons_get_self",
    "persons_update_self",
    "sources_list",
    "sources_add",
    "sources_update",
    "sources_delete",
    "sources_healthcheck",
    "identities_list_for",
    "identities_upsert",
    "identities_delete",
    "local_repos_list",
    "local_repos_set_private",
    "sinks_list",
    "sinks_add",
    "report_generate",
    "report_cancel",
    "report_get",
    "report_save",
    "retention_sweep_now",
    "activity_events_get",
    "shell_open",
    "gitlab_validate_pat",
];

/// Dev-only Tauri command identifiers. Compiled in only when the
/// `dev-commands` Cargo feature is enabled; a matching `dev.json`
/// capability file is written by `build.rs` in the same
/// configuration.
pub const DEV_COMMANDS: &[&str] = &["dev_emit_toast", "dev_start_demo_run"];

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
// Phase-2 (Task 6 PR-A) commands — sources, identities, local repos,
// reports, sinks, retention.
//
// Every command in this section is a thin pass-through over a
// `dayseam-orchestrator` or `dayseam-db` API; the orchestration crate
// owns the per-command business logic and the database crate owns the
// schema. The commands themselves only translate IPC arguments into
// repo / orchestrator calls and translate `Option<T>` / DB errors into
// the typed `DayseamError` shapes the frontend expects.
//
// Patches and "restart required" toasts deliberately live here — see
// the `boot-only contract` documented on
// [`crate::startup::build_orchestrator`]. The `ConnectorRegistry` /
// `SinkRegistry` snapshot the database at startup, so any
// `sources_*` mutation that changes the registry-relevant fields
// (scan roots, private flags) needs the user to restart for the new
// configuration to take effect. Toasts go out via the
// [`dayseam_events::AppBus`] so the broadcast forwarder picks them
// up like every other app-wide event.
// ---------------------------------------------------------------------------

const RESTART_TOAST_TITLE: &str = "Restart required";
const RESTART_TOAST_BODY: &str =
    "Source changes take effect after restarting Dayseam. Quit and reopen the app to use the new configuration.";

fn invalid_config(code: &str, message: impl Into<String>) -> DayseamError {
    DayseamError::InvalidConfig {
        code: code.to_string(),
        message: message.into(),
    }
}

fn publish_restart_required_toast(state: &AppState) {
    let event = ToastEvent {
        id: Uuid::new_v4(),
        severity: ToastSeverity::Warning,
        title: RESTART_TOAST_TITLE.into(),
        body: Some(RESTART_TOAST_BODY.into()),
        emitted_at: Utc::now(),
    };
    state.app_bus.publish_toast(event);
}

// ---- Persons --------------------------------------------------------------

/// Resolve the canonical "self" [`Person`] row, creating it on first
/// call. Phase 2 uses a single self-row everywhere; the multi-person
/// machinery lands in a later phase.
#[tauri::command]
pub async fn persons_get_self(state: State<'_, AppState>) -> Result<Person, DayseamError> {
    PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| internal("persons.get_self", e))
}

/// Default `display_name` stamped onto the self-`Person` row when
/// `persons_get_self` bootstraps it. The onboarding checklist treats a
/// display name that still equals this sentinel as "the user hasn't
/// picked one yet" so the first-run empty state has something concrete
/// to ask for.
///
/// Exposed as a `pub const` (and re-exported from `ipc`) so the
/// frontend test harness — which spoofs `persons_get_self` — can assert
/// the same sentinel instead of hard-coding the literal.
pub const SELF_DEFAULT_DISPLAY_NAME: &str = "Me";

/// Rename the canonical self-`Person` row. Phase 2 Task 7 (first-run
/// empty state) uses this to flip the "pick a name" checklist item
/// from the default `"Me"` to the user's chosen display name.
///
/// Whitespace is trimmed. An all-whitespace or empty input is rejected
/// with `IPC_INVALID_DISPLAY_NAME` so the frontend can surface the
/// error on the same dialog that triggered the call. The row to update
/// is resolved by calling [`PersonRepo::bootstrap_self`] first — that's
/// idempotent, and it means a caller that hit `persons_update_self`
/// before ever hitting `persons_get_self` still works.
#[tauri::command]
pub async fn persons_update_self(
    display_name: String,
    state: State<'_, AppState>,
) -> Result<Person, DayseamError> {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return Err(invalid_config(
            error_codes::IPC_INVALID_DISPLAY_NAME,
            "display_name must not be empty or whitespace-only",
        ));
    }
    let repo = PersonRepo::new(state.pool.clone());
    let current = repo
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| internal("persons.bootstrap_self", e))?;
    repo.update_display_name(current.id, trimmed)
        .await
        .map_err(|e| internal("persons.update_display_name", e))
}

// ---- Sources --------------------------------------------------------------

#[tauri::command]
pub async fn sources_list(state: State<'_, AppState>) -> Result<Vec<Source>, DayseamError> {
    SourceRepo::new(state.pool.clone())
        .list()
        .await
        .map_err(|e| internal("sources.list", e))
}

/// Persist a new [`Source`]. For `LocalGit` sources, every git repo
/// found beneath the supplied scan roots is upserted into
/// `local_repos` with `is_private = false` so the user can flip
/// individual rows via [`local_repos_set_private`] without a separate
/// "discover now" call.
///
/// Emits a "restart required" toast on success — the in-memory
/// connector registry is built once at startup and does not pick up
/// the new source's scan roots until the next boot. See the
/// `boot-only contract` on [`crate::startup::build_orchestrator`].
#[tauri::command]
pub async fn sources_add(
    kind: SourceKind,
    label: String,
    config: SourceConfig,
    state: State<'_, AppState>,
) -> Result<Source, DayseamError> {
    if config.kind() != kind {
        return Err(invalid_config(
            error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH,
            format!(
                "kind {kind:?} does not match config kind {:?}",
                config.kind()
            ),
        ));
    }

    let source = Source {
        id: Uuid::new_v4(),
        kind,
        label,
        config: config.clone(),
        secret_ref: None,
        created_at: Utc::now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };

    let source_repo = SourceRepo::new(state.pool.clone());
    source_repo
        .insert(&source)
        .await
        .map_err(|e| internal("sources.insert", e))?;

    if let SourceConfig::LocalGit { scan_roots } = &config {
        upsert_discovered_repos(&state, &source.id, scan_roots).await?;
    }

    publish_restart_required_toast(&state);
    Ok(source)
}

#[tauri::command]
pub async fn sources_update(
    id: SourceId,
    patch: SourcePatch,
    state: State<'_, AppState>,
) -> Result<Source, DayseamError> {
    let repo = SourceRepo::new(state.pool.clone());
    let existing = repo
        .get(&id)
        .await
        .map_err(|e| internal("sources.get", e))?
        .ok_or_else(|| {
            invalid_config(
                error_codes::IPC_SOURCE_NOT_FOUND,
                format!("no source with id {id}"),
            )
        })?;

    if let Some(label) = patch.label.as_ref() {
        repo.update_label(&id, label)
            .await
            .map_err(|e| internal("sources.update_label", e))?;
    }
    let mut new_scan_roots: Option<Vec<std::path::PathBuf>> = None;
    if let Some(config) = patch.config.as_ref() {
        if config.kind() != existing.kind {
            return Err(invalid_config(
                error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH,
                format!(
                    "patch kind {:?} does not match persisted kind {:?}",
                    config.kind(),
                    existing.kind
                ),
            ));
        }
        repo.update_config(&id, config)
            .await
            .map_err(|e| internal("sources.update_config", e))?;
        if let SourceConfig::LocalGit { scan_roots } = config {
            new_scan_roots = Some(scan_roots.clone());
        }
    }

    if let Some(roots) = new_scan_roots {
        upsert_discovered_repos(&state, &id, &roots).await?;
    }

    let updated = repo
        .get(&id)
        .await
        .map_err(|e| internal("sources.get", e))?
        .ok_or_else(|| {
            invalid_config(
                error_codes::IPC_SOURCE_NOT_FOUND,
                format!("source {id} disappeared mid-update"),
            )
        })?;

    if patch.config.is_some() {
        publish_restart_required_toast(&state);
    }
    Ok(updated)
}

#[tauri::command]
pub async fn sources_delete(id: SourceId, state: State<'_, AppState>) -> Result<(), DayseamError> {
    SourceRepo::new(state.pool.clone())
        .delete(&id)
        .await
        .map_err(|e| internal("sources.delete", e))?;
    publish_restart_required_toast(&state);
    Ok(())
}

#[tauri::command]
pub async fn sources_healthcheck(
    id: SourceId,
    state: State<'_, AppState>,
) -> Result<SourceHealth, DayseamError> {
    let source_repo = SourceRepo::new(state.pool.clone());
    let source = source_repo
        .get(&id)
        .await
        .map_err(|e| internal("sources.get", e))?
        .ok_or_else(|| {
            invalid_config(
                error_codes::IPC_SOURCE_NOT_FOUND,
                format!("no source with id {id}"),
            )
        })?;

    let connector = state
        .orchestrator
        .connectors()
        .get(source.kind)
        .ok_or_else(|| DayseamError::Internal {
            code: error_codes::ORCHESTRATOR_SINK_NOT_REGISTERED.into(),
            message: format!("no connector registered for {:?}", source.kind),
        })?;

    let person = PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| internal("persons.bootstrap_self", e))?;
    let identities = SourceIdentityRepo::new(state.pool.clone())
        .list_for_source(person.id, &id)
        .await
        .map_err(|e| internal("source_identities.list_for_source", e))?;

    // The connector's healthcheck only reads `auth` / `cancel` /
    // (sometimes) `clock`; everything else is plumbed through for
    // parity with `sync` so connectors don't have to special-case
    // probes. The throwaway `RunStreams` is dropped on return.
    let streams = RunStreams::new(RunId::new());
    let ctx = ConnCtx {
        run_id: streams.run_id,
        source_id: id,
        person,
        source_identities: identities,
        auth: Arc::new(NoneAuth),
        progress: streams.progress_tx.clone(),
        logs: streams.log_tx.clone(),
        raw_store: Arc::new(NoopRawStore),
        clock: Arc::new(SystemClock),
        http: state.orchestrator.http_client().clone(),
        cancel: CancellationToken::new(),
    };

    let health = match connector.healthcheck(&ctx).await {
        Ok(h) => h,
        Err(e) => SourceHealth {
            ok: false,
            checked_at: Some(Utc::now()),
            last_error: Some(e),
        },
    };
    source_repo
        .update_health(&id, &health)
        .await
        .map_err(|e| internal("sources.update_health", e))?;
    Ok(health)
}

async fn upsert_discovered_repos(
    state: &AppState,
    source_id: &SourceId,
    scan_roots: &[std::path::PathBuf],
) -> Result<(), DayseamError> {
    if scan_roots.is_empty() {
        return Ok(());
    }
    let outcome = match discover_repos(scan_roots, DiscoveryConfig::default()) {
        Ok(o) => o,
        Err(e) => {
            // A missing scan root surfaces as `Io`; rather than fail
            // the whole `sources_add` we log it and let the user fix
            // the configuration via `sources_update`.
            tracing::warn!(error = %e, "discover_repos failed during sources_add");
            return Ok(());
        }
    };
    let repo = LocalRepoRepo::new(state.pool.clone());
    let now = Utc::now();
    for discovered in outcome.repos {
        let row = LocalRepo {
            path: discovered.path,
            label: discovered.label,
            is_private: false,
            discovered_at: now,
        };
        repo.upsert(source_id, &row)
            .await
            .map_err(|e| internal("local_repos.upsert", e))?;
    }
    if outcome.truncated {
        tracing::warn!(
            source_id = %source_id,
            "discovery truncated at max_roots — some repos may be missing"
        );
    }
    Ok(())
}

// ---- Identities -----------------------------------------------------------

#[tauri::command]
pub async fn identities_list_for(
    person_id: Uuid,
    state: State<'_, AppState>,
) -> Result<Vec<SourceIdentity>, DayseamError> {
    SourceIdentityRepo::new(state.pool.clone())
        .list_for_person(person_id)
        .await
        .map_err(|e| internal("source_identities.list_for_person", e))
}

/// Insert-or-replace a [`SourceIdentity`] keyed by `id`. The mapping
/// table has no `ON CONFLICT(id)` clause so we delete-then-insert to
/// keep the contract simple for the frontend (one call updates the
/// row whether it existed or not).
#[tauri::command]
pub async fn identities_upsert(
    identity: SourceIdentity,
    state: State<'_, AppState>,
) -> Result<SourceIdentity, DayseamError> {
    let repo = SourceIdentityRepo::new(state.pool.clone());
    repo.delete(identity.id)
        .await
        .map_err(|e| internal("source_identities.delete", e))?;
    repo.insert(&identity)
        .await
        .map_err(|e| internal("source_identities.insert", e))?;
    Ok(identity)
}

#[tauri::command]
pub async fn identities_delete(id: Uuid, state: State<'_, AppState>) -> Result<(), DayseamError> {
    SourceIdentityRepo::new(state.pool.clone())
        .delete(id)
        .await
        .map_err(|e| internal("source_identities.delete", e))
}

// ---- Local repos ----------------------------------------------------------

#[tauri::command]
pub async fn local_repos_list(
    source_id: SourceId,
    state: State<'_, AppState>,
) -> Result<Vec<LocalRepo>, DayseamError> {
    LocalRepoRepo::new(state.pool.clone())
        .list_for_source(&source_id)
        .await
        .map_err(|e| internal("local_repos.list_for_source", e))
}

#[tauri::command]
pub async fn local_repos_set_private(
    path: std::path::PathBuf,
    is_private: bool,
    state: State<'_, AppState>,
) -> Result<LocalRepo, DayseamError> {
    let repo = LocalRepoRepo::new(state.pool.clone());
    repo.set_is_private(&path, is_private)
        .await
        .map_err(|e| internal("local_repos.set_is_private", e))?;
    repo.get(&path)
        .await
        .map_err(|e| internal("local_repos.get", e))?
        .ok_or_else(|| {
            invalid_config(
                error_codes::IPC_LOCAL_REPO_NOT_FOUND,
                format!("no local repo at path {}", path.display()),
            )
        })
}

// ---- Sinks ----------------------------------------------------------------

/// Reject sink configs that the [`SinkAdapter`] would later refuse to
/// write against: empty `dest_dirs`, relative directory paths, and
/// paths with `..` traversal components. The actual sink crate
/// re-checks at write time (defence in depth), but failing fast here
/// keeps the database from accumulating sinks that are guaranteed to
/// 500 every save_report.
fn validate_sink_config(config: &SinkConfig) -> Result<(), DayseamError> {
    use std::path::Component;

    match config {
        SinkConfig::MarkdownFile { dest_dirs, .. } => {
            if dest_dirs.is_empty() {
                return Err(invalid_config(
                    error_codes::IPC_SINK_INVALID_CONFIG,
                    "MarkdownFile sink: dest_dirs must contain at least one path",
                ));
            }
            for dir in dest_dirs {
                if !dir.is_absolute() {
                    return Err(invalid_config(
                        error_codes::IPC_SINK_INVALID_CONFIG,
                        format!(
                            "MarkdownFile sink: dest_dir `{}` must be absolute",
                            dir.display()
                        ),
                    ));
                }
                if dir.components().any(|c| matches!(c, Component::ParentDir)) {
                    return Err(invalid_config(
                        error_codes::IPC_SINK_INVALID_CONFIG,
                        format!(
                            "MarkdownFile sink: dest_dir `{}` must not contain `..` segments",
                            dir.display()
                        ),
                    ));
                }
            }
            Ok(())
        }
    }
}

#[tauri::command]
pub async fn sinks_list(state: State<'_, AppState>) -> Result<Vec<Sink>, DayseamError> {
    SinkRepo::new(state.pool.clone())
        .list()
        .await
        .map_err(|e| internal("sinks.list", e))
}

#[tauri::command]
pub async fn sinks_add(
    kind: SinkKind,
    label: String,
    config: SinkConfig,
    state: State<'_, AppState>,
) -> Result<Sink, DayseamError> {
    if config.kind() != kind {
        return Err(invalid_config(
            error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH,
            format!(
                "sink kind {kind:?} does not match config kind {:?}",
                config.kind()
            ),
        ));
    }
    validate_sink_config(&config)?;
    let sink = Sink {
        id: Uuid::new_v4(),
        kind,
        label,
        config,
        created_at: Utc::now(),
        last_write_at: None,
    };
    SinkRepo::new(state.pool.clone())
        .insert(&sink)
        .await
        .map_err(|e| internal("sinks.insert", e))?;
    Ok(sink)
}

// ---- Reports --------------------------------------------------------------

/// Name of the Tauri window event emitted when a `report_generate`
/// run reaches a terminal [`dayseam_core::SyncRunStatus`]. The
/// payload is a [`ReportCompletedEvent`]; the frontend uses
/// `draft_id` to fetch the persisted draft via [`report_get`].
const REPORT_COMPLETED_EVENT: &str = "report:completed";

#[tauri::command]
pub async fn report_generate(
    date: chrono::NaiveDate,
    source_ids: Vec<SourceId>,
    template_id: Option<String>,
    progress: Channel<ProgressEvent>,
    logs: Channel<dayseam_core::LogEvent>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RunId, DayseamError> {
    let template_id = template_id.unwrap_or_else(|| DEV_EOD_TEMPLATE_ID.to_string());

    let person = PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| internal("persons.bootstrap_self", e))?;

    let source_repo = SourceRepo::new(state.pool.clone());
    let identity_repo = SourceIdentityRepo::new(state.pool.clone());
    let mut sources = Vec::with_capacity(source_ids.len());
    for source_id in source_ids {
        let source = source_repo
            .get(&source_id)
            .await
            .map_err(|e| internal("sources.get", e))?
            .ok_or_else(|| {
                invalid_config(
                    error_codes::IPC_SOURCE_NOT_FOUND,
                    format!("no source with id {source_id}"),
                )
            })?;
        let identities = identity_repo
            .list_for_source(person.id, &source_id)
            .await
            .map_err(|e| internal("source_identities.list_for_source", e))?;
        sources.push(SourceHandle {
            source_id: source.id,
            kind: source.kind,
            auth: Arc::new(NoneAuth),
            source_identities: identities,
        });
    }

    let settings = SettingsRepo::new(state.pool.clone())
        .get::<Settings>(APP_SETTINGS_KEY)
        .await
        .map_err(|e| internal("settings.read", e))?
        .unwrap_or_default();

    let request = GenerateRequest {
        person,
        sources,
        date,
        template_id,
        template_version: DEV_EOD_TEMPLATE_VERSION.to_string(),
        verbose_mode: settings.verbose_logs,
    };

    let handle = state.orchestrator.generate_report(request).await;
    let run_id = handle.run_id;
    let cancel = handle.cancel.clone();

    let progress_task = run_forwarder::spawn_progress_forwarder(handle.progress_rx, progress);
    let log_task = run_forwarder::spawn_log_forwarder(handle.log_rx, logs);

    let app_handle = app.clone();
    let completion_task = tokio::spawn(async move {
        match handle.completion.await {
            Ok(outcome) => {
                let payload = ReportCompletedEvent {
                    run_id: outcome.run_id,
                    status: outcome.status,
                    draft_id: outcome.draft_id,
                    cancel_reason: outcome.cancel_reason,
                };
                if let Err(e) = app_handle.emit(REPORT_COMPLETED_EVENT, &payload) {
                    tracing::warn!(error = %e, "failed to emit report:completed");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, %run_id, "generate completion task panicked");
            }
        }
    });

    let reaper = spawn_run_reaper(
        state.runs.clone(),
        run_id,
        vec![progress_task, log_task, completion_task],
    );
    let mut registry = state.runs.write().await;
    registry.insert(RunHandle {
        run_id,
        cancel,
        reaper: Some(reaper),
    });
    Ok(run_id)
}

#[tauri::command]
pub async fn report_cancel(run_id: RunId, state: State<'_, AppState>) -> Result<(), DayseamError> {
    state.orchestrator.cancel(run_id).await;
    Ok(())
}

#[tauri::command]
pub async fn report_get(
    draft_id: Uuid,
    state: State<'_, AppState>,
) -> Result<ReportDraft, DayseamError> {
    DraftRepo::new(state.pool.clone())
        .get(&draft_id)
        .await
        .map_err(|e| internal("drafts.get", e))?
        .ok_or_else(|| {
            invalid_config(
                error_codes::IPC_REPORT_DRAFT_NOT_FOUND,
                format!("no draft with id {draft_id}"),
            )
        })
}

#[tauri::command]
pub async fn report_save(
    draft_id: Uuid,
    sink_id: Uuid,
    state: State<'_, AppState>,
) -> Result<Vec<WriteReceipt>, DayseamError> {
    let sink = SinkRepo::new(state.pool.clone())
        .get(&sink_id)
        .await
        .map_err(|e| internal("sinks.get", e))?
        .ok_or_else(|| {
            invalid_config(
                error_codes::IPC_SINK_NOT_FOUND,
                format!("no sink with id {sink_id}"),
            )
        })?;
    state.orchestrator.save_report(draft_id, &sink).await
}

// ---- Activity events ------------------------------------------------------

/// Hydrate a batch of [`ActivityEvent`]s for the evidence popover.
/// The popover gets event *ids* from `ReportDraft::evidence` and needs
/// the full rows to render the "what caused this bullet" list; this
/// command is the read-only bridge that turns the first into the
/// second. Ids that no longer exist on disk (retention evicted them)
/// are silently dropped rather than returned as an error — the popover
/// is a best-effort explainer, not an audit log.
#[tauri::command]
pub async fn activity_events_get(
    ids: Vec<Uuid>,
    state: State<'_, AppState>,
) -> Result<Vec<ActivityEvent>, DayseamError> {
    ActivityRepo::new(state.pool.clone())
        .get_many(&ids)
        .await
        .map_err(|e| internal("activity_events.get_many", e))
}

// ---- Shell integration ----------------------------------------------------

/// Schemes [`shell_open`] is willing to hand to the OS.
///
/// * `http` / `https` — activity event links (MRs, issues, commits).
/// * `file` — the "open saved report" action on a `WriteReceipt`.
///   `file://` URLs additionally must be absolute and free of `..`
///   components; see [`validate_file_url_path`].
/// * `vscode` / `vscode-insiders` — "open in editor" affordance.
/// * `obsidian` — "open in Obsidian" for the markdown sink.
///
/// Everything else is refused so a compromised or buggy connector
/// cannot slip a `javascript:`, `data:`, or traversal-laden `file://`
/// past the app. Callers get a typed `DayseamError` instead of a
/// silent handoff.
const SHELL_ALLOWED_SCHEMES: &[&str] = &[
    "http",
    "https",
    "file",
    "vscode",
    "vscode-insiders",
    "obsidian",
];

/// Reject `file://` URLs whose path is not absolute or contains
/// `..` traversal segments. The docstring on [`shell_open`] and
/// [`SHELL_ALLOWED_SCHEMES`] promises this guard; Phase 2 Task 8
/// added it after the correctness review found the guard was
/// missing from the scheme check.
///
/// The raw (pre-parse) URL string is inspected for `..` path
/// segments because [`url::Url`] silently normalises them away
/// during parsing — `file:///Users/alice/../../etc/passwd`
/// becomes `/etc/passwd` on the parsed `Url`, which would otherwise
/// slip past a components-only check. The parsed URL's path is
/// still used to enforce absolute-path form.
///
/// Accepts: `file:///Users/alice/Documents/Dayseam/2026-04-17.md`.
/// Rejects: `file:///Users/alice/../../etc/passwd`,
/// `file:relative/path`, `file://./relative/path`.
fn validate_file_url_path(raw_url: &str, url: &url::Url) -> Result<(), DayseamError> {
    use std::path::{Component, PathBuf};

    // `file:` URLs must be in the full `file:///absolute/path` form.
    // url::Url::parse happily accepts `file:relative/path` and
    // `file://host/path`, and it silently normalises `..` segments
    // out of the parsed path — both would let a crafted sink slip
    // a traversal or relative path past the scheme allow-list. Gate
    // on the raw input so we never have to trust the parser's
    // normalisation for security.
    if !raw_url.starts_with("file:///") {
        return Err(DayseamError::InvalidConfig {
            code: error_codes::IPC_SHELL_URL_DISALLOWED.into(),
            message: format!(
                "file:// url must use the `file:///<absolute-path>` form: `{raw_url}`"
            ),
        });
    }
    // Segment-level check so `..foo` doesn't false-positive. The URL
    // parser would strip `..` out of `url.path()`, so we look at the
    // original string.
    let after_scheme = &raw_url["file:///".len()..];
    let has_parent_segment = after_scheme.split(['/', '\\']).any(|seg| seg == "..");
    if has_parent_segment {
        return Err(DayseamError::InvalidConfig {
            code: error_codes::IPC_SHELL_URL_DISALLOWED.into(),
            message: format!("file:// path contains `..` traversal segment: `{raw_url}`"),
        });
    }

    let raw = url.path();
    if raw.is_empty() {
        return Err(DayseamError::InvalidConfig {
            code: error_codes::IPC_SHELL_URL_DISALLOWED.into(),
            message: "file:// url has empty path".to_string(),
        });
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(DayseamError::InvalidConfig {
            code: error_codes::IPC_SHELL_URL_DISALLOWED.into(),
            message: format!("file:// path must be absolute: `{raw}`"),
        });
    }
    // Belt and suspenders: even though the raw-string check above
    // already rejects `..`, re-verify on the parsed components in
    // case the parser were ever to stop normalising.
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SHELL_URL_DISALLOWED.into(),
                message: format!("file:// path contains `..` traversal component: `{raw}`"),
            });
        }
    }
    Ok(())
}

/// Hand a URL to the host OS so it opens in whatever app is registered
/// for the scheme (browser, editor, Obsidian, Finder/Explorer…). The
/// command is intentionally narrow — scheme is checked against a
/// hard-coded allow-list before the URL leaves the sandbox — because
/// it is the only Phase-2 surface that can launch another process.
///
/// Returning `Ok(())` means the OS accepted the request; it does *not*
/// mean the user actually sees the target. `opener::open` on macOS
/// forks `/usr/bin/open` and returns as soon as the shell spawns, so
/// a missing browser / broken handler only shows up post-hoc. That's
/// fine for a manual user action but worth knowing when writing tests.
#[tauri::command]
pub async fn shell_open(url: String) -> Result<(), DayseamError> {
    let parsed = url::Url::parse(&url).map_err(|e| DayseamError::InvalidConfig {
        code: error_codes::IPC_SHELL_URL_INVALID.into(),
        message: format!("invalid url `{url}`: {e}"),
    })?;
    if !SHELL_ALLOWED_SCHEMES.contains(&parsed.scheme()) {
        return Err(DayseamError::InvalidConfig {
            code: error_codes::IPC_SHELL_URL_DISALLOWED.into(),
            message: format!(
                "scheme `{}` is not in the allow-list {:?}",
                parsed.scheme(),
                SHELL_ALLOWED_SCHEMES
            ),
        });
    }
    if parsed.scheme() == "file" {
        validate_file_url_path(&url, &parsed)?;
    }
    // `opener::open` is a blocking, spawn-child-process call; push it
    // to the blocking pool so it cannot stall the IPC reactor even if
    // the OS is slow to return (e.g. Spotlight indexing the target).
    tokio::task::spawn_blocking(move || opener::open(&url))
        .await
        .map_err(|e| DayseamError::Internal {
            code: error_codes::IPC_SHELL_OPEN_FAILED.into(),
            message: format!("spawn_blocking join failed: {e}"),
        })?
        .map_err(|e| DayseamError::Internal {
            code: error_codes::IPC_SHELL_OPEN_FAILED.into(),
            message: e.to_string(),
        })?;
    Ok(())
}

// ---- GitLab admin ---------------------------------------------------------

/// One-shot PAT probe used by the `AddGitlabSourceDialog` "Validate"
/// button (Task 3). Hands the paired `host` + `pat` into
/// [`connector_gitlab::auth::validate_pat`], which calls GitLab's
/// `/api/v4/user` endpoint via HTTPS and parses back the numeric user
/// id + username we need to persist on the resulting
/// [`SourceConfig::GitLab`] row.
///
/// The PAT is received as [`IpcSecretString`] — never a bare `String` —
/// so a `tracing::instrument`-captured arg map (or any other `Debug`
/// sink) only ever shows `IpcSecretString(***)` in logs. The bytes
/// are zeroed when the command returns, regardless of whether
/// validation succeeded or surfaced a `gitlab.auth.*` error.
///
/// Error-code ladder (surfaces exactly what the UI's
/// `gitlabErrorCopy` map renders):
///
/// | HTTP / cause               | `DayseamError` code             |
/// |----------------------------|--------------------------------|
/// | 200 OK                     | *(ok)*                         |
/// | 401 Unauthorized           | `gitlab.auth.invalid_token`    |
/// | 403 Forbidden              | `gitlab.auth.missing_scope`    |
/// | DNS / connect refused      | `gitlab.url.dns`               |
/// | TLS handshake failure      | `gitlab.url.tls`               |
/// | 429 Too Many Requests      | `gitlab.rate_limited`          |
/// | 5xx or unknown transport   | `gitlab.upstream_5xx`          |
/// | Shape-changed `/user` body | `gitlab.upstream_shape_changed`|
///
/// The function intentionally does **not** persist the secret, mint a
/// [`SourceId`], or write to the database. The subsequent
/// [`sources_add`] call owns that half of the flow; this command is a
/// pure validator so the dialog can show green-check / red-error
/// feedback before the user commits to creating the source.
#[tauri::command]
pub async fn gitlab_validate_pat(
    host: String,
    pat: IpcSecretString,
) -> Result<GitlabValidationResult, DayseamError> {
    let user = connector_gitlab::auth::validate_pat(&host, pat.expose()).await?;
    Ok(GitlabValidationResult {
        user_id: user.id,
        username: user.username,
    })
}

// ---- Retention ------------------------------------------------------------

#[tauri::command]
pub async fn retention_sweep_now(state: State<'_, AppState>) -> Result<(), DayseamError> {
    let now = Utc::now();
    let cutoff = resolve_cutoff(&state.pool, now).await?;
    let report = retention_sweep(&state.pool, cutoff).await?;
    // Feed the debounce guard so the post-run hook (Task 7.4) does
    // not re-sweep on the very next `report_generate` terminal —
    // we just pruned everything in range.
    state
        .orchestrator
        .retention_schedule()
        .note_external_sweep(now)
        .await;
    let _ = LogRepo::new(state.pool.clone())
        .append(&LogRow {
            ts: Utc::now(),
            level: LogLevel::Info,
            source_id: None,
            message: format!(
                "retention_sweep_now: pruned {} raw_payloads, {} log_entries",
                report.raw_payloads_deleted, report.log_entries_deleted,
            ),
            context: Some(serde_json::json!({ "source": "ipc.retention_sweep_now" })),
        })
        .await;
    Ok(())
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
    use dayseam_orchestrator::{ConnectorRegistry, OrchestratorBuilder, SinkRegistry};
    use dayseam_secrets::InMemoryStore;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn make_state() -> (AppState, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open db");
        let app_bus = AppBus::new();
        let orchestrator = OrchestratorBuilder::new(
            pool.clone(),
            app_bus.clone(),
            ConnectorRegistry::new(),
            SinkRegistry::new(),
        )
        .build()
        .expect("build orchestrator");
        let state = AppState::new(pool, app_bus, Arc::new(InMemoryStore::new()), orchestrator);
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

    // --- shell_open ---------------------------------------------------------
    //
    // We don't actually shell out to the OS in unit tests — that would
    // be flaky and nothing to assert against — so the checks here
    // cover the guard side: validation short-circuits before
    // `opener::open` is ever reached for any URL that isn't in the
    // allow-list, which is the only behaviour the frontend depends on.

    #[tokio::test]
    async fn shell_open_rejects_disallowed_scheme() {
        // `javascript:` is the canonical footgun; if this ever passes
        // we've either widened the allow-list or lost the guard.
        let err = shell_open("javascript:alert(1)".into())
            .await
            .expect_err("javascript scheme must be rejected");
        match err {
            DayseamError::InvalidConfig { code, .. } => {
                assert_eq!(code, error_codes::IPC_SHELL_URL_DISALLOWED);
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_open_rejects_unparseable_url() {
        let err = shell_open("not a url".into())
            .await
            .expect_err("garbage string must be rejected");
        match err {
            DayseamError::InvalidConfig { code, .. } => {
                assert_eq!(code, error_codes::IPC_SHELL_URL_INVALID);
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_open_rejects_file_url_with_traversal() {
        // A `file://` URL with `..` in its path could escape the
        // user's Documents directory; the docstring promises we
        // reject it, and after Phase 2 Task 8 we actually do.
        let err = shell_open("file:///Users/alice/../../etc/passwd".into())
            .await
            .expect_err("file:// with .. must be rejected");
        match err {
            DayseamError::InvalidConfig { code, message } => {
                assert_eq!(code, error_codes::IPC_SHELL_URL_DISALLOWED);
                assert!(
                    message.contains("..") || message.contains("traversal"),
                    "expected traversal message, got: {message}"
                );
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_open_rejects_file_url_with_non_absolute_path() {
        // `file:relative/path` parses with an empty authority and a
        // relative path; `opener::open` would resolve it against
        // the process CWD, which is never what a sink intended.
        let err = shell_open("file:relative/path.md".into())
            .await
            .expect_err("file:// with relative path must be rejected");
        match err {
            DayseamError::InvalidConfig { code, .. } => {
                assert_eq!(code, error_codes::IPC_SHELL_URL_DISALLOWED);
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn shell_allowed_schemes_cover_phase_2_use_cases() {
        // Documentation test: the schemes the evidence popover and
        // save-receipt row rely on must all stay in the allow-list.
        // Adding a new sink that needs `mailto:` belongs in the same
        // change that adds `mailto` here.
        for scheme in ["http", "https", "file", "vscode", "obsidian"] {
            assert!(
                SHELL_ALLOWED_SCHEMES.contains(&scheme),
                "allow-list regression: `{scheme}` is no longer permitted"
            );
        }
    }

    // --- validate_sink_config ---------------------------------------------

    #[test]
    fn validate_sink_config_rejects_empty_dest_dirs() {
        let cfg = SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec![],
            frontmatter: false,
        };
        let err = validate_sink_config(&cfg).expect_err("empty dest_dirs must be rejected");
        match err {
            DayseamError::InvalidConfig { code, .. } => {
                assert_eq!(code, error_codes::IPC_SINK_INVALID_CONFIG);
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_sink_config_rejects_relative_path() {
        let cfg = SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec![PathBuf::from("relative/dir")],
            frontmatter: false,
        };
        let err = validate_sink_config(&cfg).expect_err("relative dest_dir must be rejected");
        match err {
            DayseamError::InvalidConfig { code, message } => {
                assert_eq!(code, error_codes::IPC_SINK_INVALID_CONFIG);
                assert!(message.contains("absolute"), "got: {message}");
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_sink_config_rejects_traversal_segment() {
        let cfg = SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec![PathBuf::from("/Users/alice/../../etc")],
            frontmatter: false,
        };
        let err = validate_sink_config(&cfg).expect_err("`..` traversal must be rejected");
        match err {
            DayseamError::InvalidConfig { code, message } => {
                assert_eq!(code, error_codes::IPC_SINK_INVALID_CONFIG);
                assert!(message.contains(".."), "got: {message}");
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_sink_config_accepts_well_formed_config() {
        let cfg = SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec![PathBuf::from("/Users/alice/Documents/Dayseam")],
            frontmatter: true,
        };
        validate_sink_config(&cfg).expect("well-formed config must validate");
    }

    // --- activity_events_get -----------------------------------------------

    #[tokio::test]
    async fn activity_events_get_returns_empty_for_unknown_ids() {
        let (state, _dir) = make_state().await;
        // Exercise the repo directly — the command is a pure
        // pass-through so repo behaviour is what the UI depends on.
        let repo = ActivityRepo::new(state.pool.clone());
        let missing = Uuid::new_v4();
        let rows = repo.get_many(&[missing]).await.expect("get_many");
        assert!(rows.is_empty(), "missing ids must drop silently, not error");
    }
}
