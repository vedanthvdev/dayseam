//! Outlook IPC surface (DAY-203).
//!
//! Two commands land here on top of DAY-201's generic OAuth login
//! scaffold and DAY-202's Outlook connector:
//!
//!   1. [`outlook_validate_credentials`] — non-consuming probe. The
//!      Add-Outlook-source dialog calls it once
//!      `oauth_session_status` reports `Completed`: we peek at the
//!      session's stored `TokenPair`, pull `tid` out of the access
//!      token's JWT payload, and hit `GET /me` on Microsoft Graph.
//!      Returns the tuple the dialog renders in the "Signed in as …"
//!      ribbon. Non-consuming on purpose — a transient Graph 5xx
//!      between Validate and Add should not invalidate the user's
//!      consent.
//!   2. [`outlook_sources_add`] — commit step. Re-validates the
//!      session defensively (the frontend may skip the Validate
//!      button on retry), rejects `(tenant_id, upn)` collisions via
//!      [`SourceRepo::find_by_outlook_identity`], consumes the
//!      session, writes both Keychain rows (access + refresh) via
//!      the `dayseam_secrets::oauth` helpers, inserts the `sources`
//!      row, and seeds the two `source_identities` rows Graph's
//!      `/me` response projects into.
//!
//! Keychain keying mirrors the GitLab / GitHub per-source pattern:
//! one Outlook source owns two Keychain rows, both under the
//! `dayseam.outlook` service, with accounts
//! `source:{source_id}.oauth.access` and
//! `source:{source_id}.oauth.refresh`. The `secret_ref` persisted on
//! the `sources` row carries the access account; the refresh
//! account is derived at read time in `build_source_auth` so the
//! schema doesn't need a dedicated column for the second keychain
//! slot.
//!
//! Rollback on partial failure matches the GitHub flow: write the
//! Keychain rows first, insert the DB row second, and on any
//! post-Keychain failure sweep both Keychain rows and the DB row
//! back so a retry sees a clean slate. Secondary-failure deletes
//! are logged but not propagated — the primary error is what the
//! user needs to see, and the boot-time orphan-secret audit
//! (DAY-81) is the safety net for the rare Keychain-delete
//! failure.

use std::sync::Arc;

use chrono::Utc;
use connector_outlook::{list_identities, validate_auth, OutlookUserInfo, GRAPH_API_BASE_URL};
use connectors_sdk::{
    AuthDescriptor, NoopTokenPersister, OAuthAuth, SharedPersister, SystemClock, TokenPair,
};
use dayseam_core::{
    error_codes, DayseamError, OAuthSessionId, OutlookValidationResult, SecretRef, Source,
    SourceConfig, SourceHealth, SourceId, SourceKind,
};
use dayseam_db::{PersonRepo, SourceIdentityRepo, SourceRepo};
use dayseam_secrets::{oauth as oauth_secrets, Secret};
use tauri::State;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::ipc::commands::{persist_restart_required_toast, SELF_DEFAULT_DISPLAY_NAME};
use crate::ipc::outlook_jwt::extract_tid;
use crate::oauth_config::{OAuthProviderConfig, PROVIDER_MICROSOFT_OUTLOOK};
use crate::state::AppState;

/// Keychain `service` half for every Outlook OAuth token pair this
/// app stores. Matches the GitHub / GitLab constant-per-connector
/// pattern so Keychain Access renders all `dayseam.outlook` entries
/// under a single heading and the boot-time orphan audit walks the
/// same key shape the IPC layer writes.
pub const OUTLOOK_KEYCHAIN_SERVICE: &str = "dayseam.outlook";

/// Suffix appended to `source:{id}` to compose the access-token
/// keychain account. Kept `pub(crate)` so `build_source_auth` and
/// the boot-time audit can derive the same string without a schema
/// column round trip.
pub(crate) const OUTLOOK_ACCESS_ACCOUNT_SUFFIX: &str = ".oauth.access";

/// Suffix appended to `source:{id}` to compose the refresh-token
/// keychain account.
pub(crate) const OUTLOOK_REFRESH_ACCOUNT_SUFFIX: &str = ".oauth.refresh";

