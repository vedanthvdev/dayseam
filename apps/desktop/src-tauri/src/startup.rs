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
use dayseam_core::{
    DayseamError, LogLevel, SourceConfig, SourceIdentity, SourceIdentityKind, SourceKind,
};
use dayseam_db::{
    open, LocalRepoRepo, LogRepo, LogRow, PersonRepo, SourceIdentityRepo, SourceRepo,
};
use dayseam_events::AppBus;
use dayseam_orchestrator::{
    default_registries, DefaultRegistryConfig, Orchestrator, OrchestratorBuilder,
};
use dayseam_secrets::{KeychainStore, SecretStore};
use sqlx::SqlitePool;
use uuid::Uuid;

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
    backfill_gitlab_self_identities(&pool).await;

    let app_bus = AppBus::new();
    let secrets: Arc<dyn SecretStore> = Arc::new(KeychainStore::new());

    let orchestrator = build_orchestrator(pool.clone(), app_bus.clone()).await?;
    run_startup_maintenance(&orchestrator, &pool).await;

    Ok(AppState::new(pool, app_bus, secrets, orchestrator))
}

/// DAY-71 backfill: for every persisted GitLab source, make sure a
/// [`SourceIdentityKind::GitLabUserId`] [`SourceIdentity`] row exists
/// that maps the source's numeric `user_id` to the self-[`Person`].
///
/// Why this runs on every boot and not just once:
///
/// * Pre-DAY-71 installs have a `sources` row but no matching
///   identity. Without this pass they stay broken forever (reports
///   render empty) unless the user deletes and re-adds the source
///   — undiscoverable from the UI.
/// * `sources_update` now seeds the identity on every save, but a
///   user who hit the bug and never reconnected would not have
///   exercised that path. The boot-time pass closes that window.
/// * [`SourceIdentityRepo::ensure`] is idempotent on the natural
///   key `(person_id, source_id, kind, external_actor_id)`, so
///   running it every boot is O(sources) work against an index.
///
/// Best-effort: failures here must not block the app from booting
/// (the user's next `sources_update` or their attempt to generate a
/// report will surface a real error in context). We log the failure
/// mode so post-mortem SRE work has a breadcrumb.
async fn backfill_gitlab_self_identities(pool: &SqlitePool) {
    let sources = match SourceRepo::new(pool.clone()).list().await {
        Ok(sources) => sources,
        Err(err) => {
            tracing::warn!(%err, "backfill: source listing failed; skipping identity seeding");
            return;
        }
    };

    let gitlab_sources: Vec<(uuid::Uuid, i64)> = sources
        .into_iter()
        .filter_map(|source| match (&source.kind, source.config) {
            (SourceKind::GitLab, SourceConfig::GitLab { user_id, .. }) => {
                Some((source.id, user_id))
            }
            _ => None,
        })
        .collect();
    if gitlab_sources.is_empty() {
        return;
    }

    let person_id = match PersonRepo::new(pool.clone()).bootstrap_self("Me").await {
        Ok(p) => p.id,
        Err(err) => {
            tracing::warn!(%err, "backfill: persons.bootstrap_self failed; skipping identity seeding");
            return;
        }
    };

    let identity_repo = SourceIdentityRepo::new(pool.clone());
    for (source_id, user_id) in gitlab_sources {
        let identity = SourceIdentity {
            id: Uuid::new_v4(),
            person_id,
            source_id: Some(source_id),
            kind: SourceIdentityKind::GitLabUserId,
            external_actor_id: user_id.to_string(),
        };
        match identity_repo.ensure(&identity).await {
            Ok(true) => {
                tracing::info!(
                    %source_id,
                    user_id,
                    "backfill: seeded missing GitLabUserId self-identity"
                );
            }
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(
                    %err,
                    %source_id,
                    user_id,
                    "backfill: failed to ensure GitLabUserId self-identity"
                );
            }
        }
    }
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
    use chrono::Utc;
    use dayseam_core::{Source, SourceHealth};
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

    // --- DAY-71: startup identity backfill --------------------------------
    //
    // Pre-DAY-71 installs carried GitLab sources without a matching
    // `GitLabUserId` [`SourceIdentity`], which silently collapsed
    // every generated report to "No tracked activity". The boot-time
    // backfill is the only path that fixes existing installs without
    // asking the user to delete-and-re-add their source, so it's worth
    // protecting with an explicit integration test.

    async fn insert_gitlab_source(pool: &SqlitePool, id: Uuid, user_id: i64) {
        SourceRepo::new(pool.clone())
            .insert(&Source {
                id,
                kind: SourceKind::GitLab,
                label: "gitlab.example.com".into(),
                config: SourceConfig::GitLab {
                    base_url: "https://gitlab.example.com".into(),
                    user_id,
                    username: "vedanth".into(),
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert gitlab source");
    }

    #[tokio::test]
    async fn backfill_seeds_missing_gitlab_user_id_identity() {
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let source_id = Uuid::new_v4();
        insert_gitlab_source(&pool, source_id, 291).await;

        // Pre-condition: no `GitLabUserId` identities exist yet —
        // this is the exact shape of a pre-DAY-71 install.
        let person = PersonRepo::new(pool.clone())
            .bootstrap_self("Me")
            .await
            .expect("self");
        let before = SourceIdentityRepo::new(pool.clone())
            .list_for_source(person.id, &source_id)
            .await
            .expect("list before");
        assert!(
            before
                .iter()
                .all(|r| r.kind != SourceIdentityKind::GitLabUserId),
            "precondition: no GitLabUserId rows exist yet"
        );

        backfill_gitlab_self_identities(&pool).await;

        let after = SourceIdentityRepo::new(pool.clone())
            .list_for_source(person.id, &source_id)
            .await
            .expect("list after");
        let seeded: Vec<_> = after
            .iter()
            .filter(|r| r.kind == SourceIdentityKind::GitLabUserId && r.external_actor_id == "291")
            .collect();
        assert_eq!(
            seeded.len(),
            1,
            "backfill must seed exactly one matching identity, got rows: {after:?}"
        );
    }

    #[tokio::test]
    async fn backfill_is_idempotent_across_boots() {
        // Every boot runs this pass; a regression that inserts a
        // fresh row each time would pollute the identities table
        // and eventually throw a UNIQUE-constraint error. Guard it.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let source_id = Uuid::new_v4();
        insert_gitlab_source(&pool, source_id, 291).await;

        backfill_gitlab_self_identities(&pool).await;
        backfill_gitlab_self_identities(&pool).await;
        backfill_gitlab_self_identities(&pool).await;

        let person = PersonRepo::new(pool.clone())
            .bootstrap_self("Me")
            .await
            .expect("self");
        let rows = SourceIdentityRepo::new(pool)
            .list_for_source(person.id, &source_id)
            .await
            .expect("list");
        let count = rows
            .iter()
            .filter(|r| r.kind == SourceIdentityKind::GitLabUserId && r.external_actor_id == "291")
            .count();
        assert_eq!(count, 1, "three boots must leave exactly one seeded row");
    }

    #[tokio::test]
    async fn backfill_skips_when_no_gitlab_sources_present() {
        // A LocalGit-only install must not bootstrap the self-person
        // (that's a side-effect we want to keep scoped to installs
        // that actually have a GitLab source to seed for), and must
        // not produce any identity rows.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");

        backfill_gitlab_self_identities(&pool).await;

        // `get_self` returns `None` if nothing triggered a
        // bootstrap; confirm the backfill did not eagerly create a
        // self-person for a DB that does not need one.
        let existing = PersonRepo::new(pool).get_self().await.expect("get_self");
        assert!(
            existing.is_none(),
            "no GitLab sources ⇒ no bootstrap, got person row: {existing:?}"
        );
    }
}
