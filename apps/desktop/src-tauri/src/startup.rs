//! App startup helpers — everything that needs to happen exactly once
//! between "Tauri is about to call `setup`" and "the window is
//! allowed to make IPC calls".
//!
//! Factored out of `main.rs` so integration tests can exercise the
//! same code path without running a real Tauri runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Offset;
use connector_gitlab::GitlabSourceCfg;
use dayseam_core::{DayseamError, LogLevel, SourceConfig, SourceKind};
use dayseam_db::{open, LocalRepoRepo, LogRepo, LogRow, SourceRepo};
use dayseam_events::AppBus;
use dayseam_orchestrator::{
    default_registries, DefaultRegistryConfig, Orchestrator, OrchestratorBuilder,
};
use dayseam_secrets::{KeychainStore, SecretStore};
use sqlx::SqlitePool;

use crate::state::AppState;

/// Fixed subdirectory inside the OS "app data" dir that Dayseam owns.
/// Matches the Tauri bundle identifier prefix so multiple installs
/// (stable, alpha, custom) can coexist without stepping on one
/// another.
const DATA_SUBDIR: &str = "dev.dayseam.desktop";
const DB_FILENAME: &str = "state.db";

/// Resolve the per-platform application-data directory Dayseam writes
/// to. Uses the same logic as Tauri so the database sits next to the
/// updater cache, the logs, and anything else the runtime may add in
/// a future phase.
///
/// Falls back to `./<DATA_SUBDIR>/` when no platform directory can be
/// resolved (should only happen in very unusual headless CI setups).
#[must_use]
pub fn default_data_dir() -> PathBuf {
    if let Some(base) = dirs_like_app_data() {
        return base.join(DATA_SUBDIR);
    }
    PathBuf::from(DATA_SUBDIR)
}

#[cfg(target_os = "macos")]
fn dirs_like_app_data() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn dirs_like_app_data() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
}

#[cfg(target_os = "windows")]
fn dirs_like_app_data() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn dirs_like_app_data() -> Option<PathBuf> {
    None
}

/// Build an [`AppState`] from a data directory. Creates the directory
/// if missing, opens the database (running migrations), writes a
/// single startup log row so the empty-state of the log drawer is
/// informative ("Dayseam started at {ts}"), and returns the populated
/// state.
pub async fn build_app_state(data_dir: &Path) -> Result<AppState, DayseamError> {
    tokio::fs::create_dir_all(data_dir)
        .await
        .map_err(|e| DayseamError::Io {
            code: "startup.data_dir".into(),
            path: Some(data_dir.to_path_buf()),
            message: e.to_string(),
        })?;

    let pool = open(&data_dir.join(DB_FILENAME))
        .await
        .map_err(|e| DayseamError::Io {
            code: "startup.db_open".into(),
            path: Some(data_dir.join(DB_FILENAME)),
            message: e.to_string(),
        })?;

    record_startup_log(&pool).await;

    let app_bus = AppBus::new();
    let secrets: Arc<dyn SecretStore> = Arc::new(KeychainStore::new());

    let orchestrator = build_orchestrator(pool.clone(), app_bus.clone()).await?;
    run_startup_maintenance(&orchestrator, &pool).await;

    Ok(AppState::new(pool, app_bus, secrets, orchestrator))
}

/// Build the process-wide [`Orchestrator`] with registries populated
/// from the persisted source and local-repo rows.
///
/// **Boot-only contract (Task 6 PR-A).** The registry is a snapshot of
/// the DB at the moment `build_orchestrator` runs. Sources added or
/// mutated after startup do *not* flow back into the registry; the
/// Task 6 UI commands (`sources_add`, `sources_update`,
/// `sources_delete`) emit a `ToastEvent` telling the user to restart
/// the app for the change to take effect. The trade-off is explicit:
/// we avoid a deeper refactor of `Orchestrator` to put the registries
/// behind a lock, and pay it back in a later PR (see CHANGELOG).
async fn build_orchestrator(
    pool: SqlitePool,
    app_bus: AppBus,
) -> Result<Orchestrator, DayseamError> {
    let cfg = resolve_registry_config(&pool).await?;
    let (connectors, sinks) = default_registries(cfg);
    OrchestratorBuilder::new(pool, app_bus, connectors, sinks).build()
}