/// Compose the access-token keychain account string for `source_id`.
/// Format: `source:{uuid}.oauth.access`.
pub(crate) fn outlook_access_account(source_id: SourceId) -> String {
    format!("source:{source_id}{OUTLOOK_ACCESS_ACCOUNT_SUFFIX}")
}

/// Compose the refresh-token keychain account string for `source_id`.
/// Format: `source:{uuid}.oauth.refresh`.
pub(crate) fn outlook_refresh_account(source_id: SourceId) -> String {
    format!("source:{source_id}{OUTLOOK_REFRESH_ACCOUNT_SUFFIX}")
}

/// Stable [`SecretRef`] recorded on the `sources` row for an Outlook
/// source. Points at the access-token row; the refresh-token row is
/// discoverable via [`outlook_refresh_account`] applied to the same
/// `source_id`. One row per `SourceRef` keeps the sources-schema
/// shape unchanged from the GitLab/GitHub precedent — no dedicated
/// "secondary keychain slot" column.
fn outlook_secret_ref(source_id: SourceId) -> SecretRef {
    SecretRef {
        keychain_service: OUTLOOK_KEYCHAIN_SERVICE.to_string(),
        keychain_account: outlook_access_account(source_id),
    }
}

/// Parse the fixed v0.9 Graph base URL into a `Url` once, up front,
/// so every call site that joins `/me` onto it can skip the
/// fallible parse step.
fn graph_base_url() -> Result<Url, DayseamError> {
    Url::parse(GRAPH_API_BASE_URL).map_err(|err| DayseamError::Internal {
        code: "ipc.outlook.graph_base_url_parse".to_string(),
        message: format!("GRAPH_API_BASE_URL={GRAPH_API_BASE_URL} did not parse: {err}"),
    })
}

/// Microsoft's tenant-specific token endpoint. The DAY-201 login
/// flow talks to the `common` authority; post-Add refreshes talk to
/// the concrete tenant endpoint so the refresh token does not
/// accept a cross-tenant swap under any future policy change.
fn token_endpoint_for_tenant(tenant_id: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token")
}

/// Build the `AuthDescriptor::OAuth` that describes an Outlook
/// source's token pair. Issuer is the tenant-scoped v2 endpoint;
/// `client_id` / `scopes` come from the same
/// [`OAuthProviderConfig`] the login flow used so the descriptor
/// encodes the same consent contract the tokens were minted
/// against.
fn outlook_auth_descriptor(
    tenant_id: &str,
    provider: &OAuthProviderConfig,
    source_id: SourceId,
) -> AuthDescriptor {
    AuthDescriptor::OAuth {
        issuer: format!("https://login.microsoftonline.com/{tenant_id}/v2.0"),
        client_id: provider.client_id.clone(),
        scopes: provider.scopes.clone(),
        keychain_service: OUTLOOK_KEYCHAIN_SERVICE.to_string(),
        access_keychain_account: outlook_access_account(source_id),
        refresh_keychain_account: outlook_refresh_account(source_id),
    }
}

/// Build a one-shot [`OAuthAuth`] wrapping the supplied [`TokenPair`]
/// for a validate-only probe. Wired to a
/// [`NoopTokenPersister`] so a transient refresh inside the
/// `validate_auth` call (Microsoft sometimes issues tokens with a
/// 60s expiry while the orchestrator clock is a few seconds
/// behind) never leaks a fresh pair into the Keychain before the
/// user has committed to adding the source.
fn build_probe_auth(
    pair: &TokenPair,
    descriptor: AuthDescriptor,
    token_endpoint: String,
    http: reqwest::Client,
) -> Result<OAuthAuth, DayseamError> {
    let persister: SharedPersister = Arc::new(NoopTokenPersister);
    OAuthAuth::new(
        pair.access_token.clone(),
        pair.refresh_token.clone(),
        pair.access_expires_at,
        descriptor,
        token_endpoint,
        http,
        persister,
        Arc::new(SystemClock),
    )
}

