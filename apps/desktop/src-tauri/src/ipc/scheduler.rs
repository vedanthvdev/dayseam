//! DAY-130 scheduler IPC surface.
//!
//! Four thin Tauri commands:
//!
//! * `scheduler_get_config` — read the persisted [`ScheduleConfig`]
//!   (or [`ScheduleConfig::default`] when nothing has been saved yet).
//! * `scheduler_set_config` — persist the [`ScheduleConfig`] the
//!   Preferences dialog builds. Returns the stored shape so the
//!   frontend's local cache matches the round-tripped JSON.
//! * `scheduler_run_catch_up` — fire the catch-up batch the cold-
//!   start banner asked the user to confirm. Iterates dates oldest-
//!   first and runs each one through the same
//!   [`run_scheduled_action`] path the hourly timer uses.
//! * `scheduler_skip_catch_up` — user clicked *Skip* on the banner.
//!   Records the dates in an in-memory, session-scoped skip set so a
//!   subsequent catch-up scan on the same boot doesn't re-surface
//!   them.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::NaiveDate;
use dayseam_core::{
    DayseamError, ScheduleConfig, SchedulerTriggerKind, WriteReceipt, SCHEDULE_CONFIG_KEY,
};
use dayseam_db::{PersonRepo, SettingsRepo, SinkRepo, SourceIdentityRepo, SourceRepo};
use dayseam_orchestrator::{run_scheduled_action, GenerateRequest, SourceHandle};
use dayseam_report::{DEV_EOD_TEMPLATE_ID, DEV_EOD_TEMPLATE_VERSION};
use tauri::State;
use tokio::sync::Mutex;

use crate::ipc::commands::{build_source_auth, SELF_DEFAULT_DISPLAY_NAME};
use crate::state::AppState;

/// Session-scoped "skip these dates" set. Not persisted: a user who
/// says *Skip* on Monday is allowed to reconsider on Tuesday. Owned
/// by the app as an `Arc<Mutex<_>>` so both the IPC commands and the
/// (future) cold-start catch-up scan share a single view.
#[derive(Clone, Default)]
pub struct SchedulerSkipSet(Arc<Mutex<BTreeSet<NaiveDate>>>);

impl SchedulerSkipSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert_many(&self, dates: impl IntoIterator<Item = NaiveDate>) {
        let mut guard = self.0.lock().await;
        guard.extend(dates);
    }

    pub async fn snapshot(&self) -> BTreeSet<NaiveDate> {
        self.0.lock().await.clone()
    }
}

/// Read the persisted [`ScheduleConfig`]. Returns the default
/// (disabled, Mon–Fri, 18:00) when nothing has been saved yet so the
/// Preferences dialog always has a shape to bind to.
#[tauri::command]
pub async fn scheduler_get_config(
    state: State<'_, AppState>,
) -> Result<ScheduleConfig, DayseamError> {
    load_schedule_config(&state).await
}

/// Persist a [`ScheduleConfig`]. Returns the stored value so the
/// frontend can replace its local cache with the exact shape that
/// round-tripped through `serde`.
#[tauri::command]
pub async fn scheduler_set_config(
    config: ScheduleConfig,
    state: State<'_, AppState>,
) -> Result<ScheduleConfig, DayseamError> {
    SettingsRepo::new(state.pool.clone())
        .set(SCHEDULE_CONFIG_KEY, &config)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "scheduler.config.write".into(),
            message: e.to_string(),
        })?;
    Ok(config)
}

/// Run the user-confirmed catch-up batch. Dates are processed in
/// chronological order; one failure does not short-circuit the rest
/// — each scheduled day stands on its own in `sync_runs`.
#[tauri::command]
pub async fn scheduler_run_catch_up(
    dates: Vec<NaiveDate>,
    state: State<'_, AppState>,
) -> Result<Vec<WriteReceipt>, DayseamError> {
    if dates.is_empty() {
        return Ok(Vec::new());
    }
    let cfg = load_schedule_config(&state).await?;
    let Some(sink_id) = cfg.sink_id else {
        return Err(DayseamError::InvalidConfig {
            code: "scheduler.no_sink".into(),
            message: "Scheduler has no sink configured yet".into(),
        });
    };
    let sink = SinkRepo::new(state.pool.clone())
        .get(&sink_id)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "scheduler.sink.get".into(),
            message: e.to_string(),
        })?
        .ok_or_else(|| DayseamError::InvalidConfig {
            code: "scheduler.sink_missing".into(),
            message: format!("Scheduler points at sink {sink_id} which no longer exists"),
        })?;

    let mut ordered = dates;
    ordered.sort();
    let mut all_receipts = Vec::new();
    for date in ordered {
        let request = build_scheduler_request(&state, &cfg, date).await?;
        match run_scheduled_action(
            &state.orchestrator,
            request,
            &sink,
            SchedulerTriggerKind::CatchUp,
        )
        .await
        {
            Ok(mut receipts) => all_receipts.append(&mut receipts),
            Err(err) => {
                tracing::warn!(%date, error = %err, "scheduler catch-up run failed; continuing");
            }
        }
    }
    Ok(all_receipts)
}

/// Record a banner dismissal. The passed dates are added to the
/// in-memory skip set for the rest of this session; a subsequent
/// catch-up scan will not re-surface them until the app restarts.
#[tauri::command]
pub async fn scheduler_skip_catch_up(
    dates: Vec<NaiveDate>,
    state: State<'_, AppState>,
) -> Result<(), DayseamError> {
    state.scheduler_skip.insert_many(dates).await;
    Ok(())
}

async fn load_schedule_config(state: &AppState) -> Result<ScheduleConfig, DayseamError> {
    SettingsRepo::new(state.pool.clone())
        .get::<ScheduleConfig>(SCHEDULE_CONFIG_KEY)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "scheduler.config.read".into(),
            message: e.to_string(),
        })
        .map(Option::unwrap_or_default)
}

/// Assemble a [`GenerateRequest`] from the app's current source
/// inventory. The scheduler always generates reports across *all*
/// connected sources — v1 does not support per-schedule source
/// filtering.
pub(crate) async fn build_scheduler_request(
    state: &AppState,
    cfg: &ScheduleConfig,
    date: NaiveDate,
) -> Result<GenerateRequest, DayseamError> {
    let person = PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "scheduler.persons.bootstrap_self".into(),
            message: e.to_string(),
        })?;

    let source_repo = SourceRepo::new(state.pool.clone());
    let identity_repo = SourceIdentityRepo::new(state.pool.clone());
    let sources = source_repo
        .list()
        .await
        .map_err(|e| DayseamError::Internal {
            code: "scheduler.sources.list".into(),
            message: e.to_string(),
        })?;

    let mut handles: Vec<SourceHandle> = Vec::with_capacity(sources.len());
    for source in &sources {
        let auth = build_source_auth(state, source)?;
        let ids = identity_repo
            .list_for_source(person.id, &source.id)
            .await
            .map_err(|e| DayseamError::Internal {
                code: "scheduler.identities.list".into(),
                message: e.to_string(),
            })?;
        handles.push(SourceHandle {
            source_id: source.id,
            kind: source.kind,
            auth,
            source_identities: ids,
        });
    }

    let template_id = cfg
        .template_id
        .clone()
        .unwrap_or_else(|| DEV_EOD_TEMPLATE_ID.to_string());

    Ok(GenerateRequest {
        person,
        sources: handles,
        date,
        template_id,
        template_version: DEV_EOD_TEMPLATE_VERSION.to_string(),
        verbose_mode: false,
    })
}
