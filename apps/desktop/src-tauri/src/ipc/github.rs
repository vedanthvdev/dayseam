//! GitHub IPC surface (DAY-99).
//!
//! Three commands live here:
//!
//!   1. [`github_validate_credentials`] — a one-shot
//!      `GET <api_base_url>/user` probe. The `AddGithubSourceDialog`
//!      calls it once the user has pasted a PAT and (optionally)
//!      overridden the API base URL so the dialog can show
//!      "Connected as @handle" before committing to a `sources_add`.
//!   2. [`github_sources_add`] — the transactional persist call that
//!      writes one `Source` row + one keychain row + one
//!      `SourceIdentity` row inside the same logical transaction.
//!      On any post-keychain-write failure the partially-written
//!      rows are swept back so a retry sees a clean slate. GitHub
//!      is single-product (unlike Atlassian), so there is no
//!      shared-PAT journey to model — each source owns exactly one
//!      keychain slot.
//!   3. [`github_sources_reconnect`] — rotate the PAT on an
//!      existing GitHub source. Validates the new token, asserts
//!      the `/user` numeric id still matches the source's bound
//!      `GitHubUserId` identity (defence against silent account
//!      rebinding), and overwrites the keychain entry at the
//!      existing `SecretRef`.
//!
//! Keychain keying mirrors the GitLab scheme rather than the
//! Atlassian slot-UUID scheme: one GitHub source owns exactly one
//! PAT, so keying by `source:<source_id>` (the GitLab shape) is the
//! simplest thing that cannot accidentally clobber a sibling.
//!
//! | connector | service              | account                    |
//! |-----------|----------------------|----------------------------|
//! | GitLab    | `dayseam.gitlab`     | `source:<source_id>`       |
//! | Atlassian | `dayseam.atlassian`  | `slot:<uuid>`              |
//! | GitHub    | `dayseam.github`     | `source:<source_id>`       |
//!
//! The GitLab shape is right for GitHub because no flow in v0.4 lets
//! two GitHub `Source` rows share a credential — GitHub is one PAT
//! per account, per source. Using the per-source keying scheme lines
//! the slot up with the owning row so delete + re-add is a pure
//! keychain replace rather than a slot-leak.
//!
//! This module does **not** build an auth strategy for persisted
//! sources — `commands::build_source_auth`'s `SourceKind::GitHub` arm
//! (landed in DAY-95) covers that path. DAY-99 is the IPC surface the
//! dialog drives; the walker and healthcheck paths were wired in
//! DAY-95 and DAY-96 already.

use chrono::Utc;
use connector_github::{list_identities, validate_auth, GithubUserInfo};
use connectors_sdk::PatAuth;
use dayseam_core::{
    error_codes, DayseamError, GithubValidationResult, SecretRef, Source, SourceConfig,
    SourceHealth, SourceId, SourceIdentity, SourceIdentityKind, SourceKind,
};
use dayseam_db::{PersonRepo, SourceIdentityRepo, SourceRepo};
use dayseam_secrets::Secret;
use tauri::State;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::ipc::commands::{
    invalid_config_public, persist_restart_required_toast, secret_store_key,
    SELF_DEFAULT_DISPLAY_NAME,
};
use crate::ipc::secret::IpcSecretString;
use crate::state::AppState;

/// Keychain `service` half for every GitHub PAT this app stores.
/// Matches the "Keychain Access readability" rationale the sibling
/// GitLab and Atlassian modules use — all `dayseam.github` entries
/// live under one heading and are the shape DAY-81's orphan-secret
/// audit on boot walks.
const GITHUB_KEYCHAIN_SERVICE: &str = "dayseam.github";

/// Stable [`SecretRef`] for a GitHub source's PAT. Each configured
/// GitHub source owns one keychain row keyed by its [`SourceId`];
/// the per-source keying is what lets the v0.1-style
/// `sources_delete` sweep tear the pair down atomically without a
/// refcount scan.
fn github_secret_ref(source_id: SourceId) -> SecretRef {
    SecretRef {
        keychain_service: GITHUB_KEYCHAIN_SERVICE.to_string(),
        keychain_account: format!("source:{source_id}"),
    }
}