/// DAY-203. Validate a freshly completed Outlook OAuth session
/// *without* consuming it.
///
/// Flow:
///   1. [`OAuthSessionRegistry::peek_token_pair`] — non-consuming;
///      returns `None` for missing / not-yet-completed sessions.
///   2. [`extract_tid`] — JWT payload base64url decode + JSON parse.
///   3. One-shot [`OAuthAuth`] wrapping the pair, pointed at the
///      tenant-scoped token endpoint (so any refresh inside the
///      probe hits the right authority).
///   4. [`validate_auth`] — `GET /me`. On success the Graph
///      response is projected into an [`OutlookValidationResult`].
///
/// Non-consuming on purpose: the user may click Validate, hit a
/// transient Graph 5xx, cancel, and retry without having to redo
/// the whole sign-in. The session (and its tokens) is only torn
/// down when [`outlook_sources_add`] runs.
#[tauri::command]
pub async fn outlook_validate_credentials(
    session_id: OAuthSessionId,
    state: State<'_, AppState>,
) -> Result<OutlookValidationResult, DayseamError> {
    outlook_validate_credentials_impl(&state, session_id).await
}

/// Test-visible implementation of [`outlook_validate_credentials`].
/// Mirrors the GitHub shape — no Tauri `State` wrapper — so unit
/// tests can drive the same logic with a bespoke [`AppState`].
pub async fn outlook_validate_credentials_impl(
    state: &AppState,
    session_id: OAuthSessionId,
) -> Result<OutlookValidationResult, DayseamError> {
    let pair = state
        .oauth_sessions
        .peek_token_pair(&session_id)
        .await
        .ok_or_else(|| DayseamError::InvalidConfig {
            code: error_codes::IPC_OUTLOOK_SESSION_NOT_READY.to_string(),
            message: format!(
                "OAuth session {session_id} is missing or has not produced a TokenPair yet; \
                 run oauth_begin_login and wait for status=completed before calling \
                 outlook_validate_credentials"
            ),
        })?;

    let tenant_id = extract_tid(&pair.access_token)?;
    let provider = crate::oauth_config::lookup_provider(PROVIDER_MICROSOFT_OUTLOOK)?;
    let descriptor = outlook_auth_descriptor(&tenant_id, &provider, SourceId::nil());
    let token_endpoint = token_endpoint_for_tenant(&tenant_id);
    let auth = build_probe_auth(
        &pair,
        descriptor,
        token_endpoint,
        state.http.reqwest().clone(),
    )?;

    let base = graph_base_url()?;
    let info = validate_auth(&state.http, &auth, &base, &CancellationToken::new(), None).await?;

    Ok(OutlookValidationResult {
        tenant_id,
        user_principal_name: info.user_principal_name,
        display_name: info.display_name,
        user_object_id: info.id,
    })
}

/// DAY-203. Persist a new Outlook calendar source in one IPC call.
///
/// Re-runs the validate probe (defensive against a client skipping
/// the Validate step), rejects `(tenant_id, upn)` collisions,
/// consumes the session, writes both Keychain rows, inserts the
/// `sources` row, and seeds the two `source_identities` rows.
/// Keychain writes come first so a mid-flight DB failure never
/// leaves an orphan `sources` row pointing at empty Keychain
/// slots; any post-Keychain failure rolls both back. Returns the
/// freshly-inserted [`Source`].
#[tauri::command]
pub async fn outlook_sources_add(
    session_id: OAuthSessionId,
    label: String,
    state: State<'_, AppState>,
) -> Result<Source, DayseamError> {
    outlook_sources_add_impl(&state, session_id, label).await
}

