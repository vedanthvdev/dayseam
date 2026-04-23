//! App startup helpers — everything that needs to happen exactly once
//! between "Tauri is about to call `setup`" and "the window is
//! allowed to make IPC calls".
//!
//! Factored out of `main.rs` so integration tests can exercise the
//! same code path without running a real Tauri runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Offset;
use connector_confluence::{ConfluenceConfig, ConfluenceSourceCfg};
use connector_github::{GithubConfig, GithubSourceCfg};
use connector_gitlab::GitlabSourceCfg;
use connector_jira::{JiraConfig, JiraSourceCfg};
use dayseam_core::{
    DayseamError, LogLevel, SourceConfig, SourceIdentity, SourceIdentityKind, SourceKind,
};
use dayseam_db::{
    open, registered_repairs, LocalRepoRepo, LogRepo, LogRow, PersonRepo, SourceIdentityRepo,
    SourceRepo,
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
    backfill_atlassian_self_identities(&pool).await;
    run_registered_repairs(&pool).await;

    let app_bus = AppBus::new();
    let secrets: Arc<dyn SecretStore> = Arc::new(KeychainStore::new());

    let orchestrator = build_orchestrator(pool.clone(), app_bus.clone()).await?;
    run_startup_maintenance(&orchestrator, &pool).await;
    // DOGFOOD-v0.4-07: the orphan-secret audit probes every distinct
    // `secret_ref` stored for a source. Each probe goes through the
    // macOS Keychain and — for an unsigned / ad-hoc-signed build
    // where the ACL hasn't been "Always Allow"-ed yet — triggers a
    // password prompt. Running it synchronously here meant the user
    // saw up to N password prompts before the app window appeared;
    // dogfooders described it as "the app asks for my laptop
    // password 4 times to open". We now spawn the audit as a
    // detached task so the window renders immediately; any
    // macOS-level prompts the audit triggers show up *after* the
    // app is visible and no longer gate the cold-boot UX. The
    // audit's own output is unchanged — it still emits a warning
    // log per orphan and is still covered by the dedicated
    // Keychain-backed tests below.
    //
    // DAY-103 F-10: make the detached task observable. The previous
    // fire-and-forget `spawn` discarded both the orphan count
    // (useful for log forensics) and any panic inside the audit —
    // a panic would have gone unlogged and invisible. We now
    // supervise the audit from a second `spawn` that awaits the
    // first's `JoinHandle`, so a panic turns into a single
    // `tracing::error!` line and a clean completion logs the
    // orphan count at `info!`. This only relies on tauri's
    // `async_runtime::spawn` (which wraps tokio's `JoinHandle`
    // with `is_panic()` semantics) — no new dependency needed.
    {
        let pool = pool.clone();
        let secrets = secrets.clone();
        let audit_handle = tauri::async_runtime::spawn(async move {
            audit_orphan_secrets(&pool, secrets.as_ref()).await
        });
        tauri::async_runtime::spawn(async move {
            match audit_handle.await {
                Ok(orphans) => tracing::info!(
                    orphans,
                    "orphan-secret audit completed (deferred, post-window-show)"
                ),
                Err(join_err) => {
                    // `JoinError::Display` surfaces whether the task
                    // panicked vs. was cancelled, plus the panic
                    // payload when it's a string. That's enough
                    // breadcrumb for a post-mortem to find the
                    // failing probe without extra scaffolding.
                    tracing::error!(
                        join_err = %join_err,
                        "orphan-secret audit task failed to complete; swallowed by the \
                         deferred task runner but recorded here for post-mortem",
                    );
                }
            }
        });
    }

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

/// CONS-v0.2-01 backfill: Atlassian counterpart to
/// [`backfill_gitlab_self_identities`].
///
/// Unlike the GitLab path, the `account_id` is not recoverable from
/// `SourceConfig` — it's only ever surfaced by a live
/// `/rest/api/3/myself` probe at add-time and then persisted on the
/// `source_identities` row. That means this backfill can only
/// *re-assert* an existing identity (a cheap no-op that exercises
/// the idempotency guarantee and guards against future regressions
/// where `source_identities` rows get out of sync), and must fall
/// back to a structured warning for any Atlassian source whose
/// identity row went missing. Manual DB surgery is the only way to
/// reach that state today, but surfacing it in logs is strictly
/// better than silently shipping empty reports.
async fn backfill_atlassian_self_identities(pool: &SqlitePool) {
    let sources = match SourceRepo::new(pool.clone()).list().await {
        Ok(sources) => sources,
        Err(err) => {
            tracing::warn!(
                %err,
                "backfill(atlassian): source listing failed; skipping identity seeding"
            );
            return;
        }
    };

    let atlassian_sources: Vec<(uuid::Uuid, SourceKind)> = sources
        .into_iter()
        .filter(|s| matches!(s.kind, SourceKind::Jira | SourceKind::Confluence))
        .map(|s| (s.id, s.kind))
        .collect();
    if atlassian_sources.is_empty() {
        return;
    }

    let person_id = match PersonRepo::new(pool.clone()).bootstrap_self("Me").await {
        Ok(p) => p.id,
        Err(err) => {
            tracing::warn!(
                %err,
                "backfill(atlassian): persons.bootstrap_self failed; skipping identity seeding"
            );
            return;
        }
    };

    let identity_repo = SourceIdentityRepo::new(pool.clone());
    for (source_id, kind) in atlassian_sources {
        let rows = match identity_repo.list_for_source(person_id, &source_id).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(
                    %err,
                    %source_id,
                    ?kind,
                    "backfill(atlassian): list_for_source failed; skipping this source"
                );
                continue;
            }
        };
        let existing = rows
            .into_iter()
            .find(|r| r.kind == SourceIdentityKind::AtlassianAccountId);
        let Some(existing) = existing else {
            tracing::warn!(
                %source_id,
                ?kind,
                "backfill(atlassian): no AtlassianAccountId identity on file — reports will \
                 silently skip this source's events until the user reconnects it",
            );
            continue;
        };
        match identity_repo.ensure(&existing).await {
            Ok(true) => {
                tracing::info!(
                    %source_id,
                    ?kind,
                    external_actor_id = %existing.external_actor_id,
                    "backfill(atlassian): re-seeded AtlassianAccountId self-identity"
                );
            }
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(
                    %err,
                    %source_id,
                    ?kind,
                    "backfill(atlassian): failed to ensure AtlassianAccountId self-identity"
                );
            }
        }
    }
}