/// Parse a caller-supplied `api_base_url` into an absolute `https://`
/// [`Url`] with a trailing slash so [`Url::join`] on the inner path
/// (`user`, `search/issues`, …) does not silently drop the last
/// segment.
///
/// The dialog normalises client-side first and defaults to
/// `https://api.github.com` for github.com tenants; this server-side
/// check is a defence-in-depth so a bespoke caller cannot round-trip
/// a malformed URL — or, crucially, an `http://` downgrade — into
/// `SourceConfig::GitHub`.
fn parse_api_base_url(input: &str) -> Result<Url, DayseamError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_GITHUB_INVALID_API_BASE_URL,
            "api_base_url must not be empty",
        ));
    }
    // Normalise: pad a trailing slash so `Url::join("user")` on the
    // result does the right thing. `GithubConfig::from_raw` does the
    // same for the connector half; doing it here too keeps the
    // persisted string canonical regardless of which caller writes
    // the row.
    let padded = if trimmed.ends_with('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    };
    let parsed = Url::parse(&padded).map_err(|e| {
        invalid_config_public(
            error_codes::IPC_GITHUB_INVALID_API_BASE_URL,
            format!("api_base_url `{trimmed}` is not a valid URL: {e}"),
        )
    })?;
    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_GITHUB_INVALID_API_BASE_URL,
            "api_base_url has no host component",
        ));
    }
    // DAY-111 / TST-v0.4-04. Scheme and host are evaluated together
    // so the `http`-for-tests escape hatch is scoped to loopback
    // origins only. Production builds continue to reject `http://`
    // (a downgrade over cleartext would ship the PAT in the clear);
    // under `test-helpers` / `#[cfg(test)]`, `http://127.0.0.1:PORT`
    // is the only cleartext form we accept, because that is the
    // shape wiremock hands `tests/reconnect_rebind.rs`.
    let host_lower = host.to_ascii_lowercase();
    let is_test_loopback = github_host_is_test_loopback(&host_lower);
    if parsed.scheme() != "https" && !(is_test_loopback && parsed.scheme() == "http") {
        return Err(invalid_config_public(
            error_codes::IPC_GITHUB_INVALID_API_BASE_URL,
            format!(
                "api_base_url scheme must be `https`; got `{}`",
                parsed.scheme()
            ),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(invalid_config_public(
            error_codes::IPC_GITHUB_INVALID_API_BASE_URL,
            "api_base_url must not carry a query string or fragment",
        ));
    }
    Ok(parsed)
}

/// Canonicalise a parsed `Url` to the string shape
/// `SourceConfig::GitHub.api_base_url` stores: scheme + host, with
/// optional port and path. A trailing slash is preserved so that
/// `Url::join` downstream stays safe. Mirrors the way
/// [`GithubConfig::from_raw`] normalises.
fn canonical_api_base_url(url: &Url) -> String {
    url.as_str().to_string()
}

/// DAY-111 / TST-v0.4-04. Narrow loopback carve-out for
/// `parse_api_base_url`: production builds never match, so the
/// `https`-only invariant holds in release. Under `test-helpers`
/// (or the crate's own `#[cfg(test)]` tests) `127.0.0.1` and
/// `localhost` are allowed so `tests/reconnect_rebind.rs` can point
/// the probe at a wiremock origin. The mock never sees a real
/// token because the whole test stack is in-process.
fn github_host_is_test_loopback(_host_lower: &str) -> bool {
    #[cfg(any(test, feature = "test-helpers"))]
    {
        if _host_lower == "127.0.0.1" || _host_lower == "localhost" {
            return true;
        }
    }
    false
}

fn require_nonempty_pat(pat: &str) -> Result<(), DayseamError> {
    if pat.trim().is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_GITHUB_PAT_MISSING,
            "pat must not be empty or whitespace-only",
        ));
    }
    Ok(())
}

/// Thin probe: parse the URL, build an ephemeral [`PatAuth`], call
/// [`validate_auth`] and project the result into a
/// [`GithubValidationResult`] the dialog renders.
///
/// The `PatAuth` descriptor records a synthetic `"probe"` keychain
/// slot because no keychain row actually backs this transient
/// credential. The probe path never hydrates a `SecretRef` from the
/// descriptor, so the placeholder is invisible to everything
/// downstream.
#[tauri::command]
pub async fn github_validate_credentials(
    api_base_url: String,
    pat: IpcSecretString,
    state: State<'_, AppState>,
) -> Result<GithubValidationResult, DayseamError> {
    github_validate_credentials_impl(&state, api_base_url, pat).await
}

