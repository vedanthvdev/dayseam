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
use connectors_sdk::{
    AuthStrategy, BasicAuth, ConnCtx, NoneAuth, NoopRawStore, PatAuth, SystemClock,
};
use dayseam_core::{
    error_codes, ActivityEvent, DayseamError, GitlabValidationResult, LocalRepo, LogEntry,
    LogLevel, Person, ProgressEvent, ReportCompletedEvent, ReportDraft, RunId, SecretRef, Settings,
    SettingsPatch, Sink, SinkConfig, SinkKind, Source, SourceConfig, SourceHealth, SourceId,
    SourceIdentity, SourceIdentityKind, SourceKind, SourcePatch, ToastEvent, ToastSeverity,
    WriteReceipt,
};
use dayseam_db::{
    ActivityRepo, DraftRepo, LocalRepoRepo, LogRepo, LogRow, PersonRepo, SettingsRepo, SinkRepo,
    SourceIdentityRepo, SourceRepo,
};
use dayseam_events::RunStreams;
use dayseam_orchestrator::{resolve_cutoff, retention_sweep, GenerateRequest, SourceHandle};
use dayseam_report::{DEV_EOD_TEMPLATE_ID, DEV_EOD_TEMPLATE_VERSION};
use dayseam_secrets::Secret;
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
    "atlassian_validate_credentials",
    "atlassian_sources_add",
    "atlassian_sources_reconnect",
    "github_validate_credentials",
    "github_sources_add",
    "github_sources_reconnect",
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

/// Crate-visible wrapper around [`invalid_config`] so the new
/// `ipc::atlassian` module (DAY-82) can mint the same
/// `DayseamError::InvalidConfig` shape this module has been minting
/// for every other command since DAY-6. Keeping the helper private
/// by default — and exposing it through a `pub(crate)` name — is
/// deliberate: we want one idiomatic way to raise structural
/// errors, but only other IPC modules get to reach for it.
pub(crate) fn invalid_config_public(code: &str, message: impl Into<String>) -> DayseamError {
    invalid_config(code, message)
}

/// Crate-visible alias for [`publish_restart_required_toast`], used
/// by the `ipc::atlassian` module (DAY-82) so both source-adding
/// surfaces fire the same "restart required" toast on success.
pub(crate) fn persist_restart_required_toast(state: &AppState) {
    publish_restart_required_toast(state);
}

/// Keychain `service` half for every GitLab PAT this app stores.
///
/// Picking a constant here — rather than threading the bundle id through
/// each call site — keeps Keychain Access readable ("all `dayseam.gitlab`
/// entries") and makes the `sources_delete` sweep trivial to audit.
const GITLAB_KEYCHAIN_SERVICE: &str = "dayseam.gitlab";

/// Compute the stable [`SecretRef`] for a GitLab source's PAT. Each
/// configured GitLab source owns one keychain row keyed by its
/// [`SourceId`]. The `account` half embeds the UUID so two sources
/// targeting the same host cannot clobber each other's tokens.
fn gitlab_secret_ref(source_id: SourceId) -> SecretRef {
    SecretRef {
        keychain_service: GITLAB_KEYCHAIN_SERVICE.to_string(),
        keychain_account: format!("source:{source_id}"),
    }
}

/// Render a [`SecretRef`] as the single-string key the
/// [`dayseam_secrets::SecretStore`] trait expects (`service::account`).
///
/// Pub-crate-visible so DAY-81's boot-time orphan-secret audit
/// (`crate::startup::audit_orphan_secrets`) can probe the exact same
/// key shape the live IPC layer writes.
pub(crate) fn secret_store_key(sr: &SecretRef) -> String {
    format!("{}::{}", sr.keychain_service, sr.keychain_account)
}

/// Build an [`AuthStrategy`] for `source`, reading the PAT out of the
/// OS keychain on demand.
///
/// * `LocalGit` → [`NoneAuth`]. Git-on-disk walks never hit the network.
/// * `GitLab` with a populated `secret_ref` and a PAT in the keychain
///   → [`PatAuth::gitlab`]. The descriptor on the auth strategy
///   matches the `secret_ref`, so [`AuthDescriptor`] traces the
///   keychain row the token came from.
/// * `GitLab` with a missing or empty keychain slot → `Err(Auth)`
///   with `gitlab.auth.invalid_token`. The UI renders the Reconnect
///   card, which reopens `AddGitlabSourceDialog` in edit-mode for the
///   affected source.
/// * `Jira` / `Confluence` → [`BasicAuth::atlassian`] built from the
///   per-source `email` on the `SourceConfig::{Jira, Confluence}`
///   row and the API token read from the keychain slot named by
///   `secret_ref`. Two sources sharing a single `secret_ref` reuse
///   the same keychain entry; separate credentials live in separate
///   slots. Missing/empty slot or missing `email` → `Err(Auth)` with
///   `atlassian.auth.invalid_credentials`, which the UI renders as a
///   Reconnect card.
///
/// Introduced in DAY-70. Before this helper, the IPC layer handed
/// every `ConnCtx` a bare [`NoneAuth`]. For self-hosted GitLab that
/// meant every `GET /api/v4/users/:id/events` went out without a
/// `PRIVATE-TOKEN` header and came back `HTTP 200 []`, so
/// `report_generate` silently produced empty reports with no visible
/// error — the bug the original user report traced. DAY-84 extended
/// the helper to Atlassian (DOG-v0.2-01): v0.2.0 shipped with this
/// arm still stubbed to `Err(Unsupported)`, so every Atlassian
/// source the dialog added came back "connector unsupported" at
/// report time.
fn build_source_auth(
    state: &AppState,
    source: &Source,
) -> Result<Arc<dyn AuthStrategy>, DayseamError> {
    match source.kind {
        SourceKind::LocalGit => Ok(Arc::new(NoneAuth)),
        SourceKind::Jira | SourceKind::Confluence => {
            let email = match &source.config {
                SourceConfig::Jira { email, .. } => email.clone(),
                SourceConfig::Confluence { email, .. } => email.clone(),
                other => {
                    return Err(DayseamError::Internal {
                        code: "ipc.sources.kind_config_mismatch".to_string(),
                        message: format!(
                            "source {} has kind {:?} but config {:?}",
                            source.id,
                            source.kind,
                            other.kind()
                        ),
                    });
                }
            };
            if email.trim().is_empty() {
                return Err(DayseamError::Auth {
                    code: error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS.to_string(),
                    message: format!(
                        "no email on file for Atlassian source {} — reconnect to re-enter it",
                        source.id
                    ),
                    retryable: false,
                    action_hint: Some("reconnect".to_string()),
                });
            }
            let secret_ref = source
                .secret_ref
                .clone()
                .ok_or_else(|| DayseamError::Auth {
                    code: error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS.to_string(),
                    message: format!(
                        "no API token on file for Atlassian source {} — reconnect to add one",
                        source.id
                    ),
                    retryable: false,
                    action_hint: Some("reconnect".to_string()),
                })?;
            let key = secret_store_key(&secret_ref);
            let token_secret = state
                .secrets
                .get(&key)
                .map_err(|e| DayseamError::Internal {
                    code: "ipc.atlassian.keychain_read_failed".to_string(),
                    message: format!("keychain read for {key} failed: {e}"),
                })?
                .ok_or_else(|| DayseamError::Auth {
                    code: error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS.to_string(),
                    message: format!(
                        "keychain slot {key} is empty for source {} — reconnect to restore the API token",
                        source.id
                    ),
                    retryable: false,
                    action_hint: Some("reconnect".to_string()),
                })?;
            Ok(Arc::new(BasicAuth::atlassian(
                email,
                token_secret.expose_secret().as_str(),
                secret_ref.keychain_service.clone(),
                secret_ref.keychain_account.clone(),
            )))
        }
        SourceKind::GitLab => {
            let secret_ref = source
                .secret_ref
                .clone()
                .ok_or_else(|| DayseamError::Auth {
                    code: error_codes::GITLAB_AUTH_INVALID_TOKEN.to_string(),
                    message: format!(
                        "no PAT on file for GitLab source {} — reconnect to add one",
                        source.id
                    ),
                    retryable: false,
                    action_hint: Some("reconnect".to_string()),
                })?;
            let key = secret_store_key(&secret_ref);
            let pat_secret = state
                .secrets
                .get(&key)
                .map_err(|e| DayseamError::Internal {
                    code: error_codes::IPC_GITLAB_KEYCHAIN_READ_FAILED.to_string(),
                    message: format!("keychain read for {key} failed: {e}"),
                })?
                .ok_or_else(|| DayseamError::Auth {
                    code: error_codes::GITLAB_AUTH_INVALID_TOKEN.to_string(),
                    message: format!(
                        "keychain slot {key} is empty for source {} — reconnect to restore the PAT",
                        source.id
                    ),
                    retryable: false,
                    action_hint: Some("reconnect".to_string()),
                })?;
            let token = pat_secret.expose_secret();
            Ok(Arc::new(PatAuth::gitlab(
                token.as_str(),
                secret_ref.keychain_service.clone(),
                secret_ref.keychain_account.clone(),
            )))
        }
        // DAY-95 wires the GitHub PAT through the same keychain +
        // `PatAuth::github` path GitLab uses. DAY-99 extends the
        // Add-Source dialog so the user can actually *produce* a
        // `SourceKind::GitHub` row; until then this arm only runs on
        // a hand-crafted DB row or a direct `sources_add` IPC call
        // for the GitHub kind. We still wire it now so the scaffold
        // works end-to-end — a GitHub source reaches
        // `GithubConnector::healthcheck` / `::sync` with the right
        // auth attached, matching every other kind's contract.
        SourceKind::GitHub => {
            let secret_ref = source
                .secret_ref
                .clone()
                .ok_or_else(|| DayseamError::Auth {
                    code: error_codes::GITHUB_AUTH_INVALID_CREDENTIALS.to_string(),
                    message: format!(
                        "no PAT on file for GitHub source {} — reconnect to add one",
                        source.id
                    ),
                    retryable: false,
                    action_hint: Some("reconnect".to_string()),
                })?;
            let key = secret_store_key(&secret_ref);
            let pat_secret = state
                .secrets
                .get(&key)
                .map_err(|e| DayseamError::Internal {
                    code: "ipc.github.keychain_read_failed".to_string(),
                    message: format!("keychain read for {key} failed: {e}"),
                })?
                .ok_or_else(|| DayseamError::Auth {
                    code: error_codes::GITHUB_AUTH_INVALID_CREDENTIALS.to_string(),
                    message: format!(
                        "keychain slot {key} is empty for source {} — reconnect to restore the PAT",
                        source.id
                    ),
                    retryable: false,
                    action_hint: Some("reconnect".to_string()),
                })?;
            let token = pat_secret.expose_secret();
            Ok(Arc::new(PatAuth::github(
                token.as_str(),
                secret_ref.keychain_service.clone(),
                secret_ref.keychain_account.clone(),
            )))
        }
    }
}