/// Test-visible implementation of [`outlook_sources_add`].
pub async fn outlook_sources_add_impl(
    state: &AppState,
    session_id: OAuthSessionId,
    label: String,
) -> Result<Source, DayseamError> {
    // ---- Peek + validate -------------------------------------------
    //
    // Peek first (non-consuming) so a validate-failure leaves the
    // session in the registry and the UI can surface "try again"
    // without forcing the user back through the browser.
    let pair = state
        .oauth_sessions
        .peek_token_pair(&session_id)
        .await
        .ok_or_else(|| DayseamError::InvalidConfig {
            code: error_codes::IPC_OUTLOOK_SESSION_NOT_READY.to_string(),
            message: format!(
                "OAuth session {session_id} is missing or has not produced a TokenPair yet"
            ),
        })?;

    let tenant_id = extract_tid(&pair.access_token)?;
    let provider = crate::oauth_config::lookup_provider(PROVIDER_MICROSOFT_OUTLOOK)?;

    // The probe `AuthDescriptor` uses a nil source id — the real
    // source id is only minted below, *after* the duplicate guard.
    // The descriptor is only used for probe-time validation; the
    // post-add persister uses the final source-id-based descriptor.
    let probe_descriptor = outlook_auth_descriptor(&tenant_id, &provider, SourceId::nil());
    let token_endpoint = token_endpoint_for_tenant(&tenant_id);
    let auth = build_probe_auth(
        &pair,
        probe_descriptor,
        token_endpoint.clone(),
        state.http.reqwest().clone(),
    )?;
    let base = graph_base_url()?;
    let info: OutlookUserInfo =
        validate_auth(&state.http, &auth, &base, &CancellationToken::new(), None).await?;

    // ---- Duplicate guard -------------------------------------------
    let source_repo = SourceRepo::new(state.pool.clone());
    if let Some(existing) = source_repo
        .find_by_outlook_identity(&tenant_id, &info.user_principal_name)
        .await
        .map_err(|e| DayseamError::Internal {
            code: "ipc.sources.find_by_outlook_identity".to_string(),
            message: e.to_string(),
        })?
    {
        return Err(DayseamError::InvalidConfig {
            code: error_codes::IPC_OUTLOOK_SOURCE_ALREADY_EXISTS.to_string(),
            message: format!(
                "an Outlook source for {} (tenant {}) already exists as `{}` ({}); \
                 reconnect it from Settings rather than adding a second row",
                info.user_principal_name, tenant_id, existing.label, existing.id
            ),
        });
    }

    // ---- Consume the session, mint a fresh source id ---------------
    //
    // Consuming here (rather than at the very start of the function)
    // is deliberate: an early `take_token_pair` followed by a
    // mid-flight failure (e.g. the duplicate guard fires) would
    // strand the user with tokens that have left process memory
    // forever. Taking the pair only after the last pre-write
    // validation passes keeps the retry story intact — everything
    // above this line is idempotent from the user's perspective.
    let pair = state
        .oauth_sessions
        .take_token_pair(&session_id)
        .await
        .ok_or_else(|| DayseamError::InvalidConfig {
            code: error_codes::IPC_OUTLOOK_SESSION_NOT_FOUND.to_string(),
            message: format!(
                "OAuth session {session_id} vanished between validate and commit; restart sign-in"
            ),
        })?;

    let source_id = Uuid::new_v4();
    let secret_ref = outlook_secret_ref(source_id);
    let access_account = outlook_access_account(source_id);
    let refresh_account = outlook_refresh_account(source_id);

    // ---- Keychain writes (access + refresh, both or neither) -------
    //
    // Track both flags through the rest of the function so any
    // failure path downstream can hand them straight to
    // [`rollback_outlook_sources_add`] without having to
    // re-inspect each write's outcome.
    if let Err(err) = oauth_secrets::put_access_token(
        state.secrets.as_ref(),
        OUTLOOK_KEYCHAIN_SERVICE,
        &access_account,
        Secret::new(pair.access_token.clone()),
    ) {
        return Err(DayseamError::Internal {
            code: error_codes::IPC_OUTLOOK_KEYCHAIN_WRITE_FAILED.to_string(),
            message: format!("keychain write for access token failed: {err}"),
        });
    }
    let wrote_access = true;

    if let Err(err) = oauth_secrets::put_refresh_token(
        state.secrets.as_ref(),
        OUTLOOK_KEYCHAIN_SERVICE,
        &refresh_account,
        Secret::new(pair.refresh_token.clone()),
    ) {
        rollback_outlook_keychain(
            state,
            wrote_access,
            false,
            &access_account,
            &refresh_account,
        );
        return Err(DayseamError::Internal {
            code: error_codes::IPC_OUTLOOK_KEYCHAIN_WRITE_FAILED.to_string(),
            message: format!("keychain write for refresh token failed: {err}"),
        });
    }
    let wrote_refresh = true;

    // ---- Person (self) + DB inserts --------------------------------
    let person_repo = PersonRepo::new(state.pool.clone());
    let identity_repo = SourceIdentityRepo::new(state.pool.clone());
    let mut inserted_source: Option<SourceId> = None;

    let self_person = match person_repo.bootstrap_self(SELF_DEFAULT_DISPLAY_NAME).await {
        Ok(p) => p,
        Err(e) => {
            rollback_outlook_sources_add(
                state,
                &source_repo,
                &inserted_source,
                wrote_access,
                wrote_refresh,
                &access_account,
                &refresh_account,
            )
            .await;
            return Err(DayseamError::Internal {
                code: "ipc.persons.bootstrap_self".to_string(),
                message: e.to_string(),
            });
        }
    };

    let trimmed_label = label.trim().to_string();
    let effective_label = if trimmed_label.is_empty() {
        // Fall back to the UPN so the sources sidebar always has
        // something human-readable — matches the GitHub
        // dialog's host fallback.
        format!("Outlook — {}", info.user_principal_name)
    } else {
        trimmed_label
    };

    let now = Utc::now();
    let source = Source {
        id: source_id,
        kind: SourceKind::Outlook,
        label: effective_label,
        config: SourceConfig::Outlook {
            tenant_id: tenant_id.clone(),
            user_principal_name: info.user_principal_name.clone(),
        },
        secret_ref: Some(secret_ref.clone()),
        created_at: now,
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };

    if let Err(e) = source_repo.insert(&source).await {
        rollback_outlook_sources_add(
            state,
            &source_repo,
            &inserted_source,
            wrote_access,
            wrote_refresh,
            &access_account,
            &refresh_account,
        )
        .await;
        return Err(DayseamError::Internal {
            code: "ipc.sources.insert".to_string(),
            message: format!("sources.insert(outlook) failed: {e}"),
        });
    }
    inserted_source = Some(source_id);

    // Seed the two self-identity rows the walker's self-filter
    // keys off (object id for the `organizer.user.id` shape, UPN
    // for the `organizer.emailAddress` shape). `list_identities`
    // is purely synchronous — the info we pass in is what Graph
    // already returned above.
    let info_with_tenant = OutlookUserInfo {
        tenant_id: tenant_id.clone(),
        ..info.clone()
    };
    let identities = match list_identities(&info_with_tenant, source_id, self_person.id, None) {
        Ok(v) => v,
        Err(e) => {
            rollback_outlook_sources_add(
                state,
                &source_repo,
                &inserted_source,
                wrote_access,
                wrote_refresh,
                &access_account,
                &refresh_account,
            )
            .await;
            return Err(e);
        }
    };
    for identity in identities {
        if let Err(e) = identity_repo.ensure(&identity).await {
            rollback_outlook_sources_add(
                state,
                &source_repo,
                &inserted_source,
                wrote_access,
                wrote_refresh,
                &access_account,
                &refresh_account,
            )
            .await;
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
        .ok_or_else(|| DayseamError::InvalidConfig {
            code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
            message: format!("source {source_id} disappeared immediately after insert"),
        })
}

/// Delete the Keychain rows a partial `outlook_sources_add` may have
/// left behind. Secondary failures are logged, not propagated — the
/// primary error is what the caller already returned, and the
/// DAY-81 orphan-secret audit is the safety net for a failed
/// delete. Kept synchronous because `SecretStore::delete` is
/// blocking; the hop is cheap enough to run on the current task
/// without a `spawn_blocking` dance.
fn rollback_outlook_keychain(
    state: &AppState,
    wrote_access: bool,
    wrote_refresh: bool,
    access_account: &str,
    refresh_account: &str,
) {
    if wrote_access {
        if let Err(e) = oauth_secrets::delete_access_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            access_account,
        ) {
            tracing::warn!(
                error = %e,
                service = OUTLOOK_KEYCHAIN_SERVICE,
                account = access_account,
                "outlook rollback: access-token keychain delete failed; row may linger",
            );
        }
    }
    if wrote_refresh {
        if let Err(e) = oauth_secrets::delete_refresh_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            refresh_account,
        ) {
            tracing::warn!(
                error = %e,
                service = OUTLOOK_KEYCHAIN_SERVICE,
                account = refresh_account,
                "outlook rollback: refresh-token keychain delete failed; row may linger",
            );
        }
    }
}