/// Test-visible implementation of [`github_validate_credentials`]. Same
/// shape minus the Tauri [`State`] wrapper, which cannot be
/// constructed outside the Tauri runtime. DAY-111 moves the HTTP
/// client construction out of this function (onto `AppState::http`)
/// so `tests/reconnect_rebind.rs` can exercise the full probe path
/// against a wiremock-backed client without going through a live
/// network call.
pub async fn github_validate_credentials_impl(
    state: &AppState,
    api_base_url: String,
    pat: IpcSecretString,
) -> Result<GithubValidationResult, DayseamError> {
    require_nonempty_pat(pat.expose())?;
    let parsed = parse_api_base_url(&api_base_url)?;

    let auth = PatAuth::github(pat.expose(), GITHUB_KEYCHAIN_SERVICE, "probe");
    let info = validate_auth(&state.http, &auth, &parsed, &CancellationToken::new(), None).await?;

    Ok(GithubValidationResult {
        user_id: info.id,
        login: info.login,
        name: info.name,
    })
}

/// Unwind a partially-committed [`github_sources_add`] call. We log
/// but do not propagate secondary errors — the primary failure the
/// caller returns is the one that matters; a keychain row we failed
/// to delete is picked up by the boot-time orphan audit (DAY-81).
async fn rollback_sources_add(
    state: &AppState,
    source_repo: &SourceRepo,
    inserted: &Option<SourceId>,
    wrote_secret: bool,
    secret_ref: &SecretRef,
) {
    if let Some(id) = inserted {
        if let Err(e) = source_repo.delete(id).await {
            tracing::warn!(
                error = %e,
                source_id = %id,
                "github rollback: sources.delete failed; row may linger",
            );
        }
    }
    if wrote_secret {
        let key = secret_store_key(secret_ref);
        if let Err(e) = state.secrets.delete(&key) {
            tracing::warn!(
                error = %e,
                %key,
                "github rollback: keychain delete failed; row may linger",
            );
        }
    }
}

/// Persist a GitHub `Source` row, its keychain secret, and the
/// matching self-identity in a single transactional call.
///
/// This is the thin Tauri wrapper — the real work lives in
/// [`github_sources_add_impl`] so integration tests can drive the
/// same logic without a live [`State`].
#[tauri::command]
pub async fn github_sources_add(
    api_base_url: String,
    label: String,
    pat: IpcSecretString,
    user_id: i64,
    login: String,
    state: State<'_, AppState>,
) -> Result<Source, DayseamError> {
    github_sources_add_impl(&state, api_base_url, label, pat, user_id, login).await
}