/// Read the persisted `sources` + `local_repos` rows and fold them
/// into the [`DefaultRegistryConfig`] the shipping connector/sink
/// defaults expect.
///
/// The local timezone comes from [`chrono::Local`] at startup; travel
/// or DST between boots is a caller concern (the connector buckets
/// every commit into a day with *this* offset).
///
/// Sink destination directories are deliberately left empty here: the
/// `MarkdownFileSink` constructor's only `dest_dirs` use is sweeping
/// orphan temp files, and the actual write target is carried on each
/// row's [`dayseam_core::SinkConfig::MarkdownFile::dest_dirs`]. The
/// registry therefore does not need per-sink-row state.
async fn resolve_registry_config(pool: &SqlitePool) -> Result<DefaultRegistryConfig, DayseamError> {
    let sources =
        SourceRepo::new(pool.clone())
            .list()
            .await
            .map_err(|e| DayseamError::Internal {
                code: "startup.sources_list".into(),
                message: e.to_string(),
            })?;

    let local_repo_repo = LocalRepoRepo::new(pool.clone());
    let mut scan_roots: Vec<PathBuf> = Vec::new();
    let mut private_roots: Vec<PathBuf> = Vec::new();
    let mut gitlab_sources: Vec<GitlabSourceCfg> = Vec::new();
    for source in sources {
        match (&source.kind, &source.config) {
            (
                SourceKind::LocalGit,
                SourceConfig::LocalGit {
                    scan_roots: roots, ..
                },
            ) => {
                scan_roots.extend(roots.iter().cloned());
                let repos = local_repo_repo
                    .list_for_source(&source.id)
                    .await
                    .map_err(|e| DayseamError::Internal {
                        code: "startup.local_repos_list".into(),
                        message: e.to_string(),
                    })?;
                for repo in repos {
                    if repo.is_private {
                        private_roots.push(repo.path);
                    }
                }
            }
            (
                SourceKind::GitLab,
                SourceConfig::GitLab {
                    base_url, user_id, ..
                },
            ) => {
                gitlab_sources.push(GitlabSourceCfg {
                    source_id: source.id,
                    base_url: base_url.clone(),
                    user_id: *user_id,
                });
            }
            // Kind/config mismatch is a core-level invariant violation
            // (serde round-trip prevents it); skip defensively rather
            // than panic at startup.
            _ => {}
        }
    }

    Ok(DefaultRegistryConfig {
        local_git_scan_roots: scan_roots,
        local_git_private_roots: private_roots,
        local_tz: chrono::Local::now().offset().fix(),
        markdown_dest_dirs: Vec::new(),
        gitlab_sources,
    })
}

/// Run [`Orchestrator::startup`] and log the outcome. Failures are
/// logged and swallowed: a sweep error must not block the app from
/// booting, and the next boot retries the same work.
async fn run_startup_maintenance(orchestrator: &Orchestrator, pool: &SqlitePool) {
    match orchestrator.startup().await {
        Ok(report) => {
            tracing::info!(
                retention_default_installed = report.retention_default_installed,
                crashed_runs_recovered = report.crashed_runs_recovered,
                raw_payloads_deleted = report.retention.raw_payloads_deleted,
                log_entries_deleted = report.retention.log_entries_deleted,
                "orchestrator startup maintenance completed",
            );
            let message = format!(
                "Startup sweep: recovered {crashed} crashed run(s); pruned {raw} raw_payloads, {logs} log_entries",
                crashed = report.crashed_runs_recovered,
                raw = report.retention.raw_payloads_deleted,
                logs = report.retention.log_entries_deleted,
            );
            let _ = LogRepo::new(pool.clone())
                .append(&LogRow {
                    ts: chrono::Utc::now(),
                    level: LogLevel::Info,
                    source_id: None,
                    message,
                    context: Some(serde_json::json!({ "source": "startup.orchestrator" })),
                })
                .await;
        }
        Err(err) => {
            tracing::warn!(error = %err, "orchestrator startup maintenance failed");
            let _ = LogRepo::new(pool.clone())
                .append(&LogRow {
                    ts: chrono::Utc::now(),
                    level: LogLevel::Warn,
                    source_id: None,
                    message: format!("Startup sweep failed: {err}"),
                    context: Some(serde_json::json!({
                        "source": "startup.orchestrator",
                    })),
                })
                .await;
        }
    }
}

async fn record_startup_log(pool: &SqlitePool) {
    let repo = LogRepo::new(pool.clone());
    // Best-effort — a startup log failing to write is not worth
    // refusing to boot. The next successful write still gives the user
    // a sensible log drawer.
    let _ = repo
        .append(&LogRow {
            ts: chrono::Utc::now(),
            level: LogLevel::Info,
            source_id: None,
            message: "Dayseam started".into(),
            context: Some(serde_json::json!({ "source": "startup" })),
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn build_app_state_writes_the_startup_log_entry() {
        let dir = TempDir::new().expect("temp dir");
        let state = build_app_state(dir.path()).await.expect("build state");
        let repo = LogRepo::new(state.pool.clone());
        let rows = repo
            .tail(chrono::DateTime::<chrono::Utc>::MIN_UTC, 10)
            .await
            .expect("tail");
        assert!(
            rows.iter().any(|r| r.message == "Dayseam started"),
            "startup log missing: {:?}",
            rows.iter().map(|r| &r.message).collect::<Vec<_>>()
        );
    }
}