/// Run every [`dayseam_db::SerdeDefaultRepair`] the workspace
/// registers. This is the v0.3 generalisation of the v0.2.1 one-off
/// Confluence-email backfill (CORR-v0.2-08 / DAY-88): each repair
/// lives next to the data it owns (`crates/dayseam-db/src/repairs/`)
/// and startup just iterates. Adding a new data-shape recovery is
/// one file under `repairs/` plus one line in `registered_repairs()`
/// — startup never needs to learn the new name.
///
/// Each repair's idempotency is its own responsibility; a repair
/// that fails is logged and skipped so the next one still runs.
/// That matches the pre-DAY-88 behaviour of the inlined backfill:
/// any error path already `tracing::warn!`ed + returned.
async fn run_registered_repairs(pool: &SqlitePool) {
    for repair in registered_repairs() {
        if let Err(err) = repair.run(pool).await {
            tracing::warn!(
                %err,
                repair = repair.name(),
                "serde-default repair returned an error; skipping",
            );
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
    let mut jira_sources: Vec<JiraSourceCfg> = Vec::new();
    let mut confluence_sources: Vec<ConfluenceSourceCfg> = Vec::new();
    let mut github_sources: Vec<GithubSourceCfg> = Vec::new();
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
            // DOG-v0.2-02: hydrate the Jira / Confluence muxes at
            // boot. v0.2.0 left these two arms as `Vec::new()` with a
            // "comes later in DAY-82" comment; the dialog then *did*
            // land and wrote rows, but the muxes stayed empty, so
            // every post-restart `report_generate` for an Atlassian
            // source hit `source_not_found` in the connector
            // registry. Malformed rows (workspace URL that no longer
            // parses — e.g. manually hand-edited) are logged and
            // skipped rather than crashing boot; the UI surfaces the
            // same source as "Unchecked" until the user fixes it.
            (
                SourceKind::Jira,
                SourceConfig::Jira {
                    workspace_url,
                    email,
                },
            ) => match JiraConfig::from_raw(workspace_url, email) {
                Ok(config) => jira_sources.push(JiraSourceCfg {
                    source_id: source.id,
                    config,
                }),
                Err(err) => tracing::warn!(
                    source_id = %source.id,
                    workspace_url = %workspace_url,
                    error = %err,
                    "skipping Jira source with unparseable workspace_url at startup",
                ),
            },
            (SourceKind::Confluence, SourceConfig::Confluence { workspace_url, .. }) => {
                match ConfluenceConfig::from_raw(workspace_url) {
                    Ok(config) => confluence_sources.push(ConfluenceSourceCfg {
                        source_id: source.id,
                        config,
                    }),
                    Err(err) => tracing::warn!(
                        source_id = %source.id,
                        workspace_url = %workspace_url,
                        error = %err,
                        "skipping Confluence source with unparseable workspace_url at startup",
                    ),
                }
            }
            // DAY-95: hydrate the GitHub mux at boot the same way the
            // Atlassian muxes above do. Malformed `api_base_url` rows
            // (e.g. hand-edited in the SQLite file) are logged and
            // skipped rather than crashing boot; the UI surfaces the
            // source as "Unchecked" until the user fixes it — the
            // DOG-v0.2-02 post-mortem that put this pattern in place
            // applies verbatim to GitHub.
            (SourceKind::GitHub, SourceConfig::GitHub { api_base_url }) => {
                match GithubConfig::from_raw(api_base_url) {
                    Ok(config) => github_sources.push(GithubSourceCfg {
                        source_id: source.id,
                        config,
                    }),
                    Err(err) => tracing::warn!(
                        source_id = %source.id,
                        api_base_url = %api_base_url,
                        error = %err,
                        "skipping GitHub source with unparseable api_base_url at startup",
                    ),
                }
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
        jira_sources,
        confluence_sources,
        github_sources,
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

/// DAY-81 orphan-secret audit. For every distinct `secret_ref`
/// persisted on the `sources` table, probe the keychain to check the
/// slot is actually readable. Missing slots are logged as warnings;
/// we deliberately do **not** auto-fix either side of the mismatch
/// because both the DB row and the keychain row can be the correct
/// source of truth in different contexts:
///
/// * A DB row pointing at a keychain slot the user (or a keyring GC
///   in a brittle OS update) removed — the source is unusable and
///   the user will hit a reconnect-style error the moment they try
///   to sync it. We log so post-mortem traces see it on boot; we
///   don't delete the `sources` row because the user may be about
///   to fix the keychain out-of-band.
/// * A keychain row the DB no longer references — harmless (no
///   source can read it); we can't enumerate keychain entries
///   portably from Rust anyway, so the detector is deliberately
///   DB-driven.
///
/// The counter-part to this pass is `SourceRepo::delete`'s
/// transactional "is this the last reference?" check (DAY-81), which
/// is the *new-install* half of the "no dangling keychain rows"
/// invariant. This function is the *existing-install* half: if the
/// user installed a pre-DAY-81 build, shared a PAT between Jira and
/// Confluence, then removed one of the two under the old delete
/// path, they would have ended up with a surviving source whose
/// `secret_ref` no longer resolved. This pass surfaces that on next
/// boot so the user gets actionable logs instead of a silent-empty
/// report.
///
/// Returns the number of orphan refs detected (never an error —
/// audit failures are surfaced purely through `tracing::warn!`).
///
/// DAY-88 / CORR-v0.2-01 (narrowed) extends the v0.2 warning line
/// with a `source_id` field. The v0.2 warning said *"a keychain
/// slot is no longer readable"* but stopped short of naming which
/// source rows were affected; a user following the log drawer to
/// find the Reconnect chip had to open every Atlassian row to
/// guess. We now iterate the full `sources` list once and emit
/// one warn line per affected `source_id`, so the user can jump
/// straight to the row that needs reconnecting.
async fn audit_orphan_secrets(pool: &SqlitePool, secrets: &dyn SecretStore) -> usize {
    // Pull the full list once so the warning can name the
    // `source_id` that depends on each orphan slot. Per-secret-ref
    // listing (`distinct_secret_refs`) was enough to detect the
    // problem, but "which source?" is what the user actually needs
    // to reconnect — see CORR-v0.2-01 in DAY-88.
    let sources = match SourceRepo::new(pool.clone()).list().await {
        Ok(sources) => sources,
        Err(err) => {
            tracing::warn!(
                %err,
                "orphan-secret audit: listing sources failed; skipping",
            );
            return 0;
        }
    };

    // Group sources by their shared `secret_ref` so a slot used by
    // both a Jira row and its Confluence sibling (Journey A) probes
    // once and attributes to both rows in the warning.
    use std::collections::BTreeMap;
    let mut by_ref: BTreeMap<(String, String), Vec<Uuid>> = BTreeMap::new();
    for source in &sources {
        if let Some(sr) = source.secret_ref.as_ref() {
            let key = (sr.keychain_service.clone(), sr.keychain_account.clone());
            by_ref.entry(key).or_default().push(source.id);
        }
    }

    let mut orphans = 0usize;
    for ((service, account), source_ids) in by_ref {
        let key = crate::ipc::commands::secret_store_key(&dayseam_core::SecretRef {
            keychain_service: service.clone(),
            keychain_account: account.clone(),
        });
        match secrets.get(&key) {
            Ok(Some(_)) => {}
            Ok(None) => {
                orphans += 1;
                for source_id in &source_ids {
                    tracing::warn!(
                        source_id = %source_id,
                        service = %service,
                        account = %account,
                        "orphan-secret audit: source row references a keychain slot the \
                         store can't read — source will fail to authenticate until the \
                         user reconnects"
                    );
                }
            }
            Err(err) => {
                // Probe errors are not treated as orphans — an
                // unhealthy keychain (locked, permission denied)
                // could otherwise stampede the warn log with rows
                // that are actually fine. One line per probe error,
                // no orphan count bump. Still attribute to all
                // affected source_ids so a transient error points
                // the user at the same set of rows a real orphan
                // would.
                for source_id in &source_ids {
                    tracing::warn!(
                        %err,
                        source_id = %source_id,
                        service = %service,
                        account = %account,
                        "orphan-secret audit: keychain probe failed; skipping this ref"
                    );
                }
            }
        }
    }
    if orphans > 0 {
        tracing::warn!(
            orphans,
            "orphan-secret audit: {orphans} keychain slot(s) no longer readable"
        );
    }
    orphans
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

    // --- DOG-v0.2-02: Atlassian mux hydration on boot --------------------
    //
    // v0.2.0 left `jira_sources` / `confluence_sources` hard-coded to
    // `Vec::new()` with a "wait for DAY-82" comment. DAY-82 *did*
    // ship the Add-Source dialog, but startup never caught up — so a
    // fresh install added an Atlassian source, restarted, and still
    // hit `connector.source_not_found` on every report because the
    // mux had no entry for the row. This test pins the post-fix
    // contract: a persisted Jira / Confluence row must be visible to
    // its mux after `resolve_registry_config` runs.

    #[tokio::test]
    async fn resolve_registry_config_hydrates_atlassian_sources() {
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");

        let jira_id = Uuid::new_v4();
        SourceRepo::new(pool.clone())
            .insert(&Source {
                id: jira_id,
                kind: SourceKind::Jira,
                label: "Jira — acme".into(),
                config: SourceConfig::Jira {
                    workspace_url: "https://acme.atlassian.net".into(),
                    email: "me@acme.com".into(),
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert jira");

        let conf_id = Uuid::new_v4();
        SourceRepo::new(pool.clone())
            .insert(&Source {
                id: conf_id,
                kind: SourceKind::Confluence,
                label: "Confluence — acme".into(),
                config: SourceConfig::Confluence {
                    workspace_url: "https://acme.atlassian.net".into(),
                    email: "me@acme.com".into(),
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert confluence");

        let cfg = resolve_registry_config(&pool).await.expect("resolve");

        assert_eq!(
            cfg.jira_sources.len(),
            1,
            "boot must hydrate the persisted Jira row into the mux config"
        );
        assert_eq!(cfg.jira_sources[0].source_id, jira_id);
        assert_eq!(
            cfg.confluence_sources.len(),
            1,
            "boot must hydrate the persisted Confluence row into the mux config"
        );
        assert_eq!(cfg.confluence_sources[0].source_id, conf_id);
    }

    #[tokio::test]
    async fn resolve_registry_config_skips_atlassian_rows_with_unparseable_url() {
        // Defensive: a hand-edited or pre-DAY-84 row whose
        // `workspace_url` is no longer parseable must not crash boot.
        // The bad row is logged-and-skipped; siblings still hydrate.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");

        SourceRepo::new(pool.clone())
            .insert(&Source {
                id: Uuid::new_v4(),
                kind: SourceKind::Jira,
                label: "broken".into(),
                config: SourceConfig::Jira {
                    workspace_url: "not a url".into(),
                    email: "me@acme.com".into(),
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert broken jira");

        let good_id = Uuid::new_v4();
        SourceRepo::new(pool.clone())
            .insert(&Source {
                id: good_id,
                kind: SourceKind::Jira,
                label: "Jira — acme".into(),
                config: SourceConfig::Jira {
                    workspace_url: "https://acme.atlassian.net".into(),
                    email: "me@acme.com".into(),
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert good jira");

        let cfg = resolve_registry_config(&pool).await.expect("resolve");
        assert_eq!(cfg.jira_sources.len(), 1, "broken row must be skipped");
        assert_eq!(cfg.jira_sources[0].source_id, good_id);
    }

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

    // --- DAY-81: orphan-secret audit -------------------------------------
    //
    // The audit is a warn-only safety net for installs whose DB row
    // outlives its keychain slot — it must never mutate either side.
    // The test exercises both halves: a ref that *does* resolve
    // produces zero orphans and no warning; a ref that *doesn't*
    // resolve produces exactly one orphan and leaves the `sources`
    // row (and the keychain) untouched.

    fn gitlab_secret_ref_for(source_id: Uuid) -> dayseam_core::SecretRef {
        dayseam_core::SecretRef {
            keychain_service: "dayseam.gitlab".into(),
            keychain_account: format!("source:{source_id}"),
        }
    }

    async fn insert_gitlab_source_with_secret(
        pool: &SqlitePool,
        id: Uuid,
        secret_ref: Option<dayseam_core::SecretRef>,
    ) {
        SourceRepo::new(pool.clone())
            .insert(&Source {
                id,
                kind: SourceKind::GitLab,
                label: "gitlab.example.com".into(),
                config: SourceConfig::GitLab {
                    base_url: "https://gitlab.example.com".into(),
                    user_id: 7,
                    username: "vedanth".into(),
                },
                secret_ref,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert gitlab source");
    }

    #[tokio::test]
    async fn orphan_secret_detector_logs_but_does_not_delete() {
        // Two sources:
        //   * `present_id` → keychain slot exists (healthy baseline)
        //   * `orphan_id`  → keychain slot absent (the regression)
        // The audit must return `1`, leave both `sources` rows
        // intact, and never write to the keychain.
        use dayseam_secrets::{InMemoryStore, Secret};

        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");

        let present_id = Uuid::new_v4();
        let orphan_id = Uuid::new_v4();
        let present_ref = gitlab_secret_ref_for(present_id);
        let orphan_ref = gitlab_secret_ref_for(orphan_id);

        insert_gitlab_source_with_secret(&pool, present_id, Some(present_ref.clone())).await;
        insert_gitlab_source_with_secret(&pool, orphan_id, Some(orphan_ref.clone())).await;

        let store = InMemoryStore::new();
        store
            .put(
                &crate::ipc::commands::secret_store_key(&present_ref),
                Secret::new("gl-pat-present".to_string()),
            )
            .expect("seed present slot");
        // Deliberately do *not* seed `orphan_ref` — that's the
        // whole point of the test.

        let orphans = audit_orphan_secrets(&pool, &store).await;
        assert_eq!(orphans, 1, "exactly one ref should fail to resolve");

        // Neither `sources` row was deleted — the audit is warn-only.
        let remaining = SourceRepo::new(pool.clone())
            .list()
            .await
            .expect("list after audit");
        let ids: Vec<Uuid> = remaining.iter().map(|s| s.id).collect();
        assert!(
            ids.contains(&present_id) && ids.contains(&orphan_id),
            "warn-only audit must leave both rows intact; got {ids:?}"
        );

        // The keychain is still missing the orphan ref (no auto-fix).
        let key = crate::ipc::commands::secret_store_key(&orphan_ref);
        assert!(
            store.get(&key).expect("probe").is_none(),
            "audit must not synthesise a keychain slot"
        );
    }

    #[tokio::test]
    async fn orphan_secret_detector_is_quiet_when_every_ref_resolves() {
        // Regression clamp: a freshly installed, consistent DB must
        // report zero orphans. A bug that counted "no secret_ref at
        // all" as an orphan would fire here.
        use dayseam_secrets::{InMemoryStore, Secret};

        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");

        let with_secret = Uuid::new_v4();
        let without_secret = Uuid::new_v4();
        let sr = gitlab_secret_ref_for(with_secret);
        insert_gitlab_source_with_secret(&pool, with_secret, Some(sr.clone())).await;
        insert_gitlab_source_with_secret(&pool, without_secret, None).await;

        let store = InMemoryStore::new();
        store
            .put(
                &crate::ipc::commands::secret_store_key(&sr),
                Secret::new("gl-pat".to_string()),
            )
            .expect("seed");

        let orphans = audit_orphan_secrets(&pool, &store).await;
        assert_eq!(
            orphans, 0,
            "healthy install → zero warnings; rows without secret_ref are ignored"
        );
    }

    // --- CONS-v0.2-01: Atlassian self-identity startup backfill ---------

    async fn insert_atlassian_source(pool: &SqlitePool, id: Uuid, kind: SourceKind) {
        let config = match kind {
            SourceKind::Jira => SourceConfig::Jira {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "me@acme.com".into(),
            },
            SourceKind::Confluence => SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "me@acme.com".into(),
            },
            other => panic!("insert_atlassian_source: non-atlassian kind {other:?}"),
        };
        SourceRepo::new(pool.clone())
            .insert(&Source {
                id,
                kind,
                label: "Acme".into(),
                config,
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert atlassian source");
    }

    async fn insert_atlassian_identity(
        pool: &SqlitePool,
        person_id: Uuid,
        source_id: Uuid,
        account_id: &str,
    ) {
        SourceIdentityRepo::new(pool.clone())
            .ensure(&SourceIdentity {
                id: Uuid::new_v4(),
                person_id,
                source_id: Some(source_id),
                kind: SourceIdentityKind::AtlassianAccountId,
                external_actor_id: account_id.to_string(),
            })
            .await
            .expect("seed atlassian identity");
    }

    #[tokio::test]
    async fn atlassian_backfill_reseeds_existing_identity_idempotently() {
        // Happy path: atlassian_sources_add already seeded the row.
        // The backfill's job is a cheap re-assert, not a repair.
        // This guards against a regression that makes the ensure()
        // call throw on the second boot.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let source_id = Uuid::new_v4();
        insert_atlassian_source(&pool, source_id, SourceKind::Jira).await;
        let person = PersonRepo::new(pool.clone())
            .bootstrap_self("Me")
            .await
            .expect("self");
        insert_atlassian_identity(&pool, person.id, source_id, "557058:abc").await;

        backfill_atlassian_self_identities(&pool).await;
        backfill_atlassian_self_identities(&pool).await;

        let rows = SourceIdentityRepo::new(pool.clone())
            .list_for_source(person.id, &source_id)
            .await
            .expect("list");
        assert_eq!(
            rows.iter()
                .filter(|r| r.kind == SourceIdentityKind::AtlassianAccountId
                    && r.external_actor_id == "557058:abc")
                .count(),
            1,
            "repeat backfill must not duplicate the atlassian identity",
        );
    }

    #[tokio::test]
    async fn atlassian_backfill_warns_and_continues_when_identity_missing() {
        // Manual-DB-surgery case: a Confluence source row exists but
        // its AtlassianAccountId row was deleted. Backfill can't
        // recover the opaque account_id without an API call, so it
        // must log and move on — never throw, never bootstrap a
        // bogus identity, never skip later sources.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let orphan_id = Uuid::new_v4();
        let healthy_id = Uuid::new_v4();
        insert_atlassian_source(&pool, orphan_id, SourceKind::Confluence).await;
        insert_atlassian_source(&pool, healthy_id, SourceKind::Jira).await;
        let person = PersonRepo::new(pool.clone())
            .bootstrap_self("Me")
            .await
            .expect("self");
        // Only seed the second source; first is deliberately missing.
        insert_atlassian_identity(&pool, person.id, healthy_id, "557058:healthy").await;

        // Must not panic.
        backfill_atlassian_self_identities(&pool).await;

        let orphan_rows = SourceIdentityRepo::new(pool.clone())
            .list_for_source(person.id, &orphan_id)
            .await
            .expect("orphan list");
        assert!(
            orphan_rows
                .iter()
                .all(|r| r.kind != SourceIdentityKind::AtlassianAccountId),
            "backfill must not invent an account_id for the orphan source",
        );

        let healthy_rows = SourceIdentityRepo::new(pool.clone())
            .list_for_source(person.id, &healthy_id)
            .await
            .expect("healthy list");
        assert_eq!(
            healthy_rows
                .iter()
                .filter(|r| r.kind == SourceIdentityKind::AtlassianAccountId
                    && r.external_actor_id == "557058:healthy")
                .count(),
            1,
            "orphan warning must not short-circuit the healthy source's re-seed",
        );
    }

    #[tokio::test]
    async fn atlassian_backfill_skips_when_no_atlassian_sources_present() {
        // LocalGit-only install: the atlassian backfill must not
        // bootstrap a self-person or otherwise touch the DB.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");

        backfill_atlassian_self_identities(&pool).await;

        let existing = PersonRepo::new(pool).get_self().await.expect("get_self");
        assert!(
            existing.is_none(),
            "no Atlassian sources ⇒ no bootstrap, got person row: {existing:?}",
        );
    }

    // --- DAY-84 Confluence email upgrade backfill -----------------------
    //
    // v0.2.0 persisted `SourceConfig::Confluence { workspace_url }` with
    // no `email` field. v0.2.1 added an `email` field (with
    // `#[serde(default)]` so old rows still deserialise) and made
    // `build_source_auth` reject empty emails with
    // `atlassian.auth.invalid_credentials`. Without this backfill, every
    // user who connected Confluence on v0.2.0 via the shared-PAT
    // journey is locked out of Confluence reports after upgrading until
    // they manually Reconnect — even though the token is fine.
    // These tests pin the three cases:
    //
    // 1. Shared-secret sibling has a non-empty email → copy it across.
    // 2. Confluence-only install (no sibling) → leave row alone, log.
    // 3. Confluence row already has an email → no-op.

    async fn insert_atlassian_source_with(
        pool: &SqlitePool,
        id: Uuid,
        config: SourceConfig,
        secret_ref: Option<dayseam_core::SecretRef>,
    ) {
        let kind = config.kind();
        SourceRepo::new(pool.clone())
            .insert(&Source {
                id,
                kind,
                label: "Acme".into(),
                config,
                secret_ref,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert atlassian source with config");
    }

    fn shared_secret_ref() -> dayseam_core::SecretRef {
        dayseam_core::SecretRef {
            keychain_service: "dayseam.atlassian".into(),
            keychain_account: "slot:shared".into(),
        }
    }

    #[tokio::test]
    async fn confluence_email_backfill_copies_from_jira_sibling_sharing_secret_ref() {
        // The v0.2.0-upgrade scenario the user hit in dogfood: Jira
        // row has email + shared secret_ref, Confluence row has empty
        // email + same secret_ref. Backfill must copy the email
        // across so `build_source_auth` stops rejecting.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let sr = shared_secret_ref();
        let jira_id = Uuid::new_v4();
        let conf_id = Uuid::new_v4();
        insert_atlassian_source_with(
            &pool,
            jira_id,
            SourceConfig::Jira {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "v@acme.com".into(),
            },
            Some(sr.clone()),
        )
        .await;
        insert_atlassian_source_with(
            &pool,
            conf_id,
            SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: String::new(),
            },
            Some(sr.clone()),
        )
        .await;

        run_registered_repairs(&pool).await;

        let after = SourceRepo::new(pool.clone())
            .get(&conf_id)
            .await
            .expect("get confluence")
            .expect("confluence row");
        match after.config {
            SourceConfig::Confluence { email, .. } => {
                assert_eq!(
                    email, "v@acme.com",
                    "backfill must copy the Jira sibling's email into the Confluence row"
                );
            }
            other => panic!("expected Confluence config, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn confluence_email_backfill_leaves_row_alone_when_no_sibling() {
        // Confluence-only install — no Jira sibling to copy from.
        // Backfill must log + skip, leaving the row with empty email
        // so `build_source_auth` routes the user to Reconnect.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let conf_id = Uuid::new_v4();
        insert_atlassian_source_with(
            &pool,
            conf_id,
            SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: String::new(),
            },
            Some(shared_secret_ref()),
        )
        .await;

        run_registered_repairs(&pool).await;

        let after = SourceRepo::new(pool.clone())
            .get(&conf_id)
            .await
            .expect("get")
            .expect("row");
        match after.config {
            SourceConfig::Confluence { email, .. } => {
                assert!(
                    email.is_empty(),
                    "no sibling → email stays empty so Reconnect is forced, got {email:?}"
                );
            }
            other => panic!("expected Confluence config, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn confluence_email_backfill_is_noop_when_email_already_present() {
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let conf_id = Uuid::new_v4();
        insert_atlassian_source_with(
            &pool,
            conf_id,
            SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "already@acme.com".into(),
            },
            Some(shared_secret_ref()),
        )
        .await;

        run_registered_repairs(&pool).await;

        let after = SourceRepo::new(pool.clone())
            .get(&conf_id)
            .await
            .expect("get")
            .expect("row");
        match after.config {
            SourceConfig::Confluence { email, .. } => {
                assert_eq!(email, "already@acme.com");
            }
            other => panic!("expected Confluence config, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn confluence_email_backfill_migrates_raw_v0_2_0_shape_json() {
        // Regression for the exact failure the user hit in dogfood:
        // a Confluence row written by v0.2.0 has NO `email` field in
        // its `config_json` column. On upgrade, v0.2.1's
        // `#[serde(default)]` must deserialise it as `email: ""`, and
        // this boot-time backfill must copy the sibling Jira email
        // across before `build_source_auth` sees the row and rejects
        // with `atlassian.auth.invalid_credentials`.
        //
        // Unlike the other tests in this block, we write raw JSON via
        // sqlx instead of going through `SourceRepo::insert` — that
        // way the `#[serde(default)]` attribute is actually exercised
        // on the read path. If a future refactor drops
        // `#[serde(default)]` from `SourceConfig::Confluence::email`
        // (or changes the wire shape in any way that breaks old
        // rows), this test fails loudly. The three
        // struct-literal-based tests below do not catch that
        // regression because they always serialise the new shape
        // (`{"email":""}` vs v0.2.0's missing key).
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");

        let sr = shared_secret_ref();

        // Jira sibling: same secret_ref, non-empty email. This path
        // is the v0.2.0 shape already — Jira carried `email` from
        // day one.
        let jira_id = Uuid::new_v4();
        insert_atlassian_source_with(
            &pool,
            jira_id,
            SourceConfig::Jira {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "v@acme.com".into(),
            },
            Some(sr.clone()),
        )
        .await;

        // Confluence row in *literal v0.2.0 shape*: no `email` key.
        // We go behind `SourceRepo::insert`'s back because the Rust
        // struct literal `SourceConfig::Confluence { email: "".into() }`
        // would serialise as `{"email":""}`, which is a strict
        // superset of v0.2.0 and does not exercise the
        // `#[serde(default)]` leg.
        let conf_id = Uuid::new_v4();
        let v020_config_json = r#"{"Confluence":{"workspace_url":"https://acme.atlassian.net/"}}"#;
        let sr_json = serde_json::to_string(&sr).expect("ser secret_ref");
        let health_json = serde_json::to_string(&SourceHealth::unchecked()).expect("ser health");
        sqlx::query(
            "INSERT INTO sources \
             (id, kind, label, config_json, secret_ref, created_at, \
              last_sync_at, last_health_json) \
             VALUES (?, ?, ?, ?, ?, ?, NULL, ?)",
        )
        .bind(conf_id.to_string())
        .bind("Confluence")
        .bind("Acme — Confluence")
        .bind(v020_config_json)
        .bind(sr_json)
        .bind(Utc::now().to_rfc3339())
        .bind(health_json)
        .execute(&pool)
        .await
        .expect("insert raw v0.2.0-shape Confluence row");

        // Pin the `#[serde(default)]` contract: the v0.2.0-shape JSON
        // (with no `email` key) must still round-trip through
        // `SourceRepo::get`. If this assertion ever fails, every
        // v0.2.0 Confluence row becomes unreadable on upgrade — the
        // backfill below would never run because `list` would return
        // a `DbError` before it gets here.
        let before = SourceRepo::new(pool.clone())
            .get(&conf_id)
            .await
            .expect("get")
            .expect("raw v0.2.0-shape row must deserialise via #[serde(default)]");
        match &before.config {
            SourceConfig::Confluence { email, .. } => {
                assert!(
                    email.is_empty(),
                    "v0.2.0 JSON has no `email` key; `#[serde(default)]` \
                     must materialise it as an empty string (got {email:?})",
                );
            }
            other => panic!("expected Confluence, got {other:?}"),
        }

        // The actual upgrade migration.
        run_registered_repairs(&pool).await;

        let after = SourceRepo::new(pool.clone())
            .get(&conf_id)
            .await
            .expect("get")
            .expect("row still present");
        match after.config {
            SourceConfig::Confluence {
                email,
                workspace_url,
            } => {
                assert_eq!(
                    email, "v@acme.com",
                    "raw v0.2.0-shape row must inherit sibling Jira's email \
                     so `build_source_auth` stops rejecting it",
                );
                assert_eq!(
                    workspace_url, "https://acme.atlassian.net/",
                    "workspace_url must survive the backfill untouched",
                );
            }
            other => panic!("expected Confluence, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn confluence_email_backfill_skips_sibling_with_different_secret_ref() {
        // Defensive: two independently-connected Atlassian tenants
        // share a host but not a credential. Backfill must not copy
        // the email across — that would leak identity.
        let dir = TempDir::new().expect("temp dir");
        let pool = open(&dir.path().join("state.db")).await.expect("open");
        let jira_id = Uuid::new_v4();
        let conf_id = Uuid::new_v4();
        insert_atlassian_source_with(
            &pool,
            jira_id,
            SourceConfig::Jira {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "jira-only@acme.com".into(),
            },
            Some(dayseam_core::SecretRef {
                keychain_service: "dayseam.atlassian".into(),
                keychain_account: "slot:jira".into(),
            }),
        )
        .await;
        insert_atlassian_source_with(
            &pool,
            conf_id,
            SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: String::new(),
            },
            Some(dayseam_core::SecretRef {
                keychain_service: "dayseam.atlassian".into(),
                keychain_account: "slot:conf".into(),
            }),
        )
        .await;

        run_registered_repairs(&pool).await;

        let after = SourceRepo::new(pool.clone())
            .get(&conf_id)
            .await
            .expect("get")
            .expect("row");
        match after.config {
            SourceConfig::Confluence { email, .. } => {
                assert!(
                    email.is_empty(),
                    "different secret_refs → must not copy, got {email:?}"
                );
            }
            other => panic!("expected Confluence config, got {other:?}"),
        }
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