/// Test-visible implementation of [`github_sources_add`].
pub async fn github_sources_add_impl(
    state: &AppState,
    api_base_url: String,
    label: String,
    pat: IpcSecretString,
    user_id: i64,
    login: String,
) -> Result<Source, DayseamError> {
    // ---- Structural validation -------------------------------------
    require_nonempty_pat(pat.expose())?;
    if user_id <= 0 {
        // Mirrors `list_identities`' own guard — a non-positive id
        // is a frontend bug (the dialog should have called
        // `github_validate_credentials` first and threaded the
        // returned id here).
        return Err(invalid_config_public(
            error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED,
            format!(
                "user_id must be positive; got {user_id}. Call github_validate_credentials first \
                 and pass its numeric id through."
            ),
        ));
    }
    let trimmed_login = login.trim().to_string();
    if trimmed_login.is_empty() {
        // `login` is the URL segment used to compose
        // `/users/{login}/events`; an empty value would silently
        // 404 every walk and `list_identities` would refuse to
        // seed the `GitHubLogin` identity. Catch it at the IPC
        // boundary so the error surface points at the dialog
        // (which already has `validation.result.login`) rather
        // than deep inside the walker.
        return Err(invalid_config_public(
            error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED,
            "login must be non-empty; call github_validate_credentials first and pass its \
             login string through alongside user_id."
                .to_string(),
        ));
    }
    let parsed_url = parse_api_base_url(&api_base_url)?;
    let canonical_url = canonical_api_base_url(&parsed_url);

    let trimmed_label = label.trim().to_string();
    let effective_label = if trimmed_label.is_empty() {
        // The dialog prefills a sensible default, but a bespoke
        // caller could omit it — fall back to the host so the row is
        // never unlabelled. `host_str` is safe here because
        // `parse_api_base_url` already rejected URLs without a host.
        format!(
            "GitHub — {}",
            parsed_url.host_str().unwrap_or(canonical_url.as_str())
        )
    } else {
        trimmed_label
    };

    // ---- Mint a fresh SourceId + keychain slot ---------------------
    let source_id = Uuid::new_v4();
    let secret_ref = github_secret_ref(source_id);
    let key = secret_store_key(&secret_ref);

    // Write the keychain row *before* inserting the sources row so
    // a failure here never leaves a row pointing at an empty slot.
    state
        .secrets
        .put(&key, Secret::new(pat.expose().to_string()))
        .map_err(|e| DayseamError::Internal {
            code: error_codes::IPC_GITHUB_KEYCHAIN_WRITE_FAILED.to_string(),
            message: format!("keychain write for {key} failed: {e}"),
        })?;
    let wrote_secret = true;
    let mut inserted: Option<SourceId> = None;

    let source_repo = SourceRepo::new(state.pool.clone());
    let identity_repo = SourceIdentityRepo::new(state.pool.clone());

    // ---- Person (self) for identity seeding ------------------------
    let self_person = match PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            rollback_sources_add(state, &source_repo, &inserted, wrote_secret, &secret_ref).await;
            return Err(DayseamError::Internal {
                code: "ipc.persons.bootstrap_self".to_string(),
                message: e.to_string(),
            });
        }
    };

    // ---- Insert the source row -------------------------------------
    let now = Utc::now();
    let source = Source {
        id: source_id,
        kind: SourceKind::GitHub,
        label: effective_label,
        config: SourceConfig::GitHub {
            api_base_url: canonical_url.clone(),
        },
        secret_ref: Some(secret_ref.clone()),
        created_at: now,
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    if let Err(e) = source_repo.insert(&source).await {
        rollback_sources_add(state, &source_repo, &inserted, wrote_secret, &secret_ref).await;
        return Err(DayseamError::Internal {
            code: "ipc.sources.insert".to_string(),
            message: format!("sources.insert(github) failed: {e}"),
        });
    }
    inserted = Some(source_id);

    // ---- Seed the self-identity rows -------------------------------
    // `list_identities` emits both a `GitHubUserId` and a
    // `GitHubLogin` row (CORR-v0.4-01 fix) — the walker requires
    // both to compose `/users/{login}/events` *and* filter
    // `event.actor.id == self`. Thread the real `login` that
    // `github_validate_credentials` returned; `name` is not
    // persisted here so a placeholder is fine.
    let info = GithubUserInfo {
        id: user_id,
        login: trimmed_login.clone(),
        name: None,
    };
    let identities = match list_identities(&info, source_id, self_person.id, None) {
        Ok(v) => v,
        Err(e) => {
            rollback_sources_add(state, &source_repo, &inserted, wrote_secret, &secret_ref).await;
            return Err(e);
        }
    };
    for identity in identities {
        if let Err(e) = identity_repo.ensure(&identity).await {
            rollback_sources_add(state, &source_repo, &inserted, wrote_secret, &secret_ref).await;
            return Err(DayseamError::Internal {
                code: "ipc.source_identities.ensure".to_string(),
                message: e.to_string(),
            });
        }
    }

    // ---- Commit, re-read, toast ------------------------------------
    persist_restart_required_toast(state);

    source_repo
        .get(&source_id)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "ipc.sources.get".to_string(),
            message: e.to_string(),
        })?
        .ok_or_else(|| {
            invalid_config_public(
                error_codes::IPC_SOURCE_NOT_FOUND,
                format!("source {source_id} disappeared immediately after insert"),
            )
        })
}