/// Validate the `pat` argument handed to `sources_update` against
/// the `existing` source row. There are four relevant combinations:
///
/// | kind       | existing.secret_ref | pat arg          | verdict      |
/// |------------|---------------------|------------------|--------------|
/// | LocalGit   | —                   | None             | OK (no-op)   |
/// | LocalGit   | —                   | Some(_)          | KindMismatch |
/// | GitLab     | Some(_)             | None             | OK (no-op)   |
/// | GitLab     | Some(_)             | Some(empty)      | PatMissing   |
/// | GitLab     | Some(_)             | Some(non-empty)  | OK (rotate)  |
/// | GitLab     | None                | None             | PatMissing*  |
/// | GitLab     | None                | Some(empty)      | PatMissing   |
/// | GitLab     | None                | Some(non-empty)  | OK (seed)    |
///
/// The starred row is the defense-in-depth case: a GitLab row whose
/// `secret_ref` is already null cannot *also* get a null `pat` from
/// `sources_update`, or the whole call becomes a silent no-op and
/// the user loops between "reconnect" and "report comes back with a
/// `gitlab.auth.invalid_token` toast". The frontend is supposed to
/// always pass the PAT in the reconnect flow (`useSources.update`
/// threads it, `AddGitlabSourceDialog.handleSubmit` fills it from
/// the `pat` state), but a stale bundle still calling the
/// pre-DAY-70 `sources_update({id, patch})` shape would sail past
/// this branch silently. Failing loud here turns a baffling cross-
/// command bug into a local Save-button error.
fn validate_pat_arg(existing: &Source, pat: Option<&IpcSecretString>) -> Result<(), DayseamError> {
    match (existing.kind, pat, existing.secret_ref.as_ref()) {
        (SourceKind::LocalGit, Some(_), _) => Err(invalid_config(
            error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH,
            format!(
                "pat arg is only valid for GitLab sources; {} is {:?}",
                existing.id, existing.kind
            ),
        )),
        (SourceKind::GitLab, Some(p), _) if p.expose().trim().is_empty() => {
            Err(invalid_config(
                error_codes::IPC_GITLAB_PAT_MISSING,
                "sources_update pat must be non-empty when provided",
            ))
        }
        (SourceKind::GitLab, None, None) => Err(invalid_config(
            error_codes::IPC_GITLAB_PAT_MISSING,
            format!(
                "GitLab source {} has no PAT on file and sources_update was called without one — the reconnect dialog must provide `pat` in the IPC payload",
                existing.id
            ),
        )),
        _ => Ok(()),
    }
}

/// Persist `pat` to the OS keychain for `source_id`, returning the
/// canonical [`SecretRef`] the caller should stamp onto the `sources`
/// row. Failures are surfaced as `Internal` (rather than `Auth`)
/// because "couldn't write to the keychain" is a local-environment
/// problem, not a PAT-rotation prompt; the user retrying the dialog
/// will re-exercise the exact same failing path, which is the signal
/// we want them to see.
fn persist_gitlab_pat(
    state: &AppState,
    source_id: SourceId,
    pat: &IpcSecretString,
) -> Result<SecretRef, DayseamError> {
    let secret_ref = gitlab_secret_ref(source_id);
    let key = secret_store_key(&secret_ref);
    state
        .secrets
        .put(&key, Secret::new(pat.expose().to_string()))
        .map_err(|e| DayseamError::Internal {
            code: error_codes::IPC_GITLAB_KEYCHAIN_WRITE_FAILED.to_string(),
            message: format!("keychain write for {key} failed: {e}"),
        })?;
    Ok(secret_ref)
}

/// Best-effort: remove the keychain entry pointed at by `secret_ref`.
/// Swallows failures with a `warn!` because a half-deleted source is
/// worse than a lingering keychain row (the orchestrator registry will
/// reject the row on next boot anyway, and the user can clean
/// leftovers with Keychain Access).
fn best_effort_delete_secret(state: &AppState, secret_ref: &SecretRef) {
    let key = secret_store_key(secret_ref);
    if let Err(e) = state.secrets.delete(&key) {
        tracing::warn!(error = %e, %key, "keychain delete for source failed; row may linger");
    }
}

/// Guarantee the [`SourceIdentityKind::GitLabUserId`] row that maps
/// this GitLab source's numeric `user_id` to the current self
/// [`Person`] exists, so the render-stage self-filter
/// (`dayseam-report::filter_events_by_self`) recognises the source's
/// events as authored by the user.
///
/// This is the production-side half of the DAY-71 fix. The bug looked
/// like this: `sync_runs` showed `fetched_count: N`, `activity_events`
/// held all N rows, and yet the draft came back with "No tracked
/// activity". The upstream GitLab `/events` payload populates
/// `actor.external_id` with the numeric user id (and leaves
/// `actor.email` `None`), but onboarding never seeded a matching
/// `GitLabUserId` identity — so every event was silently dropped at
/// the render stage as "unknown actor".
///
/// Idempotent by design: the unique index on
/// `(person_id, source_id, kind, external_actor_id)` collapses
/// repeated calls into a no-op, which is exactly what the startup
/// backfill relies on.
async fn ensure_gitlab_self_identity(
    state: &AppState,
    source_id: SourceId,
    user_id: i64,
) -> Result<(), DayseamError> {
    let person = PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| internal("persons.bootstrap_self", e))?;

    let identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: person.id,
        source_id: Some(source_id),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: user_id.to_string(),
    };
    SourceIdentityRepo::new(state.pool.clone())
        .ensure(&identity)
        .await
        .map_err(|e| internal("source_identities.ensure", e))?;
    Ok(())
}