/// Unwind a partially-committed [`outlook_sources_add`]. Deletes any
/// inserted `sources` row first (so the identity rows go with it via
/// the foreign-key cascade) and then the Keychain rows. Logs but
/// does not propagate secondary errors — matches the GitHub rollback
/// contract.
async fn rollback_outlook_sources_add(
    state: &AppState,
    source_repo: &SourceRepo,
    inserted_source: &Option<SourceId>,
    wrote_access: bool,
    wrote_refresh: bool,
    access_account: &str,
    refresh_account: &str,
) {
    if let Some(id) = inserted_source {
        if let Err(e) = source_repo.delete(id).await {
            tracing::warn!(
                error = %e,
                source_id = %id,
                "outlook rollback: sources.delete failed; row may linger",
            );
        }
    }
    rollback_outlook_keychain(
        state,
        wrote_access,
        wrote_refresh,
        access_account,
        refresh_account,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use chrono::Duration as ChronoDuration;
    use connectors_sdk::TokenPair;
    use dayseam_core::OAuthSessionStatus;
    use dayseam_db::open;
    use dayseam_events::AppBus;
    use dayseam_orchestrator::{ConnectorRegistry, OrchestratorBuilder, SinkRegistry};
    use dayseam_secrets::InMemoryStore;
    use std::sync::Arc;
    use tempfile::TempDir;

    use crate::oauth_session::OAuthSession;

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

    fn make_jwt(payload_json: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(b"not-verified");
        format!("{header}.{payload}.{signature}")
    }

    async fn seed_session_with_pair(state: &AppState, access_token: &str) -> OAuthSessionId {
        let id = OAuthSessionId::new();
        let session = OAuthSession {
            id,
            provider_id: PROVIDER_MICROSOFT_OUTLOOK.to_string(),
            created_at: Utc::now(),
            status: OAuthSessionStatus::Completed,
            token_pair: None,
            cancel: CancellationToken::new(),
        };
        state.oauth_sessions.insert(session).await;
        let pair = TokenPair::new(
            access_token.to_string(),
            "refresh-token".to_string(),
            Utc::now() + ChronoDuration::hours(1),
            vec![
                "offline_access".to_string(),
                "Calendars.Read".to_string(),
                "User.Read".to_string(),
            ],
        );
        state.oauth_sessions.set_token_pair(&id, pair).await;
        id
    }

    #[test]
    fn outlook_secret_ref_is_stable_per_source() {
        let id = Uuid::new_v4();
        let a = outlook_secret_ref(id);
        let b = outlook_secret_ref(id);
        assert_eq!(a, b);
        assert_eq!(a.keychain_service, OUTLOOK_KEYCHAIN_SERVICE);
        assert_eq!(a.keychain_account, format!("source:{id}.oauth.access"));
    }

    #[test]
    fn outlook_access_and_refresh_accounts_diverge() {
        let id = Uuid::new_v4();
        let access = outlook_access_account(id);
        let refresh = outlook_refresh_account(id);
        assert_ne!(access, refresh);
        assert!(access.ends_with(".oauth.access"));
        assert!(refresh.ends_with(".oauth.refresh"));
        assert!(access.starts_with(&format!("source:{id}")));
        assert!(refresh.starts_with(&format!("source:{id}")));
    }

    #[test]
    fn token_endpoint_interpolates_tenant_guid() {
        let endpoint = token_endpoint_for_tenant("11111111-2222-3333-4444-555555555555");
        assert_eq!(
            endpoint,
            "https://login.microsoftonline.com/11111111-2222-3333-4444-555555555555/oauth2/v2.0/token"
        );
    }

    #[tokio::test]
    async fn validate_rejects_missing_session() {
        let (state, _dir) = make_state().await;
        let err = outlook_validate_credentials_impl(&state, OAuthSessionId::new())
            .await
            .expect_err("missing session must reject");
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_SESSION_NOT_READY);
    }

    #[tokio::test]
    async fn validate_rejects_session_without_token_pair() {
        let (state, _dir) = make_state().await;
        let id = OAuthSessionId::new();
        let session = OAuthSession {
            id,
            provider_id: PROVIDER_MICROSOFT_OUTLOOK.to_string(),
            created_at: Utc::now(),
            status: OAuthSessionStatus::Pending,
            token_pair: None,
            cancel: CancellationToken::new(),
        };
        state.oauth_sessions.insert(session).await;

        let err = outlook_validate_credentials_impl(&state, id)
            .await
            .expect_err("session with no tokens yet must reject");
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_SESSION_NOT_READY);
    }

    #[tokio::test]
    async fn validate_rejects_access_token_without_tid() {
        let (state, _dir) = make_state().await;
        let token = make_jwt(r#"{"sub":"only-sub"}"#);
        let id = seed_session_with_pair(&state, &token).await;

        let err = outlook_validate_credentials_impl(&state, id)
            .await
            .expect_err("tokens with no tid must reject");
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);
    }

    #[tokio::test]
    async fn add_rejects_missing_session() {
        let (state, _dir) = make_state().await;
        let err = outlook_sources_add_impl(&state, OAuthSessionId::new(), "lbl".into())
            .await
            .expect_err("missing session must reject before any side effect");
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_SESSION_NOT_READY);
    }

    #[tokio::test]
    async fn add_rejects_bad_jwt_before_network() {
        let (state, _dir) = make_state().await;
        let id = seed_session_with_pair(&state, "not-a-jwt").await;
        let err = outlook_sources_add_impl(&state, id, "lbl".into())
            .await
            .expect_err("bad JWT must reject at tid extraction");
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);

        // Session should still be addressable — validate is
        // non-consuming, so the UI can try again.
        assert!(state.oauth_sessions.peek_token_pair(&id).await.is_some());
    }

    #[tokio::test]
    async fn rollback_keychain_deletes_written_rows() {
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        let access_account = outlook_access_account(source_id);
        let refresh_account = outlook_refresh_account(source_id);

        oauth_secrets::put_access_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            &access_account,
            Secret::new("access".into()),
        )
        .unwrap();
        oauth_secrets::put_refresh_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            &refresh_account,
            Secret::new("refresh".into()),
        )
        .unwrap();

        rollback_outlook_keychain(&state, true, true, &access_account, &refresh_account);

        assert!(oauth_secrets::get_access_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            &access_account,
        )
        .unwrap()
        .is_none());
        assert!(oauth_secrets::get_refresh_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            &refresh_account,
        )
        .unwrap()
        .is_none());
    }

    #[tokio::test]
    async fn rollback_keychain_honours_write_flags() {
        let (state, _dir) = make_state().await;
        let source_id = Uuid::new_v4();
        let access_account = outlook_access_account(source_id);
        let refresh_account = outlook_refresh_account(source_id);

        oauth_secrets::put_access_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            &access_account,
            Secret::new("access".into()),
        )
        .unwrap();

        // wrote_refresh=false => the refresh row was never written,
        // so rollback must not reach it. The access row *was*
        // written and should disappear.
        rollback_outlook_keychain(&state, true, false, &access_account, &refresh_account);

        assert!(oauth_secrets::get_access_token(
            state.secrets.as_ref(),
            OUTLOOK_KEYCHAIN_SERVICE,
            &access_account,
        )
        .unwrap()
        .is_none());
    }
}