/// Rotate the PAT on an existing GitHub source.
///
/// The Reconnect chip on `SourceErrorCard` fires this command when a
/// GitHub source's last walk failed with
/// `github.auth.invalid_credentials`. The flow is:
///
///   1. Load the source by id; must exist and be `SourceKind::GitHub`.
///   2. Extract its `api_base_url` from `SourceConfig::GitHub` —
///      reconnect is a *rotation*, not an edit, so the target URL is
///      always the one already persisted.
///   3. Validate the new token against that URL. A 401 / 403 comes
///      back as `github.auth.invalid_credentials`, which the dialog
///      surfaces inline instead of persisting anything.
///   4. Assert the validated `/user` numeric id still matches the
///      source's existing `GitHubUserId` identity. A token valid for
///      a *different* account must not silently rebind the source —
///      that would make every event afterwards a cross-actor
///      rollup bug.
///   5. Overwrite the keychain entry at the existing `SecretRef`.
///
/// Returns the source id whose token was rotated so the caller can
/// fire `sources_healthcheck` to clear the red error chip
/// immediately rather than waiting for the next scheduled walk.
#[tauri::command]
pub async fn github_sources_reconnect(
    source_id: SourceId,
    pat: IpcSecretString,
    state: State<'_, AppState>,
) -> Result<SourceId, DayseamError> {
    github_sources_reconnect_impl(&state, source_id, pat).await
}