/// CONS-v0.2-01. Atlassian counterpart to
/// [`ensure_gitlab_self_identity`]: re-seed the
/// `AtlassianAccountId` self-identity row for a Jira or Confluence
/// source, using the `account_id` already on file in
/// `source_identities` (the one `atlassian_sources_add` stamped at
/// add-time).
///
/// Why this exists even though `atlassian_sources_add` already
/// seeds the row:
///
///   1. **`sources_update` parity.** GitLab's path calls
///      [`ensure_gitlab_self_identity`] on every update so pre-DAY-71
///      installs self-heal on the first reconnect. The Atlassian
///      path had no analogous call, meaning a user whose
///      `source_identities` row got manually removed (by a DB repair,
///      a migration accident, or a future "delete linked identities"
///      UI we haven't built yet) would silently render empty reports
///      forever. This re-asserts the invariant on every update.
///   2. **Startup backfill parity.** `backfill_atlassian_self_identities`
///      calls this helper too; boot is the only other path that can
///      repair a stale install without asking the user to re-add.
///
/// Unlike the GitLab path, the Atlassian `account_id` lives on the
/// `source_identities` row itself (we don't persist it on
/// `SourceConfig` — the Atlassian Cloud opaque id is only ever
/// surfaced by a live `/rest/api/3/myself` probe). So this helper:
///
///   * finds the existing `AtlassianAccountId` row for this source,
///   * re-calls `SourceIdentityRepo::ensure` with its persisted
///     `external_actor_id` (idempotent by construction), and
///   * returns `Ok(false)` as an outward-facing "no row yet —
///     caller's choice whether to treat that as an error".
///
/// Callers that can't repair a missing row (startup backfill, pure
/// label-edit updates) turn `Ok(false)` into a `tracing::warn!` and
/// keep going; callers that are morally required to have one
/// (`sources_update` after `atlassian_sources_add` already ran)
/// treat it as silent-success because the only way to reach that
/// state is deliberate DB surgery.
async fn ensure_atlassian_self_identity(
    state: &AppState,
    source_id: SourceId,
) -> Result<bool, DayseamError> {
    let person = PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| internal("persons.bootstrap_self", e))?;

    let identity_repo = SourceIdentityRepo::new(state.pool.clone());
    let rows = identity_repo
        .list_for_source(person.id, &source_id)
        .await
        .map_err(|e| internal("source_identities.list_for_source", e))?;

    let Some(existing) = rows
        .into_iter()
        .find(|r| r.kind == SourceIdentityKind::AtlassianAccountId)
    else {
        return Ok(false);
    };

    identity_repo
        .ensure(&existing)
        .await
        .map_err(|e| internal("source_identities.ensure", e))?;
    Ok(true)
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
/// Persist a new [`Source`] and, for GitLab, its PAT.
///
/// `pat` is required (and non-empty) when `kind == SourceKind::GitLab`
/// and ignored otherwise. The command runs in this order:
///
///   1. Insert the source row (with `secret_ref = None`).
///   2. For GitLab: write the PAT to the Keychain under
///      `dayseam.gitlab::source:<uuid>`, then update the row's
///      `secret_ref` to point at that slot.
///   3. For LocalGit: discover repos underneath `scan_roots` and
///      upsert them into `local_repos`.
///
/// If step 2 fails (e.g. Keychain is locked or the user denied access)
/// the partially-inserted row is deleted before the error propagates,
/// so the user never ends up with a ghost GitLab source that can't
/// authenticate. Introduced alongside [`build_source_auth`] in DAY-70
/// to fix the silent empty-report bug.
#[tauri::command]
pub async fn sources_add(
    kind: SourceKind,
    label: String,
    config: SourceConfig,
    pat: Option<IpcSecretString>,
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
    // F-8 (DAY-106). Reject a LocalGit add whose scan roots overlap
    // another LocalGit source's scan roots *before* we touch sqlite
    // or Keychain, so a rejected overlap leaves zero traces behind
    // and the user sees the same dialog they'll retry from. See
    // `ensure_local_git_scan_roots_are_disjoint` for the rationale.
    if let SourceConfig::LocalGit { scan_roots } = &config {
        ensure_local_git_scan_roots_are_disjoint(&state, None, scan_roots).await?;
    }
    // Fast path: fail before we touch sqlite if this is a GitLab add
    // without a PAT. The old code silently persisted a `secret_ref:
    // None` row here, which is exactly what made `report_generate`
    // run unauthenticated later.
    if kind == SourceKind::GitLab {
        match pat.as_ref() {
            Some(p) if !p.expose().trim().is_empty() => {}
            _ => {
                return Err(invalid_config(
                    error_codes::IPC_GITLAB_PAT_MISSING,
                    "sources_add for GitLab requires a non-empty PAT",
                ));
            }
        }
    }

    let source_id = Uuid::new_v4();
    let source = Source {
        id: source_id,
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

    if kind == SourceKind::GitLab {
        let pat = pat.as_ref().expect("guarded above");
        match persist_gitlab_pat(&state, source_id, pat) {
            Ok(secret_ref) => {
                if let Err(e) = source_repo
                    .update_secret_ref(&source_id, Some(&secret_ref))
                    .await
                {
                    // Keychain already holds the PAT; rolling back the
                    // sqlite row is now the right move so the next
                    // re-add starts from a clean slate.
                    best_effort_delete_secret(&state, &secret_ref);
                    let _ = source_repo.delete(&source_id).await;
                    return Err(internal("sources.update_secret_ref", e));
                }
            }
            Err(e) => {
                // Keychain write failed; remove the source row we
                // inserted above so the user isn't left with a row
                // that can never authenticate.
                let _ = source_repo.delete(&source_id).await;
                return Err(e);
            }
        }

        // DAY-71: seed the `GitLabUserId` self-identity so the render
        // stage recognises this source's events as authored by us.
        // Without this row the upstream `/events` payload's
        // `actor.external_id = "<user_id>"` matches no identity and
        // every event is dropped as "unknown actor" — the exact bug
        // that landed with `fetched_count: N` but "No tracked
        // activity" in the rendered draft.
        if let SourceConfig::GitLab { user_id, .. } = &config {
            if let Err(e) = ensure_gitlab_self_identity(&state, source_id, *user_id).await {
                // Seeding failed after the source + keychain are
                // already durable. Roll the whole thing back so we
                // never leave behind a source that would silently
                // render empty; next re-add will re-exercise the
                // same path and surface the same error so the user
                // can act on it.
                if let Some(sr) = source_repo
                    .get(&source_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| s.secret_ref)
                {
                    best_effort_delete_secret(&state, &sr);
                }
                let _ = source_repo.delete(&source_id).await;
                return Err(e);
            }
        }
    }

    if let SourceConfig::LocalGit { scan_roots } = &config {
        upsert_discovered_repos(&state, &source.id, scan_roots).await?;
    }

    publish_restart_required_toast(&state);
    // Re-read so the caller sees the `secret_ref` we just stamped.
    source_repo
        .get(&source_id)
        .await
        .map_err(|e| internal("sources.get", e))?
        .ok_or_else(|| {
            invalid_config(
                error_codes::IPC_SOURCE_NOT_FOUND,
                format!("source {source_id} disappeared immediately after insert"),
            )
        })
}

/// Edit an existing [`Source`]. The new `pat` arg lets the caller
/// rotate the stored GitLab PAT (the "Reconnect" flow) in the same
/// round-trip as a `config` update, so the UI doesn't have to juggle
/// two partial-success states.
///
/// Rotation semantics: when `pat` is `Some(_)` and the source is a
/// GitLab source, the existing Keychain slot (or the canonical one
/// derived from `source_id` if `secret_ref` was `None`) is
/// overwritten with the new token. Any previously stored PAT bytes
/// are zeroed by the Keychain on replace. When `pat` is `None`, the
/// stored PAT is left untouched — a pure label-rename does not need
/// to re-unlock the Keychain.
#[tauri::command]
pub async fn sources_update(
    id: SourceId,
    patch: SourcePatch,
    pat: Option<IpcSecretString>,
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
        // F-8 (DAY-106). Reject a scan-root edit that would introduce
        // overlap with another LocalGit source *before* we rewrite
        // the persisted config. `self_source_id = Some(id)` so the
        // source can shrink or keep its own scan roots without
        // reporting itself as a contender.
        if let SourceConfig::LocalGit { scan_roots } = config {
            ensure_local_git_scan_roots_are_disjoint(&state, Some(id), scan_roots).await?;
        }
        repo.update_config(&id, config)
            .await
            .map_err(|e| internal("sources.update_config", e))?;
        if let SourceConfig::LocalGit { scan_roots } = config {
            new_scan_roots = Some(scan_roots.clone());
        }
    }

    validate_pat_arg(&existing, pat.as_ref())?;
    if let Some(new_pat) = pat.as_ref() {
        let secret_ref = persist_gitlab_pat(&state, id, new_pat)?;
        repo.update_secret_ref(&id, Some(&secret_ref))
            .await
            .map_err(|e| internal("sources.update_secret_ref", e))?;
        tracing::info!(
            source_id = %id,
            keychain_service = %secret_ref.keychain_service,
            keychain_account = %secret_ref.keychain_account,
            "sources_update persisted GitLab PAT and stamped secret_ref"
        );
    }

    // DAY-71: on every GitLab-source update, make sure the
    // `GitLabUserId` self-identity exists for the *current* user_id
    // on this source. This covers two cases:
    //   1. Existing installs created before the auto-seed landed —
    //      `sources_update` is the only path a reconnecting user
    //      hits, so seeding here fixes them without requiring a
    //      delete + re-add.
    //   2. The user edited the GitLab config and changed `user_id`;
    //      we seed the new id. Stale rows from a previous `user_id`
    //      are left behind deliberately: they match no actor and
    //      therefore do no harm, and deleting them risks dropping
    //      rows that are actually still valid for older persisted
    //      events this person authored under the old numeric id.
    if existing.kind == SourceKind::GitLab {
        // Use the patched config if one was supplied; otherwise fall
        // back to the persisted config — a pure label edit or PAT
        // rotation still needs to re-seed pre-DAY-71 rows.
        let effective_config = patch.config.as_ref().unwrap_or(&existing.config);
        if let SourceConfig::GitLab { user_id, .. } = effective_config {
            ensure_gitlab_self_identity(&state, id, *user_id).await?;
        }
    }
    if matches!(existing.kind, SourceKind::Jira | SourceKind::Confluence) {
        // CONS-v0.2-01. Same parity argument as GitLab above: a
        // silent-repair invariant on every update. Unlike GitLab we
        // can't reconstruct the `account_id` from config alone, so
        // a missing row can't self-heal here — we downgrade to a
        // structured warning and let the user's next reconnect pass
        // through `atlassian_sources_add`, which does seed it.
        match ensure_atlassian_self_identity(&state, id).await? {
            true => {}
            false => {
                tracing::warn!(
                    source_id = %id,
                    source_kind = ?existing.kind,
                    "sources_update: no AtlassianAccountId identity on file — self-filtering will \
                     silently skip this source's events until the user reconnects it",
                );
            }
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

    // Only config changes on a LocalGit source require a restart: the
    // in-memory `LocalGitConnector` snapshots its `scan_roots` list
    // at boot (see `crate::startup::build_orchestrator`). A GitLab
    // config edit (base_url / user_id / username) and a PAT rotation
    // both hit fresh DB + keychain reads on the next
    // `report_generate` via `build_source_auth`, so nudging the user
    // to restart after a reconnect would be a lie — and a lie that
    // makes them think the PAT save failed.
    let needs_restart = patch.config.is_some() && existing.kind == SourceKind::LocalGit;
    if needs_restart {
        publish_restart_required_toast(&state);
    }
    Ok(updated)
}

#[tauri::command]
pub async fn sources_delete(id: SourceId, state: State<'_, AppState>) -> Result<(), DayseamError> {
    let repo = SourceRepo::new(state.pool.clone());
    // DAY-81: `SourceRepo::delete` resolves the "is this secret
    // still referenced by another source?" question inside the same
    // transaction as the `DELETE`, handing us back `Some(ref)`
    // only when this row was the *last* holder of the secret —
    // which is the only case where dropping the keychain entry is
    // safe. In the shared-PAT case (Jira + Confluence sharing one
    // API token) removing one of the two sources here returns
    // `None`, preserving the surviving source's ability to
    // authenticate.
    let orphaned_secret = repo
        .delete(&id)
        .await
        .map_err(|e| internal("sources.delete", e))?;
    if let Some(sr) = orphaned_secret {
        best_effort_delete_secret(&state, &sr);
    }
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

    // Route the stored PAT (if any) into the connector so the
    // healthcheck actually probes the configured credentials —
    // unauthenticated `GET /user` on public GitLab returns 200 for
    // anonymous, which would report spurious "healthy" for a row
    // whose PAT is missing/rotated. If the PAT is absent we surface
    // the `gitlab.auth.invalid_token` error via `SourceHealth.last_error`,
    // which is the same path a live 401 takes.
    let auth_result = build_source_auth(&state, &source);

    // The connector's healthcheck only reads `auth` / `cancel` /
    // (sometimes) `clock`; everything else is plumbed through for
    // parity with `sync` so connectors don't have to special-case
    // probes. The throwaway `RunStreams` is dropped on return.
    let streams = RunStreams::new(RunId::new());
    let auth: Arc<dyn AuthStrategy> = match auth_result {
        Ok(a) => a,
        Err(e) => {
            let health = SourceHealth {
                ok: false,
                checked_at: Some(Utc::now()),
                last_error: Some(e),
            };
            source_repo
                .update_health(&id, &health)
                .await
                .map_err(|e| internal("sources.update_health", e))?;
            return Ok(health);
        }
    };
    let ctx = ConnCtx {
        run_id: streams.run_id,
        source_id: id,
        person,
        source_identities: identities,
        auth,
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

/// F-8 (DAY-106 / [#113](https://github.com/vedanthvdev/dayseam/issues/113)).
/// Rejects a proposed `LocalGit` `scan_roots` set if any root would
/// overlap — be equal to, or an ancestor or descendant of — a root
/// already declared by another LocalGit source.
///
/// The `local_repos` table is primary-keyed on `path` alone (see
/// [`crates/dayseam-db/migrations/0001_initial.sql`](crates/dayseam-db/migrations/0001_initial.sql)),
/// so two LocalGit sources whose scan roots overlap would take turns
/// claiming ownership of the shared repos on every rescan — the
/// walker is per-source but the row isn't. The visible symptoms are
/// (a) sidebar chip counts flickering between sources and (b)
/// [`LocalRepoRepo::reconcile_for_source`] scoping its delete to
/// `source_id = ?` and therefore no longer pruning rows that another
/// source has since claimed. Cross-source event dedup downstream in
/// `dayseam-report` absorbs the duplicate-commit fallout, so there
/// is no *data* corruption, only UX confusion — but the UX confusion
/// is real and has no in-product recovery path.
///
/// Rather than migrate the schema to a composite `(source_id, path)`
/// key (the correct but larger `semver:minor` fix still tracked on
/// #113), this probe stops overlap at the IPC boundary: both
/// [`sources_add`] and [`sources_update`] call it before any source
/// row is mutated, so the "no overlap" invariant is front-loaded and
/// the DB never observes the bad state.
///
/// `self_source_id` is `Some(id)` on `sources_update` so the source
/// being edited can shrink or keep its own scan roots without
/// reporting itself as a contender, and `None` on `sources_add`.
///
/// The probe is pure path-prefix reasoning on canonicalised roots —
/// no filesystem walk, no `local_repos` query — so it runs in
/// microseconds and returns the same answer regardless of walk order
/// or discovery state. A root whose `canonicalize()` call fails
/// (typo, permission-denied, missing folder) falls back to the raw
/// declared path so a not-yet-existing scan root still participates
/// in the comparison rather than silently bypassing it.
async fn ensure_local_git_scan_roots_are_disjoint(
    state: &AppState,
    self_source_id: Option<SourceId>,
    proposed: &[std::path::PathBuf],
) -> Result<(), DayseamError> {
    if proposed.is_empty() {
        return Ok(());
    }
    let canonical_proposed: Vec<std::path::PathBuf> = proposed
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();

    let source_repo = SourceRepo::new(state.pool.clone());
    let all_sources = source_repo
        .list()
        .await
        .map_err(|e| internal("sources.list", e))?;

    for existing in &all_sources {
        if existing.kind != SourceKind::LocalGit {
            continue;
        }
        if let Some(self_id) = self_source_id {
            if existing.id == self_id {
                continue;
            }
        }
        let SourceConfig::LocalGit {
            scan_roots: existing_roots,
        } = &existing.config
        else {
            continue;
        };
        for existing_root in existing_roots {
            let canonical_existing =
                std::fs::canonicalize(existing_root).unwrap_or_else(|_| existing_root.clone());
            for (declared, canonical) in proposed.iter().zip(canonical_proposed.iter()) {
                if paths_overlap(canonical, &canonical_existing) {
                    return Err(invalid_config(
                        error_codes::IPC_SOURCE_SCAN_ROOT_OVERLAP,
                        format!(
                            "Scan root {declared:?} overlaps with source \"{label}\" \
                             (scan root {existing_root:?}). Two local-git sources whose \
                             scan roots contain one another would ping-pong ownership of \
                             every shared repo on each rescan. Remove the other source, \
                             or narrow this scan root so no discovered repo would be \
                             tracked twice.",
                            label = existing.label,
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Two scan roots overlap if they resolve to the same path, or if
/// one is a strict ancestor of the other on the filesystem tree.
/// Siblings under a common parent do not overlap — `~/code/alpha`
/// and `~/code/beta` can coexist in two LocalGit sources without
/// producing a shared repo set.
fn paths_overlap(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
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
    // DOGFOOD-v0.4-03: build the full "keep" set first, then
    // reconcile in one transaction. The previous per-row `upsert`
    // loop never deleted stale rows, so the sidebar chip would show
    // the cumulative high-water-mark of every repo ever discovered
    // for the source — even ones the user had since moved or
    // deleted. `reconcile_for_source` now diffs against the current
    // table and prunes anything outside the fresh walk.
    let keep: Vec<LocalRepo> = outcome
        .repos
        .into_iter()
        .map(|discovered| LocalRepo {
            path: discovered.path,
            label: discovered.label,
            is_private: false,
            discovered_at: now,
        })
        .collect();

    // DAY-103 F-2: if discovery truncated at `max_roots`, `keep`
    // only holds the first N repos and reconciling would delete
    // every DB row beyond the cap — including user-set `is_private`
    // flags that survive a normal rescan. Fall back to per-row
    // `upsert` (add/refresh newly-seen rows, touch nothing else)
    // and surface a warn so the user can raise the cap before the
    // next scan. The cap itself is tuning, not a safety boundary —
    // it's fine to leave the excess rows in place.
    if outcome.truncated {
        tracing::warn!(
            source_id = %source_id,
            kept = keep.len(),
            "local-git discovery truncated at max_roots; skipping reconcile and falling back \
             to additive upsert so stale rows beyond the cap are not nuked"
        );
        for row in &keep {
            repo.upsert(source_id, row)
                .await
                .map_err(|e| internal("local_repos.upsert", e))?;
        }
        return Ok(());
    }

    // DAY-103 F-3: a transient `read_dir` failure on the scan root
    // now surfaces as `discover_repos -> Err(_)` (see the scan-root
    // guard in `discovery.rs`), but even a clean walk that simply
    // returns zero results is suspicious when the DB still thinks
    // this source owned N repos a moment ago. Rather than commit
    // the one-way delete, we refuse to reconcile an empty discovery
    // against a non-empty DB and warn. The user can recover by
    // rescanning once the transient condition clears, or by deleting
    // the source deliberately if the empty result is intentional.
    if keep.is_empty() {
        let prior = repo
            .list_for_source(source_id)
            .await
            .map_err(|e| internal("local_repos.list_for_source", e))?;
        if !prior.is_empty() {
            tracing::warn!(
                source_id = %source_id,
                prior_count = prior.len(),
                "local-git discovery returned zero repos but DB has {prior} tracked row(s); \
                 skipping reconcile (data-loss guard). Run rescan after verifying scan roots.",
                prior = prior.len(),
            );
            return Ok(());
        }
    }

    let removed = repo
        .reconcile_for_source(source_id, &keep)
        .await
        .map_err(|e| internal("local_repos.reconcile", e))?;
    if removed > 0 {
        tracing::info!(
            source_id = %source_id,
            removed,
            kept = keep.len(),
            "reconciled local_repos table against fresh discovery pass"
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
        // DAY-70: route the stored PAT into the orchestrator's
        // `SourceHandle`. Previously we hardcoded `Arc::new(NoneAuth)`
        // here, which was the root cause of "report comes back
        // empty" on self-hosted GitLab: the walker's requests went
        // out with no `PRIVATE-TOKEN` header, GitLab returned a
        // successful empty list for the unauthenticated user, and
        // the run silently completed with zero events.
        let auth = build_source_auth(&state, &source)?;
        sources.push(SourceHandle {
            source_id: source.id,
            kind: source.kind,
            auth,
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
        let http = connectors_sdk::HttpClient::new().expect("build HttpClient");
        let state = AppState::new(
            pool,
            app_bus,
            Arc::new(InMemoryStore::new()),
            orchestrator,
            http,
        );
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

    // --- DAY-70: GitLab PAT plumbing ---------------------------------------
    //
    // These tests cover the helpers introduced to fix the
    // "reports come back empty on self-hosted GitLab" bug.
    // The full tauri::command wrappers need a `State<'_, AppState>`
    // and so are exercised end-to-end from the frontend tests; here
    // we pin down the individual pieces the wrappers are built from.

    fn gitlab_source(id: Uuid, secret_ref: Option<SecretRef>) -> Source {
        Source {
            id,
            kind: SourceKind::GitLab,
            label: "gitlab.example.com".into(),
            config: SourceConfig::GitLab {
                base_url: "https://gitlab.example.com".into(),
                user_id: 17,
                username: "vedanth".into(),
            },
            secret_ref,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        }
    }

    #[test]
    fn gitlab_secret_ref_is_stable_per_source_id() {
        // The `SourceId` is what disambiguates two GitLab sources
        // that happen to target the same host, so the keychain row
        // must be keyed off it. Pin the exact format: if we change
        // it later without a migration, every existing install's
        // PAT becomes unreadable and every report comes back empty
        // again. That's exactly the bug we just fixed.
        let id = uuid::uuid!("11111111-2222-3333-4444-555555555555");
        let sr = gitlab_secret_ref(id);
        assert_eq!(sr.keychain_service, "dayseam.gitlab");
        assert_eq!(
            sr.keychain_account,
            format!("source:{id}"),
            "account format is a stored artifact"
        );
        assert_eq!(
            secret_store_key(&sr),
            "dayseam.gitlab::source:11111111-2222-3333-4444-555555555555"
        );
    }

    #[tokio::test]
    async fn persist_and_build_source_auth_round_trips_pat() {
        let (state, _dir) = make_state().await;
        let id = Uuid::new_v4();
        let pat = IpcSecretString::new("glpat-test-token");

        let secret_ref = persist_gitlab_pat(&state, id, &pat).expect("persist pat");
        assert_eq!(secret_ref, gitlab_secret_ref(id));

        let source = gitlab_source(id, Some(secret_ref.clone()));
        let auth = build_source_auth(&state, &source).expect("build auth");
        // PatAuth encodes its descriptor with the keychain pointer,
        // which is how we assert the strategy threads the right
        // token through to the connector without exposing the PAT.
        assert_eq!(auth.name(), "pat");
        assert_eq!(
            auth.descriptor(),
            connectors_sdk::AuthDescriptor::Pat {
                keychain_service: secret_ref.keychain_service.clone(),
                keychain_account: secret_ref.keychain_account.clone(),
            }
        );
    }

    #[tokio::test]
    async fn build_source_auth_errors_when_gitlab_secret_ref_missing() {
        // Regression: the original bug was `sources_add` happily
        // persisting a GitLab source with `secret_ref: None`, and
        // then `report_generate` silently building `NoneAuth`.
        // Now the auth builder must refuse outright so the UI
        // shows the Reconnect card instead of producing an empty
        // report.
        let (state, _dir) = make_state().await;
        let source = gitlab_source(Uuid::new_v4(), None);
        let err = build_source_auth(&state, &source).expect_err("must error");
        match err {
            DayseamError::Auth {
                code, action_hint, ..
            } => {
                assert_eq!(code, error_codes::GITLAB_AUTH_INVALID_TOKEN);
                assert_eq!(action_hint.as_deref(), Some("reconnect"));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn build_source_auth_errors_when_keychain_slot_empty() {
        // `secret_ref` is set on the row but the keychain itself
        // has no entry — e.g. the user wiped their keychain or
        // restored the DB to a different machine. We want the same
        // reconnect flow as a missing `secret_ref`.
        let (state, _dir) = make_state().await;
        let id = Uuid::new_v4();
        let source = gitlab_source(id, Some(gitlab_secret_ref(id)));
        let err = build_source_auth(&state, &source).expect_err("must error");
        match err {
            DayseamError::Auth { code, .. } => {
                assert_eq!(code, error_codes::GITLAB_AUTH_INVALID_TOKEN);
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    // --- DAY-103 F-3: reconcile data-loss guard -----------------------
    //
    // `upsert_discovered_repos` used to call
    // `LocalRepoRepo::reconcile_for_source` unconditionally. A
    // transient `read_dir` failure on a scan root's children (now
    // only possible *below* the scan root after the scan-root
    // guard in `discovery.rs`) could still produce an empty
    // outcome for a source that had real rows a moment ago, and
    // the reconcile would commit the delete. The guard makes
    // "empty walk + non-empty DB" a no-op with a warn log instead
    // of silent data loss.

    #[tokio::test]
    async fn upsert_discovered_repos_refuses_to_nuke_source_on_empty_walk() {
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();

        // Seed a LocalGit source row so the DB foreign-key is
        // satisfied when we write to `local_repos`.
        let source_repo = SourceRepo::new(state.pool.clone());
        source_repo
            .insert(&Source {
                id: source_id,
                kind: SourceKind::LocalGit,
                label: "work repos".into(),
                config: SourceConfig::LocalGit {
                    scan_roots: vec![PathBuf::from("/ignored-by-this-test")],
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("seed source row");

        // Plant two rows the user "owns" — including an
        // `is_private = true` flag that regressed-behaviour would
        // silently drop. These represent repos the user had the
        // last time discovery ran cleanly.
        let local_repos = LocalRepoRepo::new(state.pool.clone());
        for (path, is_private) in [
            (PathBuf::from("/home/user/Code/alpha"), false),
            (PathBuf::from("/home/user/Code/private"), true),
        ] {
            local_repos
                .upsert(
                    &source_id,
                    &LocalRepo {
                        path,
                        label: "stub".into(),
                        is_private,
                        discovered_at: Utc::now(),
                    },
                )
                .await
                .expect("seed local_repo");
        }

        // Simulate the "empty walk" failure mode by pointing the
        // scan-roots list at an empty tempdir. The walker returns
        // cleanly (no error) with zero repos, which is exactly the
        // shape the guard has to refuse.
        let empty_root = TempDir::new().expect("empty scan root");
        let before = local_repos
            .list_for_source(&source_id)
            .await
            .expect("list before");
        assert_eq!(before.len(), 2);

        upsert_discovered_repos(&state, &source_id, &[empty_root.path().to_path_buf()])
            .await
            .expect("guarded call must not error");

        let after = local_repos
            .list_for_source(&source_id)
            .await
            .expect("list after");
        assert_eq!(
            after.len(),
            2,
            "empty walk against a non-empty DB must be a no-op (data-loss guard)",
        );
        assert!(
            after.iter().any(|r| r.is_private),
            "is_private flag on the survivor must be preserved by the guard",
        );
    }

    #[tokio::test]
    async fn upsert_discovered_repos_allows_empty_walk_when_db_is_also_empty() {
        // The guard should only refuse when there's data to lose.
        // A fresh source with zero rows and an empty scan root is
        // a legitimate "first walk found nothing" — no warn-worthy.
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        let source_repo = SourceRepo::new(state.pool.clone());
        source_repo
            .insert(&Source {
                id: source_id,
                kind: SourceKind::LocalGit,
                label: "empty".into(),
                config: SourceConfig::LocalGit {
                    scan_roots: vec![PathBuf::from("/ignored")],
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("seed source");

        let empty_root = TempDir::new().expect("empty scan root");
        upsert_discovered_repos(&state, &source_id, &[empty_root.path().to_path_buf()])
            .await
            .expect("empty-on-empty path must be Ok");

        let rows = LocalRepoRepo::new(state.pool.clone())
            .list_for_source(&source_id)
            .await
            .expect("list");
        assert!(rows.is_empty());
    }

    // --- F-8 (DAY-106 / #113): scan-root overlap guard ---------------
    //
    // Regression battery for `ensure_local_git_scan_roots_are_disjoint`.
    // The probe is the whole fix for F-8 at the IPC boundary, so each
    // failure mode deserves its own named test rather than a single
    // table-driven case — a future reader chasing a regression should
    // be able to find the exact test that pins their suspect invariant
    // by name, not by branch-index.

    /// Helper: seed a persisted LocalGit source with the given label
    /// and scan roots so the probe has something to compare against.
    async fn seed_local_git_source(
        state: &AppState,
        label: &str,
        scan_roots: Vec<PathBuf>,
    ) -> SourceId {
        let id = Uuid::new_v4();
        SourceRepo::new(state.pool.clone())
            .insert(&Source {
                id,
                kind: SourceKind::LocalGit,
                label: label.into(),
                config: SourceConfig::LocalGit { scan_roots },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("seed local-git source");
        id
    }

    fn assert_overlap_error(err: &DayseamError, expected_contender: &str) {
        match err {
            DayseamError::InvalidConfig { code, message } => {
                assert_eq!(code, error_codes::IPC_SOURCE_SCAN_ROOT_OVERLAP);
                assert!(
                    message.contains(expected_contender),
                    "overlap error must name the contending source in its message; got: {message}",
                );
            }
            other => panic!("expected InvalidConfig overlap, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn overlap_guard_accepts_disjoint_scan_roots_on_add() {
        // `~/code/alpha` + `~/code/beta` share a parent but overlap
        // nothing, which is the common "two sibling workspaces" case
        // the probe must never reject.
        let (state, dir) = make_state().await;
        let alpha = dir.path().join("alpha");
        let beta = dir.path().join("beta");
        std::fs::create_dir(&alpha).unwrap();
        std::fs::create_dir(&beta).unwrap();

        seed_local_git_source(&state, "Alpha", vec![alpha]).await;

        ensure_local_git_scan_roots_are_disjoint(&state, None, &[beta])
            .await
            .expect("sibling roots under a shared parent must not trip the overlap guard");
    }

    #[tokio::test]
    async fn overlap_guard_rejects_add_whose_root_equals_existing_root() {
        let (state, dir) = make_state().await;
        let shared = dir.path().join("code");
        std::fs::create_dir(&shared).unwrap();

        seed_local_git_source(&state, "Work", vec![shared.clone()]).await;

        let err = ensure_local_git_scan_roots_are_disjoint(&state, None, &[shared])
            .await
            .expect_err("identical scan root must be rejected");
        assert_overlap_error(&err, "Work");
    }

    #[tokio::test]
    async fn overlap_guard_rejects_add_whose_root_is_ancestor_of_existing() {
        // Adding `~/code` when `~/code/foo` already exists: every
        // repo the existing source discovers would also fall under
        // the new source, so two rows per shared path on each
        // rescan — the exact F-8 shape.
        let (state, dir) = make_state().await;
        let parent = dir.path().join("code");
        let child = parent.join("foo");
        std::fs::create_dir_all(&child).unwrap();

        seed_local_git_source(&state, "Existing", vec![child]).await;

        let err = ensure_local_git_scan_roots_are_disjoint(&state, None, &[parent])
            .await
            .expect_err("ancestor-of-existing scan root must be rejected");
        assert_overlap_error(&err, "Existing");
    }

    #[tokio::test]
    async fn overlap_guard_rejects_add_whose_root_is_descendant_of_existing() {
        // Adding `~/code/foo` when `~/code` already exists — the
        // symmetric case of the previous test. Same failure mode
        // (ping-pong ownership), so the probe must catch both
        // directions.
        let (state, dir) = make_state().await;
        let parent = dir.path().join("code");
        let child = parent.join("foo");
        std::fs::create_dir_all(&child).unwrap();

        seed_local_git_source(&state, "Existing", vec![parent]).await;

        let err = ensure_local_git_scan_roots_are_disjoint(&state, None, &[child])
            .await
            .expect_err("descendant-of-existing scan root must be rejected");
        assert_overlap_error(&err, "Existing");
    }

    #[tokio::test]
    async fn overlap_guard_ignores_non_local_git_sources() {
        // A Jira or GitHub source has no `scan_roots` to compare, so
        // it must never contribute false-positive overlap. Pattern-
        // matching the config guards this at compile time, but this
        // test pins the behaviour explicitly for anyone who might
        // later widen the probe to compare labels or ids.
        let (state, dir) = make_state().await;
        let code = dir.path().join("code");
        std::fs::create_dir(&code).unwrap();

        // Seed a Jira source whose label / workspace_url are
        // deliberately string-similar to the kind of path a LocalGit
        // source might declare, to rule out accidental prefix
        // comparison across kinds.
        let jira_id = Uuid::new_v4();
        SourceRepo::new(state.pool.clone())
            .insert(&Source {
                id: jira_id,
                kind: SourceKind::Jira,
                label: "Acme".into(),
                config: SourceConfig::Jira {
                    workspace_url: "https://acme.atlassian.net".into(),
                    email: "me@acme.example".into(),
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("seed jira");

        ensure_local_git_scan_roots_are_disjoint(&state, None, &[code])
            .await
            .expect("non-LocalGit sources must be invisible to the overlap probe");
    }

    #[tokio::test]
    async fn overlap_guard_skips_self_source_on_update() {
        // A source editing its own config must never report itself
        // as a contender — otherwise any `sources_update` that
        // includes the LocalGit config (even a pure label rename
        // carrying the unchanged scan_roots for round-trip) would
        // fail the guard and users could never edit LocalGit
        // sources again.
        let (state, dir) = make_state().await;
        let code = dir.path().join("code");
        std::fs::create_dir(&code).unwrap();

        let id = seed_local_git_source(&state, "Self", vec![code.clone()]).await;

        ensure_local_git_scan_roots_are_disjoint(&state, Some(id), &[code])
            .await
            .expect("update with self's own scan roots must be accepted");
    }

    #[tokio::test]
    async fn overlap_guard_rejects_update_that_introduces_overlap() {
        // Source A has `~/code/alpha`, source B has `~/code/beta`.
        // Editing B to use `~/code` would swallow A's tree — the
        // probe must catch this at the IPC boundary so B's bad
        // config never hits sqlite.
        let (state, dir) = make_state().await;
        let parent = dir.path().join("code");
        let alpha = parent.join("alpha");
        let beta = parent.join("beta");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();

        seed_local_git_source(&state, "A", vec![alpha]).await;
        let b_id = seed_local_git_source(&state, "B", vec![beta]).await;

        let err = ensure_local_git_scan_roots_are_disjoint(&state, Some(b_id), &[parent])
            .await
            .expect_err("update that widens B's root to contain A must be rejected");
        assert_overlap_error(&err, "A");
    }

    #[tokio::test]
    async fn overlap_guard_uses_canonical_path_for_comparison() {
        // Two textually-different scan roots that canonicalise to
        // the same path (trailing `/.`, normalised separators, etc.)
        // must still be flagged as overlap — otherwise a user could
        // bypass the guard with a cosmetic textual tweak.
        let (state, dir) = make_state().await;
        let code = dir.path().join("code");
        std::fs::create_dir(&code).unwrap();

        seed_local_git_source(&state, "Existing", vec![code.clone()]).await;

        // Same directory via `/.` suffix. `canonicalize` resolves
        // both to `code`, so the guard should still fire.
        let aliased = code.join(".");
        let err = ensure_local_git_scan_roots_are_disjoint(&state, None, &[aliased])
            .await
            .expect_err("aliased scan root must canonicalise to the existing root and be rejected");
        assert_overlap_error(&err, "Existing");
    }

    #[tokio::test]
    async fn build_source_auth_returns_none_auth_for_local_git() {
        let (state, _dir) = make_state().await;
        let source = Source {
            id: Uuid::new_v4(),
            kind: SourceKind::LocalGit,
            label: "work repos".into(),
            config: SourceConfig::LocalGit {
                scan_roots: vec![PathBuf::from("/tmp/repos")],
            },
            secret_ref: None,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        let auth = build_source_auth(&state, &source).expect("local-git auth");
        assert_eq!(auth.name(), "none");
    }

    // --- Atlassian auth (DOG-v0.2-01 regression) -------------------------
    //
    // v0.2.0 shipped `build_source_auth` with the Jira/Confluence arm
    // stubbed to `Err(DayseamError::Unsupported)` — the dialog would
    // happily add Atlassian sources and validate credentials, and then
    // the first `report_generate` returned
    // "connector.unsupported_sync_request" with no pointer back to the
    // auth helper that refused to build a strategy. These tests pin
    // the fixed contract: the arm builds a `BasicAuth` from the
    // per-source email + keychain-stored API token, and errors out
    // via the `atlassian.auth.invalid_credentials` reconnect flow when
    // either is missing.

    fn atlassian_secret_ref() -> SecretRef {
        SecretRef {
            keychain_service: "dayseam.atlassian".into(),
            keychain_account: "workspace:acme".into(),
        }
    }

    fn jira_source(id: Uuid, email: &str, secret_ref: Option<SecretRef>) -> Source {
        Source {
            id,
            kind: SourceKind::Jira,
            label: "Jira — acme".into(),
            config: SourceConfig::Jira {
                workspace_url: "https://acme.atlassian.net".into(),
                email: email.into(),
            },
            secret_ref,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        }
    }

    fn confluence_source(id: Uuid, email: &str, secret_ref: Option<SecretRef>) -> Source {
        Source {
            id,
            kind: SourceKind::Confluence,
            label: "Confluence — acme".into(),
            config: SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net".into(),
                email: email.into(),
            },
            secret_ref,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        }
    }

    #[tokio::test]
    async fn build_source_auth_builds_basic_auth_for_jira_source() {
        let (state, _dir) = make_state().await;
        let sr = atlassian_secret_ref();
        state
            .secrets
            .put(
                &secret_store_key(&sr),
                Secret::new("atlassian-api-token".into()),
            )
            .expect("put");
        let source = jira_source(Uuid::new_v4(), "me@acme.com", Some(sr.clone()));
        let auth = build_source_auth(&state, &source).expect("build auth");
        assert_eq!(auth.name(), "basic");
        assert_eq!(
            auth.descriptor(),
            connectors_sdk::AuthDescriptor::Basic {
                email: "me@acme.com".into(),
                keychain_service: sr.keychain_service.clone(),
                keychain_account: sr.keychain_account.clone(),
            }
        );
    }

    #[tokio::test]
    async fn build_source_auth_builds_basic_auth_for_confluence_source() {
        // Journey C in the Add-Source dialog (Confluence-only) must
        // work even without a paired Jira sibling. v0.2.0 stored the
        // Confluence row without an email field, so this case was
        // impossible to service; DAY-84 added `email` to
        // `SourceConfig::Confluence` precisely for this.
        let (state, _dir) = make_state().await;
        let sr = atlassian_secret_ref();
        state
            .secrets
            .put(
                &secret_store_key(&sr),
                Secret::new("atlassian-api-token".into()),
            )
            .expect("put");
        let source = confluence_source(Uuid::new_v4(), "me@acme.com", Some(sr.clone()));
        let auth = build_source_auth(&state, &source).expect("build auth");
        assert_eq!(auth.name(), "basic");
        assert_eq!(
            auth.descriptor(),
            connectors_sdk::AuthDescriptor::Basic {
                email: "me@acme.com".into(),
                keychain_service: sr.keychain_service.clone(),
                keychain_account: sr.keychain_account.clone(),
            }
        );
    }

    #[tokio::test]
    async fn build_source_auth_errors_when_atlassian_secret_ref_missing() {
        let (state, _dir) = make_state().await;
        let source = jira_source(Uuid::new_v4(), "me@acme.com", None);
        let err = build_source_auth(&state, &source).expect_err("must error");
        match err {
            DayseamError::Auth {
                code, action_hint, ..
            } => {
                assert_eq!(code, error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS);
                assert_eq!(action_hint.as_deref(), Some("reconnect"));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn build_source_auth_errors_when_atlassian_keychain_slot_empty() {
        let (state, _dir) = make_state().await;
        let source = confluence_source(Uuid::new_v4(), "me@acme.com", Some(atlassian_secret_ref()));
        let err = build_source_auth(&state, &source).expect_err("must error");
        match err {
            DayseamError::Auth {
                code, action_hint, ..
            } => {
                assert_eq!(code, error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS);
                assert_eq!(action_hint.as_deref(), Some("reconnect"));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn build_source_auth_errors_when_atlassian_email_is_blank() {
        // A v0.2.0 Confluence row deserialised with
        // `#[serde(default)] email = ""` lands here; surface a clear
        // reconnect error instead of letting `BasicAuth::atlassian`
        // encode an empty-email header upstream.
        let (state, _dir) = make_state().await;
        let sr = atlassian_secret_ref();
        state
            .secrets
            .put(
                &secret_store_key(&sr),
                Secret::new("atlassian-api-token".into()),
            )
            .expect("put");
        let source = confluence_source(Uuid::new_v4(), "", Some(sr));
        let err = build_source_auth(&state, &source).expect_err("must error");
        match err {
            DayseamError::Auth {
                code, action_hint, ..
            } => {
                assert_eq!(code, error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS);
                assert_eq!(action_hint.as_deref(), Some("reconnect"));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn build_source_auth_returns_none_auth_for_local_git_post_atlassian() {
        // Sanity: adding the Atlassian arm did not break the earlier
        // LocalGit short-circuit.
        let (state, _dir) = make_state().await;
        let source = Source {
            id: Uuid::new_v4(),
            kind: SourceKind::LocalGit,
            label: "work repos".into(),
            config: SourceConfig::LocalGit {
                scan_roots: vec![PathBuf::from("/tmp/repos")],
            },
            secret_ref: None,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        let auth = build_source_auth(&state, &source).expect("local-git auth");
        assert_eq!(auth.name(), "none");
    }

    #[tokio::test]
    async fn best_effort_delete_secret_clears_keychain_slot() {
        let (state, _dir) = make_state().await;
        let id = Uuid::new_v4();
        let pat = IpcSecretString::new("glpat-delete-me");
        let sr = persist_gitlab_pat(&state, id, &pat).expect("persist");
        // Sanity: the slot is populated right after persist.
        let key = secret_store_key(&sr);
        assert!(state.secrets.get(&key).expect("get").is_some());

        best_effort_delete_secret(&state, &sr);
        assert!(
            state.secrets.get(&key).expect("get").is_none(),
            "keychain slot must be empty after delete"
        );
    }

    // --- validate_pat_arg --------------------------------------------------
    //
    // The fixture of bugs this guard closes:
    //   1. A stale frontend bundle calls the pre-DAY-70 shape
    //      `invoke("sources_update", { id, patch })` and omits `pat`
    //      entirely. Tauri happily deserialises `pat` as None and
    //      the old code silently skipped the keychain write, leaving
    //      the GitLab row with `secret_ref: None` so the next
    //      `report_generate` errored with `gitlab.auth.invalid_token`
    //      from the orchestrator — a cross-command failure mode with
    //      no pointer back to the save that dropped the PAT.
    //   2. A user hits Save in the reconnect dialog with a blank
    //      PAT field (e.g. pasted and deleted). Without this guard
    //      the update succeeds, the row keeps its old broken state,
    //      and the user re-opens the dialog convinced the save did
    //      nothing.

    fn local_git_source(id: Uuid) -> Source {
        Source {
            id,
            kind: SourceKind::LocalGit,
            label: "work repos".into(),
            config: SourceConfig::LocalGit {
                scan_roots: vec![PathBuf::from("/tmp/repos")],
            },
            secret_ref: None,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        }
    }

    #[test]
    fn validate_pat_arg_local_git_allows_no_pat() {
        let src = local_git_source(Uuid::new_v4());
        validate_pat_arg(&src, None).expect("LocalGit with no pat is OK");
    }

    #[test]
    fn validate_pat_arg_local_git_rejects_pat() {
        let src = local_git_source(Uuid::new_v4());
        let pat = IpcSecretString::new("glpat-unexpected");
        let err = validate_pat_arg(&src, Some(&pat)).expect_err("LocalGit + pat must error");
        match err {
            DayseamError::InvalidConfig { code, .. } => {
                assert_eq!(code, error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH);
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_pat_arg_gitlab_with_secret_allows_no_pat_for_label_edit() {
        let id = Uuid::new_v4();
        let src = gitlab_source(id, Some(gitlab_secret_ref(id)));
        validate_pat_arg(&src, None)
            .expect("GitLab with existing secret_ref must allow label/config-only edits");
    }

    #[test]
    fn validate_pat_arg_gitlab_with_secret_rejects_empty_pat() {
        let id = Uuid::new_v4();
        let src = gitlab_source(id, Some(gitlab_secret_ref(id)));
        let empty = IpcSecretString::new("   ");
        let err = validate_pat_arg(&src, Some(&empty)).expect_err("empty pat must error");
        match err {
            DayseamError::InvalidConfig { code, .. } => {
                assert_eq!(code, error_codes::IPC_GITLAB_PAT_MISSING);
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_pat_arg_gitlab_with_secret_accepts_rotation() {
        let id = Uuid::new_v4();
        let src = gitlab_source(id, Some(gitlab_secret_ref(id)));
        let fresh = IpcSecretString::new("glpat-rotated");
        validate_pat_arg(&src, Some(&fresh)).expect("rotation must be accepted");
    }

    #[test]
    fn validate_pat_arg_gitlab_orphan_row_rejects_missing_pat() {
        // The exact silent-no-op failure mode DAY-70 users hit when
        // running a fresh backend against a stale frontend: the row
        // already has `secret_ref: None` (legacy add path), and the
        // IPC call arrives with `pat: None`. Without this guard
        // sources_update happily returns Ok and the user loops on
        // reconnect-then-generate forever.
        let src = gitlab_source(Uuid::new_v4(), None);
        let err = validate_pat_arg(&src, None).expect_err("orphan + no pat must error");
        match err {
            DayseamError::InvalidConfig { code, message } => {
                assert_eq!(code, error_codes::IPC_GITLAB_PAT_MISSING);
                assert!(
                    message.contains("no PAT on file"),
                    "message should name the failure mode, got: {message}"
                );
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_pat_arg_gitlab_orphan_row_accepts_fresh_pat() {
        let src = gitlab_source(Uuid::new_v4(), None);
        let fresh = IpcSecretString::new("glpat-first-time");
        validate_pat_arg(&src, Some(&fresh)).expect("orphan + fresh pat is the fix path");
    }

    // --- DAY-71: GitLab self-identity auto-seed ---------------------------
    //
    // The bug these pin down: a GitLab source could land in the DB
    // without a matching `GitLabUserId` [`SourceIdentity`], which meant
    // `dayseam-report::filter_events_by_self` dropped every event as
    // "unknown actor" and the rendered draft collapsed to "No tracked
    // activity" — even though the connector fetched and persisted the
    // events just fine. `ensure_gitlab_self_identity` plus the
    // `sources_add` / `sources_update` / startup-backfill call sites
    // guarantee the identity exists for every configured GitLab
    // source.

    /// Persist a minimally-valid GitLab [`Source`] row. The
    /// `source_identities` table FK-references `sources(id)` so tests
    /// that exercise [`ensure_gitlab_self_identity`] must seed the
    /// parent row first; production does the same — the helper only
    /// runs from `sources_add` / `sources_update` / startup, all of
    /// which guarantee the row exists when they call it.
    async fn seed_gitlab_source_row(state: &AppState, source_id: Uuid, user_id: i64) {
        SourceRepo::new(state.pool.clone())
            .insert(&Source {
                id: source_id,
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
            .expect("insert gitlab source fixture");
    }

    #[tokio::test]
    async fn ensure_gitlab_self_identity_seeds_missing_identity() {
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        seed_gitlab_source_row(&state, source_id, 291).await;

        ensure_gitlab_self_identity(&state, source_id, 291)
            .await
            .expect("first ensure must succeed");

        let person = PersonRepo::new(state.pool.clone())
            .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
            .await
            .expect("self person");
        let rows = SourceIdentityRepo::new(state.pool.clone())
            .list_for_source(person.id, &source_id)
            .await
            .expect("list");
        let seeded: Vec<_> = rows
            .into_iter()
            .filter(|r| r.kind == SourceIdentityKind::GitLabUserId && r.external_actor_id == "291")
            .collect();
        assert_eq!(
            seeded.len(),
            1,
            "exactly one GitLabUserId=291 identity must exist after ensure"
        );
    }

    #[tokio::test]
    async fn ensure_gitlab_self_identity_is_idempotent_on_repeat_calls() {
        // The startup backfill re-runs this code every boot, so a
        // regression that makes it throw on the second call would
        // turn into "every report is empty until the user clears
        // the db". This test is the guard.
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        seed_gitlab_source_row(&state, source_id, 42).await;
        ensure_gitlab_self_identity(&state, source_id, 42)
            .await
            .expect("first ensure");
        ensure_gitlab_self_identity(&state, source_id, 42)
            .await
            .expect("second ensure must be a no-op, not an error");

        let person = PersonRepo::new(state.pool.clone())
            .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
            .await
            .expect("self person");
        let rows = SourceIdentityRepo::new(state.pool.clone())
            .list_for_source(person.id, &source_id)
            .await
            .expect("list");
        assert_eq!(
            rows.iter()
                .filter(
                    |r| r.kind == SourceIdentityKind::GitLabUserId && r.external_actor_id == "42"
                )
                .count(),
            1,
            "repeat ensure must not duplicate the row"
        );
    }

    #[tokio::test]
    async fn ensure_gitlab_self_identity_resolves_via_identity_repo() {
        // End-to-end shape check: after ensure, the identity is
        // reachable via `resolve_person_id` keyed on the same
        // `(source, kind, external_actor_id)` the render-stage
        // filter uses. If this ever stops returning Some(self), the
        // production bug has regressed.
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        seed_gitlab_source_row(&state, source_id, 777).await;
        ensure_gitlab_self_identity(&state, source_id, 777)
            .await
            .expect("ensure");

        let person = PersonRepo::new(state.pool.clone())
            .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
            .await
            .expect("self");
        let resolved = SourceIdentityRepo::new(state.pool.clone())
            .resolve_person_id(Some(&source_id), SourceIdentityKind::GitLabUserId, "777")
            .await
            .expect("resolve");
        assert_eq!(resolved, Some(person.id));
    }

    // --- CONS-v0.2-01: Atlassian self-identity re-ensure ---------------

    /// Persist a minimally-valid Jira [`Source`] row plus a seeded
    /// `AtlassianAccountId` [`SourceIdentity`] — the shape
    /// `atlassian_sources_add` leaves behind on the happy path.
    async fn seed_atlassian_source_with_identity(
        state: &AppState,
        source_id: Uuid,
        kind: SourceKind,
        account_id: &str,
    ) {
        let config = match kind {
            SourceKind::Jira => SourceConfig::Jira {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "me@acme.com".into(),
            },
            SourceKind::Confluence => SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net/".into(),
                email: "me@acme.com".into(),
            },
            other => panic!("seed_atlassian_source_with_identity: non-atlassian kind {other:?}"),
        };
        SourceRepo::new(state.pool.clone())
            .insert(&Source {
                id: source_id,
                kind,
                label: "Acme".into(),
                config,
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("insert atlassian source fixture");

        let person = PersonRepo::new(state.pool.clone())
            .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
            .await
            .expect("self person");
        SourceIdentityRepo::new(state.pool.clone())
            .ensure(&SourceIdentity {
                id: Uuid::new_v4(),
                person_id: person.id,
                source_id: Some(source_id),
                kind: SourceIdentityKind::AtlassianAccountId,
                external_actor_id: account_id.to_string(),
            })
            .await
            .expect("seed atlassian identity fixture");
    }

    #[tokio::test]
    async fn ensure_atlassian_self_identity_reseeds_existing_row_idempotently() {
        // The common case: atlassian_sources_add already stamped the
        // row; sources_update / startup backfill just re-assert it.
        // A regression that makes ensure() throw on the re-assert
        // would regress every v0.2 atlassian install to empty
        // reports, so the idempotency guarantee gets its own test.
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        seed_atlassian_source_with_identity(&state, source_id, SourceKind::Jira, "557058:abc")
            .await;

        let first = ensure_atlassian_self_identity(&state, source_id)
            .await
            .expect("first ensure");
        assert!(first, "row exists on disk, ensure must report true");
        let second = ensure_atlassian_self_identity(&state, source_id)
            .await
            .expect("second ensure must be a no-op, not an error");
        assert!(second);

        let person = PersonRepo::new(state.pool.clone())
            .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
            .await
            .expect("self");
        let rows = SourceIdentityRepo::new(state.pool.clone())
            .list_for_source(person.id, &source_id)
            .await
            .expect("list");
        assert_eq!(
            rows.iter()
                .filter(|r| r.kind == SourceIdentityKind::AtlassianAccountId
                    && r.external_actor_id == "557058:abc")
                .count(),
            1,
            "repeat ensure must not duplicate the atlassian identity"
        );
    }

    #[tokio::test]
    async fn ensure_atlassian_self_identity_returns_false_when_no_identity_on_file() {
        // The manual-DB-surgery path: the source row is still there
        // but the identity row is gone. The helper must return
        // Ok(false) (not Err) so callers can decide whether to warn
        // or re-add-with-API. A regression that changes the return
        // type to an error would make sources_update fail hard for
        // users in that state.
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        SourceRepo::new(state.pool.clone())
            .insert(&Source {
                id: source_id,
                kind: SourceKind::Jira,
                label: "Orphan".into(),
                config: SourceConfig::Jira {
                    workspace_url: "https://acme.atlassian.net/".into(),
                    email: "me@acme.com".into(),
                },
                secret_ref: None,
                created_at: Utc::now(),
                last_sync_at: None,
                last_health: SourceHealth::unchecked(),
            })
            .await
            .expect("seed orphan source");

        let result = ensure_atlassian_self_identity(&state, source_id)
            .await
            .expect("orphan source must not error");
        assert!(
            !result,
            "missing identity must report Ok(false) so caller can warn rather than fail",
        );
    }

    #[tokio::test]
    async fn ensure_atlassian_self_identity_resolves_via_identity_repo() {
        // Parity with the GitLab resolve test: after ensure, the
        // identity is reachable via resolve_person_id keyed on the
        // same (source, kind, external_actor_id) the render-stage
        // filter uses. If this ever stops returning Some(self), the
        // self-filtering bug has regressed.
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        seed_atlassian_source_with_identity(
            &state,
            source_id,
            SourceKind::Confluence,
            "557058:xyz",
        )
        .await;
        ensure_atlassian_self_identity(&state, source_id)
            .await
            .expect("ensure");

        let person = PersonRepo::new(state.pool.clone())
            .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
            .await
            .expect("self");
        let resolved = SourceIdentityRepo::new(state.pool.clone())
            .resolve_person_id(
                Some(&source_id),
                SourceIdentityKind::AtlassianAccountId,
                "557058:xyz",
            )
            .await
            .expect("resolve");
        assert_eq!(resolved, Some(person.id));
    }
}