/// Test-visible implementation of [`github_sources_reconnect`].
pub async fn github_sources_reconnect_impl(
    state: &AppState,
    source_id: SourceId,
    pat: IpcSecretString,
) -> Result<SourceId, DayseamError> {
    require_nonempty_pat(pat.expose())?;

    let source_repo = SourceRepo::new(state.pool.clone());
    let identity_repo = SourceIdentityRepo::new(state.pool.clone());
    let person_repo = PersonRepo::new(state.pool.clone());

    // ---- 1. Load + kind-check -------------------------------------
    let source = source_repo
        .get(&source_id)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "ipc.sources.get".to_string(),
            message: e.to_string(),
        })?
        .ok_or_else(|| {
            invalid_config_public(
                error_codes::IPC_SOURCE_NOT_FOUND,
                format!("source {source_id} does not exist"),
            )
        })?;

    let api_base_url = match &source.config {
        SourceConfig::GitHub { api_base_url } => api_base_url.clone(),
        _ => {
            return Err(invalid_config_public(
                error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH,
                format!(
                    "source {source_id} is {:?}; github_sources_reconnect is GitHub only",
                    source.kind
                ),
            ));
        }
    };

    // A secret_ref-less GitHub source is an upgrade-path artefact
    // (rows whose keychain was wiped out from under the app). The
    // reconnect flow cannot rotate a token that has nowhere to land,
    // so route the user to delete-and-re-add instead of writing a
    // brand-new `SecretRef` here and silently forking the "add"
    // codepath.
    let secret_ref = source.secret_ref.clone().ok_or_else(|| {
        invalid_config_public(
            error_codes::IPC_GITHUB_PAT_MISSING,
            format!(
                "source {source_id} has no secret_ref on file; delete the source and add it again \
                 rather than reconnecting"
            ),
        )
    })?;

    // ---- 2. Validate new token against the bound account ----------
    //
    // DAY-111 pulls the [`HttpClient`] from `state.http` rather than
    // minting a fresh one per call — the same client the walker uses,
    // so keep-alive survives a validate→reconnect round-trip on the
    // same host and `tests/reconnect_rebind.rs` can route this probe
    // through a wiremock server.
    let parsed_url = parse_api_base_url(&api_base_url)?;
    let auth = PatAuth::github(pat.expose(), GITHUB_KEYCHAIN_SERVICE, "probe");
    let info = validate_auth(
        &state.http,
        &auth,
        &parsed_url,
        &CancellationToken::new(),
        None,
    )
    .await?;
    let new_user_id = info.id;

    // ---- 3. Identity-binding invariant ----------------------------
    // The reconnect flow only cares about identities bound to the
    // *self* person — that's the set `github_sources_add` seeded on
    // initial connect. `list_for_source` requires a `person_id`, so
    // resolve it via `bootstrap_self` the same way `sources_add` does;
    // this is idempotent and matches the row we wrote earlier.
    let self_person = person_repo
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "ipc.persons.bootstrap_self".to_string(),
            message: e.to_string(),
        })?;
    let identities = identity_repo
        .list_for_source(self_person.id, &source_id)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "ipc.source_identities.list_for_source".to_string(),
            message: e.to_string(),
        })?;
    let bound_user_id = identities
        .iter()
        .find(|id| matches!(id.kind, SourceIdentityKind::GitHubUserId))
        .map(|id| id.external_actor_id.clone());
    if let Some(existing) = bound_user_id {
        if existing != new_user_id.to_string() {
            return Err(DayseamError::Auth {
                code: error_codes::GITHUB_AUTH_INVALID_CREDENTIALS.to_string(),
                message: format!(
                    "token is valid but for user id `{new_user_id}`; this source is bound to \
                     `{existing}`. Paste a token for the original account, or delete this source \
                     and add a new one."
                ),
                retryable: false,
                action_hint: Some("reconnect".to_string()),
            });
        }
    }
    // Zero-identity-rows case: the source pre-dates DAY-99's identity
    // seeding (only possible if a row was hand-crafted before this
    // ticket). Treat reconnect as a self-healing path — rotate the
    // token and let the next sync seed the identity.
    //
    // Login-row self-heal (CORR-v0.4-01 recovery path): v0.4 pre-fix
    // sources may already exist with only the `GitHubUserId` row
    // (the IPC add path was dropping `login` before DAY-101). Ensure
    // the `GitHubLogin` row exists on every successful reconnect so
    // upgraded installs recover without the user having to
    // delete-and-re-add. Idempotent via `SourceIdentityRepo::ensure`.
    if !info.login.trim().is_empty() {
        let login_row = SourceIdentity {
            id: Uuid::new_v4(),
            person_id: self_person.id,
            source_id: Some(source_id),
            kind: SourceIdentityKind::GitHubLogin,
            external_actor_id: info.login.clone(),
        };
        if let Err(e) = identity_repo.ensure(&login_row).await {
            return Err(DayseamError::Internal {
                code: "ipc.source_identities.ensure".to_string(),
                message: format!("reconnect login-row self-heal failed: {e}"),
            });
        }
    }

    // ---- 4. Rotate the keychain slot ------------------------------
    let key = secret_store_key(&secret_ref);
    state
        .secrets
        .put(&key, Secret::new(pat.expose().to_string()))
        .map_err(|e| DayseamError::Internal {
            code: error_codes::IPC_GITHUB_KEYCHAIN_WRITE_FAILED.to_string(),
            message: format!("keychain write for {key} failed: {e}"),
        })?;

    Ok(source_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dayseam_db::open;
    use dayseam_events::AppBus;
    use dayseam_orchestrator::{ConnectorRegistry, OrchestratorBuilder, SinkRegistry};
    use dayseam_secrets::InMemoryStore;
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

    fn pat() -> IpcSecretString {
        IpcSecretString::new("ghp_faketokenforthetest")
    }

    #[test]
    fn parse_api_base_url_accepts_github_com() {
        let url = parse_api_base_url("https://api.github.com").unwrap();
        assert_eq!(canonical_api_base_url(&url), "https://api.github.com/");
    }

    #[test]
    fn parse_api_base_url_preserves_existing_trailing_slash() {
        let url = parse_api_base_url("https://api.github.com/").unwrap();
        assert_eq!(canonical_api_base_url(&url), "https://api.github.com/");
    }

    #[test]
    fn parse_api_base_url_accepts_github_enterprise_path() {
        // GHE rule: Enterprise hosts put the API under `/api/v3`.
        // Accept any host + path shape so self-hosted installs are
        // not forced through github.com.
        let url = parse_api_base_url("https://ghe.example.com/api/v3").unwrap();
        assert_eq!(
            canonical_api_base_url(&url),
            "https://ghe.example.com/api/v3/"
        );
    }

    #[test]
    fn parse_api_base_url_rejects_non_https() {
        let err = parse_api_base_url("http://api.github.com").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_GITHUB_INVALID_API_BASE_URL);
    }

    #[test]
    fn parse_api_base_url_rejects_empty() {
        let err = parse_api_base_url("   ").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_GITHUB_INVALID_API_BASE_URL);
    }

    #[test]
    fn parse_api_base_url_rejects_scheme_missing() {
        let err = parse_api_base_url("api.github.com").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_GITHUB_INVALID_API_BASE_URL);
    }

    #[test]
    fn parse_api_base_url_rejects_query_or_fragment() {
        for bad in [
            "https://api.github.com?foo=bar",
            "https://api.github.com#fragment",
        ] {
            let err = parse_api_base_url(bad).unwrap_err();
            assert_eq!(
                err.code(),
                error_codes::IPC_GITHUB_INVALID_API_BASE_URL,
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn github_secret_ref_is_stable_per_source() {
        let id = Uuid::new_v4();
        let a = github_secret_ref(id);
        let b = github_secret_ref(id);
        assert_eq!(a, b, "same source_id must produce the same SecretRef");
        assert_eq!(a.keychain_service, GITHUB_KEYCHAIN_SERVICE);
        assert!(a.keychain_account.starts_with("source:"));
    }

    #[test]
    fn github_secret_ref_differs_across_sources() {
        let a = github_secret_ref(Uuid::new_v4());
        let b = github_secret_ref(Uuid::new_v4());
        assert_ne!(a.keychain_account, b.keychain_account);
    }

    // -------------------------------------------------------------------
    // `github_sources_add_impl` IPC integration tests
    //
    // These pin the pre-network path: input validation and the
    // transactional rollback invariants. The happy path exercises the
    // DB + keychain writes without going through `validate_auth` (the
    // dialog calls that separately via
    // `github_validate_credentials`), so these tests run
    // offline.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn add_happy_path_writes_source_and_keychain_and_identity() {
        let (state, _dir) = make_state().await;

        let source = github_sources_add_impl(
            &state,
            "https://api.github.com".into(),
            "GitHub — vedanth".into(),
            pat(),
            17,
            "vedanth".into(),
        )
        .await
        .expect("happy path succeeds");

        assert_eq!(source.kind, SourceKind::GitHub);
        assert!(matches!(source.config, SourceConfig::GitHub { .. }));
        let sr = source.secret_ref.clone().expect("secret_ref present");
        assert_eq!(sr.keychain_service, GITHUB_KEYCHAIN_SERVICE);
        assert_eq!(sr.keychain_account, format!("source:{}", source.id));

        let value = state
            .secrets
            .get(&secret_store_key(&sr))
            .expect("keychain get")
            .expect("keychain row present");
        assert_eq!(value.expose_secret(), "ghp_faketokenforthetest");

        // CORR-v0.4-01: both a `GitHubUserId` row (filter-time key)
        // and a `GitHubLogin` row (URL-composition key) must land on
        // happy-path add, otherwise the walker silently returns
        // `WalkOutcome::default()` on every sync.
        let person = PersonRepo::new(state.pool.clone())
            .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
            .await
            .expect("bootstrap self");
        let ids = SourceIdentityRepo::new(state.pool.clone())
            .list_for_source(person.id, &source.id)
            .await
            .expect("list identities");
        let user_ids: Vec<_> = ids
            .iter()
            .filter(|i| matches!(i.kind, SourceIdentityKind::GitHubUserId))
            .collect();
        assert_eq!(user_ids.len(), 1, "exactly one GitHubUserId row");
        assert_eq!(user_ids[0].external_actor_id, "17");

        let logins: Vec<_> = ids
            .iter()
            .filter(|i| matches!(i.kind, SourceIdentityKind::GitHubLogin))
            .collect();
        assert_eq!(logins.len(), 1, "exactly one GitHubLogin row");
        assert_eq!(logins[0].external_actor_id, "vedanth");
    }

    #[tokio::test]
    async fn add_rejects_empty_login_before_any_write() {
        // Guard against the CORR-v0.4-01 regression: the dialog must
        // thread a real login through; refuse rather than write a
        // half-seeded source that produces zero events forever.
        let (state, _dir) = make_state().await;
        let err = github_sources_add_impl(
            &state,
            "https://api.github.com".into(),
            "lbl".into(),
            pat(),
            17,
            "   ".into(),
        )
        .await
        .expect_err("empty login must be rejected");
        assert_eq!(err.code(), error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED);
    }

    #[tokio::test]
    async fn add_falls_back_to_host_label_when_label_is_blank() {
        let (state, _dir) = make_state().await;
        let source = github_sources_add_impl(
            &state,
            "https://api.github.com".into(),
            "   ".into(),
            pat(),
            42,
            "octocat".into(),
        )
        .await
        .expect("add with blank label");
        assert_eq!(source.label, "GitHub — api.github.com");
    }

    #[tokio::test]
    async fn add_normalises_api_base_url_trailing_slash() {
        let (state, _dir) = make_state().await;
        let source = github_sources_add_impl(
            &state,
            "https://ghe.example.com/api/v3".into(),
            "GHE".into(),
            pat(),
            42,
            "octocat".into(),
        )
        .await
        .expect("add");
        match source.config {
            SourceConfig::GitHub { api_base_url } => {
                assert_eq!(api_base_url, "https://ghe.example.com/api/v3/");
            }
            other => panic!("wrong config kind: {other:?}"),
        }
    }

    #[tokio::test]
    async fn add_rejects_empty_pat() {
        let (state, _dir) = make_state().await;
        let err = github_sources_add_impl(
            &state,
            "https://api.github.com".into(),
            "lbl".into(),
            IpcSecretString::new("   "),
            17,
            "vedanth".into(),
        )
        .await
        .expect_err("empty pat must reject before touching sqlite");
        assert_eq!(err.code(), error_codes::IPC_GITHUB_PAT_MISSING);
    }

    #[tokio::test]
    async fn add_rejects_non_positive_user_id() {
        let (state, _dir) = make_state().await;
        let err = github_sources_add_impl(
            &state,
            "https://api.github.com".into(),
            "lbl".into(),
            pat(),
            0,
            "vedanth".into(),
        )
        .await
        .expect_err("non-positive user_id must reject");
        assert_eq!(err.code(), error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED);
    }

    #[tokio::test]
    async fn add_rejects_malformed_api_base_url() {
        let (state, _dir) = make_state().await;
        let err = github_sources_add_impl(
            &state,
            "not-a-url".into(),
            "lbl".into(),
            pat(),
            17,
            "vedanth".into(),
        )
        .await
        .expect_err("malformed url must reject");
        assert_eq!(err.code(), error_codes::IPC_GITHUB_INVALID_API_BASE_URL);
    }

    // -------------------------------------------------------------------
    // `github_sources_reconnect_impl` IPC integration tests
    //
    // The happy path routes through `validate_auth`, which hits the
    // live GitHub API and cannot be exercised from a pure unit test.
    // These cover the pre-network branches: empty token, missing
    // source, kind mismatch, and the secret-ref-missing upgrade
    // artefact.
    // -------------------------------------------------------------------

    async fn seed_github_source(state: &AppState) -> Source {
        github_sources_add_impl(
            state,
            "https://api.github.com".into(),
            "lbl".into(),
            pat(),
            17,
            "vedanth".into(),
        )
        .await
        .expect("seed github source")
    }

    #[tokio::test]
    async fn reconnect_rejects_empty_token() {
        let (state, _dir) = make_state().await;
        let source = seed_github_source(&state).await;
        let err = github_sources_reconnect_impl(&state, source.id, IpcSecretString::new("   "))
            .await
            .expect_err("whitespace-only token must fail before any network call");
        assert_eq!(err.code(), error_codes::IPC_GITHUB_PAT_MISSING);
    }

    #[tokio::test]
    async fn reconnect_rejects_missing_source() {
        let (state, _dir) = make_state().await;
        let err = github_sources_reconnect_impl(&state, Uuid::new_v4(), pat())
            .await
            .expect_err("unknown source_id must be rejected");
        assert_eq!(err.code(), error_codes::IPC_SOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn reconnect_rejects_non_github_source() {
        let (state, _dir) = make_state().await;
        let repo = SourceRepo::new(state.pool.clone());
        let local = Source {
            id: Uuid::new_v4(),
            kind: SourceKind::LocalGit,
            label: "Local".into(),
            config: SourceConfig::LocalGit { scan_roots: vec![] },
            secret_ref: None,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        repo.insert(&local).await.expect("insert local");

        let err = github_sources_reconnect_impl(&state, local.id, pat())
            .await
            .expect_err("LocalGit must not reconnect via the github IPC");
        assert_eq!(err.code(), error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH);
    }

    #[tokio::test]
    async fn reconnect_rejects_source_without_secret_ref() {
        let (state, _dir) = make_state().await;
        let repo = SourceRepo::new(state.pool.clone());
        let ghost = Source {
            id: Uuid::new_v4(),
            kind: SourceKind::GitHub,
            label: "ghost".into(),
            config: SourceConfig::GitHub {
                api_base_url: "https://api.github.com/".into(),
            },
            secret_ref: None,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        repo.insert(&ghost).await.expect("insert ghost");

        let err = github_sources_reconnect_impl(&state, ghost.id, pat())
            .await
            .expect_err("secret_ref-less github row must not accept reconnect");
        assert_eq!(err.code(), error_codes::IPC_GITHUB_PAT_MISSING);
    }
}
