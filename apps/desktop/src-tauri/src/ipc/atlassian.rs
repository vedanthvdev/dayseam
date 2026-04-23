//! Atlassian IPC surface (DAY-82).
//!
//! Two commands live here:
//!
//!   1. [`atlassian_validate_credentials`] — a one-shot
//!      `GET /rest/api/3/myself` probe. The `AddAtlassianSourceDialog`
//!      calls it once the user has pasted an email + API token and a
//!      workspace URL so the dialog can show "Connected as …" before
//!      committing to a `sources_add`.
//!   2. [`atlassian_sources_add`] — the transactional persist call
//!      that covers all four Add-Atlassian journeys (shared-PAT,
//!      single-product, reuse-existing-PAT, different-PAT) documented
//!      in the plan at `docs/plan/2026-04-20-v0.2-atlassian.md`. Every
//!      row it writes points at a [`SecretRef`] either freshly minted
//!      here (one new keychain row) or handed in by the caller
//!      (Journey C mode 1, zero new keychain rows). DAY-81's refcount
//!      guard handles clean-up on delete — the shared/not-shared
//!      distinction lives entirely in how many `Source` rows end up
//!      holding a given `secret_ref`.
//!
//! This module does **not** build an auth strategy for the persisted
//! sources the way `commands::build_source_auth` does for GitLab: the
//! Jira and Confluence arms of that function still return
//! `Unsupported` until DAY-84 wires the real walkers. DAY-82's IPC
//! exists so the dialog can drive DAY-81's secret management end-to-
//! end; the walkers that actually *use* those secrets are the next
//! ticket's problem.
//!
//! The keychain keying scheme is deliberate:
//!
//! | GitLab (DAY-70)                    | Atlassian (this file)                    |
//! |------------------------------------|------------------------------------------|
//! | `service = dayseam.gitlab`         | `service = dayseam.atlassian`            |
//! | `account = source:<source_id>`     | `account = slot:<uuid>`                  |
//!
//! GitLab keys by `source_id` because one GitLab source owns exactly
//! one PAT. Atlassian keys by an opaque UUID slot because the
//! shared-PAT flow needs two `Source` rows to address the *same*
//! keychain entry, and `source_id` cannot do that by construction.
//! The slot UUID is independent of either `source_id` so a later
//! rename or reshape (e.g. splitting a shared PAT by rotating one of
//! the two products' credentials) does not require re-keying the
//! keychain.

use chrono::Utc;
use connector_atlassian_common::{
    cloud::{discover_cloud, AtlassianAccountInfo},
    identity::seed_atlassian_identity,
};
use connectors_sdk::BasicAuth;
use dayseam_core::{
    error_codes, AtlassianValidationResult, DayseamError, SecretRef, Source, SourceConfig,
    SourceHealth, SourceId, SourceIdentityKind, SourceKind,
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

/// Keychain `service` half for every Atlassian API token this app
/// stores. Matches the "Keychain Access readability" rationale that
/// all `dayseam.atlassian` entries live under one heading and is
/// the shape DAY-81's orphan-secret audit on boot walks.
const ATLASSIAN_KEYCHAIN_SERVICE: &str = "dayseam.atlassian";

/// Build a fresh [`SecretRef`] for an Atlassian PAT. Unlike the
/// GitLab variant, the `account` half is a brand-new UUID rather
/// than derived from any one `SourceId` — two sources may point at
/// the same keychain row (shared-PAT mode) and neither one is
/// "canonical".
fn new_atlassian_secret_ref() -> SecretRef {
    SecretRef {
        keychain_service: ATLASSIAN_KEYCHAIN_SERVICE.to_string(),
        keychain_account: format!("slot:{}", Uuid::new_v4()),
    }
}

/// Parse a caller-supplied `workspace_url` into an absolute
/// `https://<host>` URL (no path, no query, no trailing slash). The
/// dialog normalises client-side first (see the TS
/// `atlassian-workspace-url.ts` helper); this server-side check is a
/// defence-in-depth so a bespoke caller cannot round-trip a malformed
/// URL into `SourceConfig::{Jira, Confluence}`.
fn parse_workspace_url(input: &str) -> Result<Url, DayseamError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
            "workspace_url must not be empty",
        ));
    }
    // `Url::parse` refuses scheme-less input, which is exactly what
    // we want here: the dialog adds `https://` client-side, so any
    // input that still lacks a scheme is a caller bug.
    let parsed = Url::parse(trimmed).map_err(|e| {
        invalid_config_public(
            error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
            format!("workspace_url `{trimmed}` is not a valid URL: {e}"),
        )
    })?;
    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
            "workspace_url has no host component",
        ));
    }
    let host_lower = host.to_ascii_lowercase();
    // DAY-111 / TST-v0.4-04. Carve out a loopback escape hatch *only*
    // under the `test-helpers` seam (or the crate's own
    // `#[cfg(test)]` tests): a `http://127.0.0.1:PORT` workspace URL
    // is the shape wiremock hands out, and nothing in production
    // ever parses one. Both scheme and host are checked together so
    // the existing DOG-v0.2-03 guard still rejects
    // `http://modulrfinance.atlassian.net` (downgrade over cleartext)
    // and `https://attacker.example` (wrong tenant) in every build —
    // the loopback carve-out is the *only* relaxation.
    let is_test_loopback = workspace_host_is_test_loopback(&host_lower);
    if parsed.scheme() != "https" && !(is_test_loopback && parsed.scheme() == "http") {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
            format!(
                "workspace_url scheme must be `https`; got `{}`",
                parsed.scheme()
            ),
        ));
    }
    // DOG-v0.2-03 (security). Reject any host that is not under
    // Atlassian Cloud's `.atlassian.net` apex. Without this guard a
    // user (or a hostile clipboard contents) could persist
    // `https://attacker.example/` and the next BasicAuth handshake
    // would ship the API token to that origin. Lower-casing first so
    // case-mangled hosts (`Acme.Atlassian.NET`) are evaluated against
    // the same apex; `url::Url` already lower-cases on parse, but
    // belt-and-braces here keeps the rule self-contained.
    let host_ok = host_lower == "atlassian.net" || host_lower.ends_with(".atlassian.net");
    if !host_ok && !is_test_loopback {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
            format!(
                "workspace_url host must be a `*.atlassian.net` Atlassian Cloud tenant; got `{host}`"
            ),
        ));
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
            format!(
                "workspace_url must be origin-only; got path `{}`",
                parsed.path()
            ),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
            "workspace_url must not carry a query string or fragment",
        ));
    }
    Ok(parsed)
}

/// Canonicalise a parsed `Url` to the string shape
/// `SourceConfig::{Jira, Confluence}.workspace_url` stores: scheme +
/// host (+ port) with no trailing slash and no path.
fn canonical_workspace_url(url: &Url) -> String {
    let mut s = url.origin().ascii_serialization();
    while s.ends_with('/') {
        s.pop();
    }
    s
}

/// DAY-111 / TST-v0.4-04. Let `127.0.0.1` / `localhost` through
/// only under the `test-helpers` seam. The `*.atlassian.net` apex
/// guard (DOG-v0.2-03) stays in place for production callers so a
/// hostile clipboard cannot persist an attacker-controlled origin;
/// the test seam carves out a narrow hole for the integration
/// fixture that fronts `GET /rest/api/3/myself` on a loopback mock.
fn workspace_host_is_test_loopback(_host_lower: &str) -> bool {
    #[cfg(any(test, feature = "test-helpers"))]
    {
        if _host_lower == "127.0.0.1" || _host_lower == "localhost" {
            return true;
        }
    }
    false
}

fn require_nonempty_email(email: &str) -> Result<(), DayseamError> {
    if email.trim().is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING,
            "email must not be empty or whitespace-only",
        ));
    }
    Ok(())
}

fn require_nonempty_token(token: &str) -> Result<(), DayseamError> {
    if token.trim().is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING,
            "api_token must not be empty or whitespace-only",
        ));
    }
    Ok(())
}

/// Build a minimal [`AtlassianAccountInfo`] wrapper around a bare
/// `account_id`. `seed_atlassian_identity` only reads the id field —
/// display_name / email are placeholders here because the dialog
/// already used them at validate time for the "Connected as …"
/// ribbon.
fn account_info_from_id(account_id: &str) -> AtlassianAccountInfo {
    AtlassianAccountInfo {
        account_id: account_id.to_string(),
        display_name: String::new(),
        email: None,
        cloud_id: None,
    }
}

/// One-shot `GET /rest/api/3/myself` probe. Returns the account
/// triple the dialog renders in its "Connected as …" ribbon. See the
/// [`crate::ipc::atlassian`] module docs for the failure modes.
#[tauri::command]
pub async fn atlassian_validate_credentials(
    workspace_url: String,
    email: String,
    api_token: IpcSecretString,
    state: State<'_, AppState>,
) -> Result<AtlassianValidationResult, DayseamError> {
    atlassian_validate_credentials_impl(&state, workspace_url, email, api_token).await
}

/// Test-visible implementation of [`atlassian_validate_credentials`].
/// Same shape minus the Tauri [`State`] wrapper, which cannot be
/// constructed outside the Tauri runtime. DAY-111 moves the HTTP
/// client construction out of this function (onto `AppState::http`)
/// so `tests/reconnect_rebind.rs` can exercise the full probe path
/// against a wiremock-backed client without going through a live
/// network call.
pub async fn atlassian_validate_credentials_impl(
    state: &AppState,
    workspace_url: String,
    email: String,
    api_token: IpcSecretString,
) -> Result<AtlassianValidationResult, DayseamError> {
    require_nonempty_email(&email)?;
    require_nonempty_token(api_token.expose())?;
    let parsed = parse_workspace_url(&workspace_url)?;

    // The `BasicAuth` constructed here lives only for the duration
    // of this call — the descriptor's keychain handle is a synthetic
    // `"probe"` slot because no keychain row actually backs this
    // transient credential. The probe path never hydrates a
    // `SecretRef` from the descriptor, so the placeholder is invisible
    // to everything downstream.
    let auth = BasicAuth::atlassian(
        email.as_str(),
        api_token.expose(),
        ATLASSIAN_KEYCHAIN_SERVICE,
        "probe",
    );
    let cloud =
        discover_cloud(&state.http, &auth, &parsed, &CancellationToken::new(), None).await?;

    Ok(AtlassianValidationResult {
        account_id: cloud.account.account_id,
        display_name: cloud.account.display_name,
        email: cloud.account.email,
    })
}

/// Unwind the partial work a failing [`atlassian_sources_add`] left
/// behind. Called at every failure point after the first keychain
/// write / row insert. We log but do not propagate secondary errors
/// — the primary failure the caller returns is the one that matters;
/// a keychain row we failed to delete is picked up by the boot-time
/// orphan audit (DAY-81).
async fn rollback_sources_add(
    state: &AppState,
    source_repo: &SourceRepo,
    inserted: &[SourceId],
    wrote_new_secret: bool,
    secret_ref: &SecretRef,
) {
    for id in inserted {
        if let Err(e) = source_repo.delete(id).await {
            tracing::warn!(
                error = %e,
                source_id = %id,
                "atlassian rollback: sources.delete failed; row may linger",
            );
        }
    }
    if wrote_new_secret {
        let key = secret_store_key(secret_ref);
        if let Err(e) = state.secrets.delete(&key) {
            tracing::warn!(
                error = %e,
                %key,
                "atlassian rollback: keychain delete failed; row may linger",
            );
        }
    }
}

/// Persist one or two Atlassian `Source` rows. See the module docs
/// for the four journeys this single command implements and the
/// keychain-write invariants each enforces.
///
/// This is the thin Tauri wrapper — the real work lives in
/// [`atlassian_sources_add_impl`] so integration tests can drive the
/// same logic without building a [`State`].
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn atlassian_sources_add(
    workspace_url: String,
    email: String,
    api_token: Option<IpcSecretString>,
    account_id: String,
    enable_jira: bool,
    enable_confluence: bool,
    reuse_secret_ref: Option<SecretRef>,
    state: State<'_, AppState>,
) -> Result<Vec<Source>, DayseamError> {
    atlassian_sources_add_impl(
        &state,
        workspace_url,
        email,
        api_token,
        account_id,
        enable_jira,
        enable_confluence,
        reuse_secret_ref,
    )
    .await
}

/// Test-visible implementation of [`atlassian_sources_add`]. Same
/// shape minus the Tauri [`State`] wrapper, which cannot be
/// constructed outside the Tauri runtime.
#[allow(clippy::too_many_arguments)]
pub async fn atlassian_sources_add_impl(
    state: &AppState,
    workspace_url: String,
    email: String,
    api_token: Option<IpcSecretString>,
    account_id: String,
    enable_jira: bool,
    enable_confluence: bool,
    reuse_secret_ref: Option<SecretRef>,
) -> Result<Vec<Source>, DayseamError> {
    // ---- Structural validation -------------------------------------
    if !enable_jira && !enable_confluence {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_NO_PRODUCT_SELECTED,
            "at least one of enable_jira / enable_confluence must be true",
        ));
    }
    require_nonempty_email(&email)?;
    if account_id.trim().is_empty() {
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING,
            "account_id must not be empty; call atlassian_validate_credentials first",
        ));
    }
    let parsed_url = parse_workspace_url(&workspace_url)?;
    let canonical_url = canonical_workspace_url(&parsed_url);

    // ---- Keychain: resolve `SecretRef` -----------------------------
    // Two paths:
    //
    //   * `reuse_secret_ref = Some(_)`  → the caller is adding a
    //     product alongside one that already exists and wants the
    //     two to share a keychain row. Verify the slot is populated
    //     (so a stale dialog state cannot write a source row that
    //     points at an empty slot) and skip the token-required check.
    //   * `reuse_secret_ref = None`     → Journeys A / B / C-mode-2.
    //     `api_token` must be present and non-empty; we write a fresh
    //     keychain row keyed by a new UUID slot.
    //
    // The split happens *before* the DB write so a failure here
    // never leaves a half-created source behind.
    let (secret_ref, wrote_new_secret) = match reuse_secret_ref {
        Some(ref existing) => {
            let key = secret_store_key(existing);
            let present = state
                .secrets
                .get(&key)
                .map_err(|e| DayseamError::Internal {
                    code: error_codes::IPC_ATLASSIAN_KEYCHAIN_WRITE_FAILED.to_string(),
                    message: format!("keychain probe for {key} failed: {e}"),
                })?;
            if present.is_none() {
                return Err(invalid_config_public(
                    error_codes::IPC_ATLASSIAN_REUSE_SECRET_MISSING,
                    format!(
                        "reuse_secret_ref `{key}` is empty in the keychain; the owning source was \
                         likely deleted. Re-open the dialog and paste a fresh token instead."
                    ),
                ));
            }
            (existing.clone(), false)
        }
        None => {
            let token = api_token.as_ref().ok_or_else(|| {
                invalid_config_public(
                    error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING,
                    "api_token must be provided when reuse_secret_ref is null",
                )
            })?;
            require_nonempty_token(token.expose())?;
            let sr = new_atlassian_secret_ref();
            let key = secret_store_key(&sr);
            state
                .secrets
                .put(&key, Secret::new(token.expose().to_string()))
                .map_err(|e| DayseamError::Internal {
                    code: error_codes::IPC_ATLASSIAN_KEYCHAIN_WRITE_FAILED.to_string(),
                    message: format!("keychain write for {key} failed: {e}"),
                })?;
            (sr, true)
        }
    };

    let source_repo = SourceRepo::new(state.pool.clone());
    let identity_repo = SourceIdentityRepo::new(state.pool.clone());
    let mut inserted: Vec<SourceId> = Vec::new();

    // ---- Person (self) for identity seeding ------------------------
    let self_person = match PersonRepo::new(state.pool.clone())
        .bootstrap_self(SELF_DEFAULT_DISPLAY_NAME)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            rollback_sources_add(
                state,
                &source_repo,
                &inserted,
                wrote_new_secret,
                &secret_ref,
            )
            .await;
            return Err(DayseamError::Internal {
                code: "ipc.persons.bootstrap_self".to_string(),
                message: e.to_string(),
            });
        }
    };

    // ---- Insert each enabled product row ---------------------------
    let now = Utc::now();

    if enable_jira {
        let source_id = Uuid::new_v4();
        let source = Source {
            id: source_id,
            kind: SourceKind::Jira,
            label: format!(
                "Jira — {}",
                parsed_url.host_str().unwrap_or(canonical_url.as_str())
            ),
            config: SourceConfig::Jira {
                workspace_url: canonical_url.clone(),
                email: email.clone(),
            },
            secret_ref: Some(secret_ref.clone()),
            created_at: now,
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        if let Err(e) = source_repo.insert(&source).await {
            rollback_sources_add(
                state,
                &source_repo,
                &inserted,
                wrote_new_secret,
                &secret_ref,
            )
            .await;
            return Err(DayseamError::Internal {
                code: "ipc.sources.insert".to_string(),
                message: format!("sources.insert(jira) failed: {e}"),
            });
        }
        inserted.push(source_id);

        let info = account_info_from_id(&account_id);
        let identity = match seed_atlassian_identity(&info, source_id, self_person.id, None) {
            Ok(i) => i,
            Err(e) => {
                rollback_sources_add(
                    state,
                    &source_repo,
                    &inserted,
                    wrote_new_secret,
                    &secret_ref,
                )
                .await;
                return Err(e);
            }
        };
        if let Err(e) = identity_repo.ensure(&identity).await {
            rollback_sources_add(
                state,
                &source_repo,
                &inserted,
                wrote_new_secret,
                &secret_ref,
            )
            .await;
            return Err(DayseamError::Internal {
                code: "ipc.source_identities.ensure".to_string(),
                message: e.to_string(),
            });
        }
    }

    if enable_confluence {
        let source_id = Uuid::new_v4();
        let source = Source {
            id: source_id,
            kind: SourceKind::Confluence,
            label: format!(
                "Confluence — {}",
                parsed_url.host_str().unwrap_or(canonical_url.as_str())
            ),
            config: SourceConfig::Confluence {
                workspace_url: canonical_url.clone(),
                email: email.clone(),
            },
            secret_ref: Some(secret_ref.clone()),
            created_at: now,
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        if let Err(e) = source_repo.insert(&source).await {
            rollback_sources_add(
                state,
                &source_repo,
                &inserted,
                wrote_new_secret,
                &secret_ref,
            )
            .await;
            return Err(DayseamError::Internal {
                code: "ipc.sources.insert".to_string(),
                message: format!("sources.insert(confluence) failed: {e}"),
            });
        }
        inserted.push(source_id);

        let info = account_info_from_id(&account_id);
        let identity = match seed_atlassian_identity(&info, source_id, self_person.id, None) {
            Ok(i) => i,
            Err(e) => {
                rollback_sources_add(
                    state,
                    &source_repo,
                    &inserted,
                    wrote_new_secret,
                    &secret_ref,
                )
                .await;
                return Err(e);
            }
        };
        if let Err(e) = identity_repo.ensure(&identity).await {
            rollback_sources_add(
                state,
                &source_repo,
                &inserted,
                wrote_new_secret,
                &secret_ref,
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

    let mut rows = Vec::with_capacity(inserted.len());
    for id in &inserted {
        match source_repo.get(id).await {
            Ok(Some(src)) => rows.push(src),
            Ok(None) => {
                return Err(invalid_config_public(
                    error_codes::IPC_SOURCE_NOT_FOUND,
                    format!("source {id} disappeared immediately after insert"),
                ));
            }
            Err(e) => {
                return Err(DayseamError::Internal {
                    code: "ipc.sources.get".to_string(),
                    message: e.to_string(),
                });
            }
        }
    }

    Ok(rows)
}

/// Rotate the API token on an existing Atlassian source (DAY-87).
///
/// The Reconnect chip on `SourceErrorCard` fires this command when a
/// Jira or Confluence source's last walk failed with
/// `atlassian.auth.invalid_credentials` (or any other code the chip's
/// copy table maps to the `reconnect` action). The flow is:
///
///   1. Load the source by id. Must exist and be Atlassian-kind.
///   2. Extract its `(workspace_url, email)` from `SourceConfig` —
///      the reconnect dialog renders these as read-only context, so
///      the tokens we validate against here are always the ones
///      already bound to the source's identity.
///   3. Validate the new token against that `(workspace_url, email)`
///      via the same `discover_cloud` probe `atlassian_validate_
///      credentials` uses. A `401` comes back as
///      `atlassian.auth.invalid_credentials`, which the dialog
///      surfaces inline instead of persisting anything.
///   4. Assert the validated `account_id` still matches the source's
///      existing `AtlassianAccountId` `SourceIdentity`. A token that
///      happens to be valid for a *different* Atlassian account must
///      not silently rebind the source — that would make every event
///      emitted afterwards a cross-actor rollup bug. Mismatch returns
///      `atlassian.auth.invalid_credentials` with an explicit
///      "wrong account" message so the Reconnect copy can explain
///      why the token was rejected despite being otherwise valid.
///   5. Overwrite the keychain entry at the existing `SecretRef`.
///      Because shared-PAT sources (Journey A from `atlassian_
///      sources_add`) point two rows at the same `SecretRef`, a
///      single rotation here fixes both siblings atomically — that
///      is, precisely the desired UX for a shared-token reconnect.
///
/// Returns the ids of every `Source` whose token was rotated by this
/// call: `[self]` for single-product sources, `[self, sibling]` for
/// shared-PAT pairs. The frontend uses this list to fire
/// `sources_healthcheck` on each so the red error chips clear
/// immediately rather than waiting for the next scheduled walk.
///
/// **Not changed** by this command: `workspace_url`, `email`, and
/// the source's `SourceIdentity` rows. A user who needs to change
/// those should delete + re-add — the identity binding is part of
/// the source's contract, not a rotation concern.
#[tauri::command]
pub async fn atlassian_sources_reconnect(
    source_id: SourceId,
    api_token: IpcSecretString,
    state: State<'_, AppState>,
) -> Result<Vec<SourceId>, DayseamError> {
    atlassian_sources_reconnect_impl(&state, source_id, api_token).await
}

/// Test-visible implementation of [`atlassian_sources_reconnect`].
/// Same shape minus the Tauri [`State`] wrapper, which cannot be
/// constructed outside the Tauri runtime.
pub async fn atlassian_sources_reconnect_impl(
    state: &AppState,
    source_id: SourceId,
    api_token: IpcSecretString,
) -> Result<Vec<SourceId>, DayseamError> {
    require_nonempty_token(api_token.expose())?;

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

    let (workspace_url, email) = match &source.config {
        SourceConfig::Jira {
            workspace_url,
            email,
        } => (workspace_url.clone(), email.clone()),
        SourceConfig::Confluence {
            workspace_url,
            email,
        } => (workspace_url.clone(), email.clone()),
        // `LocalGit` / `GitLab` / anything-else has its own reconnect
        // path — routing one of those here is a frontend bug, so
        // fail loud with the shared kind-mismatch code rather than
        // silently falling back to a healthcheck.
        _ => {
            return Err(invalid_config_public(
                error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH,
                format!(
                    "source {source_id} is {:?}; atlassian_sources_reconnect is Jira/Confluence only",
                    source.kind
                ),
            ));
        }
    };

    // A secret_ref-less Atlassian source is an upgrade-path artifact
    // (pre-DAY-81 rows, or rows whose keychain was wiped out from
    // under the app). The reconnect flow cannot rotate a token that
    // has nowhere to land, so route the user to delete-and-re-add
    // instead of writing a brand-new `SecretRef` here and silently
    // forking the "add" codepath.
    let secret_ref = source.secret_ref.clone().ok_or_else(|| {
        invalid_config_public(
            error_codes::IPC_ATLASSIAN_REUSE_SECRET_MISSING,
            format!(
                "source {source_id} has no secret_ref on file; delete the source and add it again rather than reconnecting"
            ),
        )
    })?;

    if email.trim().is_empty() {
        // The v0.2 `#[serde(default)]` backfill (see DOG-v0.2-04)
        // could leave a Confluence row with an empty email when no
        // Jira sibling existed to backfill from. Reconnect cannot
        // /myself against an empty email, so surface the missing
        // piece explicitly rather than letting the upstream 400
        // bubble up as a shape-changed error.
        return Err(invalid_config_public(
            error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING,
            format!(
                "source {source_id} has no email on file; delete the source and add it again so the dialog can capture one"
            ),
        ));
    }

    // ---- 2. Validate new token against the bound account ----------
    //
    // DAY-111 pulls the [`HttpClient`] from `state.http` rather than
    // minting a fresh one per call — the same client the walker uses,
    // so keep-alive survives a validate→reconnect round-trip on the
    // same host and `tests/reconnect_rebind.rs` can route this probe
    // through a wiremock server.
    let parsed_url = parse_workspace_url(&workspace_url)?;
    let auth = BasicAuth::atlassian(
        email.as_str(),
        api_token.expose(),
        ATLASSIAN_KEYCHAIN_SERVICE,
        "probe",
    );
    let cloud = discover_cloud(
        &state.http,
        &auth,
        &parsed_url,
        &CancellationToken::new(),
        None,
    )
    .await?;
    let new_account_id = cloud.account.account_id;

    // ---- 3. Identity-binding invariant ----------------------------
    // The source row's bound AtlassianAccountId identity is written
    // by `atlassian_sources_add` and must not be silently rebound by
    // a reconnect. If the user pastes a token that is valid for a
    // *different* account (common: they copied their personal token
    // instead of their work token), fail rather than rotate — the
    // next report would cross-contaminate the self-filter and
    // attribute the new actor's commits to the old one.
    // The reconnect flow only cares about identities bound to the
    // *self* person — that's the set `atlassian_sources_add` seeded
    // on initial connect. `list_for_source` requires a `person_id`,
    // so resolve it via `bootstrap_self` the same way `sources_add`
    // does; this is idempotent and matches the row we wrote earlier.
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
    let bound_account_id = identities
        .iter()
        .find(|id| matches!(id.kind, SourceIdentityKind::AtlassianAccountId))
        .map(|id| id.external_actor_id.clone());
    if let Some(existing) = bound_account_id {
        if existing != new_account_id {
            return Err(DayseamError::Auth {
                code: error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS.to_string(),
                message: format!(
                    "token is valid but for account `{new_account_id}`; this source is bound to `{existing}`. Paste a token for the original account, or delete this source and add a new one."
                ),
                retryable: false,
                action_hint: Some("reconnect".to_string()),
            });
        }
    }
    // Zero-identity-rows case: the source pre-dates DAY-82's identity
    // seeding. Treat reconnect as a self-healing path — rotate the
    // token and let the existing sync pipeline seed the identity on
    // its next run. No mismatch check fires because there's nothing
    // to mismatch against.

    // ---- 4. Rotate the keychain slot ------------------------------
    let key = secret_store_key(&secret_ref);
    state
        .secrets
        .put(&key, Secret::new(api_token.expose().to_string()))
        .map_err(|e| DayseamError::Internal {
            code: error_codes::IPC_ATLASSIAN_KEYCHAIN_WRITE_FAILED.to_string(),
            message: format!("keychain write for {key} failed: {e}"),
        })?;

    // ---- 5. Enumerate siblings sharing the rotated `SecretRef` ----
    // Two Atlassian rows can point at the same keychain slot in
    // Journey A (shared-PAT). The reconnect UI wants to clear both
    // chips' red state on success, so return both ids; the frontend
    // fires `sources_healthcheck` for each.
    let all = source_repo
        .list()
        .await
        .map_err(|e| DayseamError::Internal {
            code: "ipc.sources.list".to_string(),
            message: e.to_string(),
        })?;
    let affected: Vec<SourceId> = all
        .into_iter()
        .filter(|s| s.secret_ref.as_ref() == Some(&secret_ref))
        .map(|s| s.id)
        .collect();

    Ok(affected)
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

    fn token() -> IpcSecretString {
        IpcSecretString::new("atlassian-pat-token")
    }

    #[test]
    fn parse_workspace_url_accepts_canonical_form() {
        let url = parse_workspace_url("https://modulrfinance.atlassian.net").unwrap();
        assert_eq!(
            canonical_workspace_url(&url),
            "https://modulrfinance.atlassian.net"
        );
    }

    #[test]
    fn parse_workspace_url_strips_trailing_slash() {
        let url = parse_workspace_url("https://modulrfinance.atlassian.net/").unwrap();
        assert_eq!(
            canonical_workspace_url(&url),
            "https://modulrfinance.atlassian.net"
        );
    }

    #[test]
    fn parse_workspace_url_rejects_non_https() {
        let err = parse_workspace_url("http://modulrfinance.atlassian.net").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL);
    }

    #[test]
    fn parse_workspace_url_rejects_path_segments() {
        let err = parse_workspace_url("https://modulrfinance.atlassian.net/wiki").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL);
    }

    #[test]
    fn parse_workspace_url_rejects_empty() {
        let err = parse_workspace_url("   ").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL);
    }

    #[test]
    fn parse_workspace_url_rejects_scheme_missing() {
        // URLs without a scheme fail `Url::parse` with "relative URL without a base".
        let err = parse_workspace_url("modulrfinance.atlassian.net").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL);
    }

    #[test]
    fn parse_workspace_url_rejects_non_atlassian_host() {
        // DOG-v0.2-03 (security). A hostile or typo-squatted host
        // must not survive the IPC even if the rest of the URL shape
        // looks valid; otherwise the next `BasicAuth` request would
        // POST the user's API token at `attacker.example`.
        for bad in [
            "https://attacker.example/",
            "https://acme.atlassian.net.attacker.example/",
            "https://atlassian.net.attacker.example/",
            "https://acme.example.com/",
        ] {
            let err = parse_workspace_url(bad).unwrap_err();
            assert_eq!(
                err.code(),
                error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL,
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn parse_workspace_url_accepts_atlassian_apex_and_subdomains() {
        // Belt-and-braces: the canonical workspace shape is
        // `<slug>.atlassian.net`, but the rule is "host is under the
        // `.atlassian.net` apex". Accept the apex itself defensively
        // so a future product flow that targets the apex (rare) does
        // not need a re-deploy.
        for good in [
            "https://acme.atlassian.net",
            "https://my-team.atlassian.net/",
            "https://Acme.Atlassian.NET",
        ] {
            parse_workspace_url(good).unwrap_or_else(|e| panic!("{good} must parse: {e}"));
        }
    }

    #[test]
    fn new_atlassian_secret_ref_is_unique_per_call() {
        let a = new_atlassian_secret_ref();
        let b = new_atlassian_secret_ref();
        assert_eq!(a.keychain_service, ATLASSIAN_KEYCHAIN_SERVICE);
        assert_eq!(b.keychain_service, ATLASSIAN_KEYCHAIN_SERVICE);
        assert_ne!(a.keychain_account, b.keychain_account);
        assert!(a.keychain_account.starts_with("slot:"));
    }

    // -------------------------------------------------------------------
    // `atlassian_sources_add_impl` IPC integration tests
    //
    // These cover the four journeys the module docs enumerate (A, B,
    // C-mode-1, C-mode-2) plus the rejection paths the frontend relies
    // on to short-circuit before firing the IPC at all.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn journey_a_shared_pat_writes_two_sources_and_one_keychain_row() {
        // Shared-PAT default: one `sources_add` call with both products
        // enabled must land two `Source` rows pointing at the *same*
        // `SecretRef`, and the keychain must hold exactly one entry.
        let (state, _dir) = make_state().await;

        let rows = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(token()),
            "5d53f3cbc6b9320d9ea5bdc2".into(),
            true,
            true,
            None,
        )
        .await
        .expect("shared-PAT add succeeds");

        assert_eq!(rows.len(), 2, "shared PAT adds both products");
        let jira = rows.iter().find(|r| r.kind == SourceKind::Jira).unwrap();
        let conf = rows
            .iter()
            .find(|r| r.kind == SourceKind::Confluence)
            .unwrap();
        assert_eq!(
            jira.secret_ref, conf.secret_ref,
            "shared-PAT journey must write one SecretRef, reused across rows"
        );
        let sr = jira.secret_ref.clone().unwrap();
        assert_eq!(sr.keychain_service, ATLASSIAN_KEYCHAIN_SERVICE);
        assert!(sr.keychain_account.starts_with("slot:"));

        // Exactly one keychain row, and it holds the token we sent.
        let value = state
            .secrets
            .get(&secret_store_key(&sr))
            .expect("keychain get")
            .expect("keychain row present");
        assert_eq!(value.expose_secret(), "atlassian-pat-token");
    }

    #[tokio::test]
    async fn journey_b_single_product_writes_one_source() {
        // Single-product add: user enables exactly one product; the
        // other arm must not be touched.
        let (state, _dir) = make_state().await;

        let rows = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(token()),
            "acc-1".into(),
            true,
            false,
            None,
        )
        .await
        .expect("jira-only add succeeds");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, SourceKind::Jira);
    }

    #[tokio::test]
    async fn journey_c_mode_1_reuse_secret_ref_writes_zero_keychain_rows() {
        // Add Jira first, then Confluence reusing the same SecretRef.
        // The second call must NOT write a new keychain row — the
        // dialog's "reuse existing token" path is how DAY-81's
        // refcount guard actually gets shared keychain rows in
        // practice.
        let (state, _dir) = make_state().await;

        let jira_rows = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(token()),
            "acc-1".into(),
            true,
            false,
            None,
        )
        .await
        .expect("jira add succeeds");
        let shared_ref = jira_rows[0]
            .secret_ref
            .clone()
            .expect("jira has secret_ref");

        // Now add Confluence reusing the exact same SecretRef, with
        // `api_token = None` — reuse mode must not demand a token.
        let conf_rows = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            None,
            "acc-1".into(),
            false,
            true,
            Some(shared_ref.clone()),
        )
        .await
        .expect("confluence reuse-PAT add succeeds");

        assert_eq!(conf_rows.len(), 1);
        assert_eq!(conf_rows[0].kind, SourceKind::Confluence);
        assert_eq!(
            conf_rows[0].secret_ref.as_ref(),
            Some(&shared_ref),
            "reuse mode must point at the same SecretRef"
        );
    }

    #[tokio::test]
    async fn journey_c_mode_2_separate_pat_writes_new_keychain_row() {
        // Second product with a *different* PAT: must write a brand-
        // new keychain slot uncoupled from the first source's row.
        let (state, _dir) = make_state().await;

        let jira_rows = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(token()),
            "acc-1".into(),
            true,
            false,
            None,
        )
        .await
        .expect("jira add");
        let jira_ref = jira_rows[0].secret_ref.clone().unwrap();

        let conf_rows = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(IpcSecretString::new("different-token")),
            "acc-1".into(),
            false,
            true,
            None,
        )
        .await
        .expect("confluence separate-PAT add");
        let conf_ref = conf_rows[0].secret_ref.clone().unwrap();

        assert_ne!(
            jira_ref.keychain_account, conf_ref.keychain_account,
            "separate-PAT mode must mint a fresh keychain slot"
        );
        let conf_token = state
            .secrets
            .get(&secret_store_key(&conf_ref))
            .expect("get")
            .expect("present");
        assert_eq!(conf_token.expose_secret(), "different-token");
    }

    #[tokio::test]
    async fn rejects_when_both_products_disabled() {
        let (state, _dir) = make_state().await;
        let err = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(token()),
            "acc-1".into(),
            false,
            false,
            None,
        )
        .await
        .expect_err("must reject when nothing is enabled");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_NO_PRODUCT_SELECTED);
    }

    #[tokio::test]
    async fn rejects_reuse_when_keychain_slot_is_empty() {
        // Stale dialog state: the user had two sources, deleted the
        // owner, then tried to add the other product. The secret_ref
        // the dialog kept in state is pointing at an empty slot, and
        // we must refuse to persist a row that would reference a
        // missing key.
        let (state, _dir) = make_state().await;
        let stale = SecretRef {
            keychain_service: ATLASSIAN_KEYCHAIN_SERVICE.into(),
            keychain_account: "slot:deadbeef".into(),
        };
        let err = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            None,
            "acc-1".into(),
            true,
            false,
            Some(stale),
        )
        .await
        .expect_err("must reject reuse of empty slot");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_REUSE_SECRET_MISSING);
    }

    #[tokio::test]
    async fn rejects_fresh_add_without_api_token() {
        // `reuse_secret_ref = None` and `api_token = None` is a caller
        // bug: the dialog should have either collected a token or
        // passed a SecretRef. Refuse before touching sqlite.
        let (state, _dir) = make_state().await;
        let err = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            None,
            "acc-1".into(),
            true,
            false,
            None,
        )
        .await
        .expect_err("must reject fresh add without token");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING);
    }

    #[tokio::test]
    async fn rejects_empty_email() {
        let (state, _dir) = make_state().await;
        let err = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "   ".into(),
            Some(token()),
            "acc-1".into(),
            true,
            false,
            None,
        )
        .await
        .expect_err("whitespace email must be rejected");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING);
    }

    #[tokio::test]
    async fn rejects_malformed_workspace_url() {
        let (state, _dir) = make_state().await;
        let err = atlassian_sources_add_impl(
            &state,
            "not-a-url".into(),
            "user@acme.com".into(),
            Some(token()),
            "acc-1".into(),
            true,
            false,
            None,
        )
        .await
        .expect_err("malformed url must be rejected");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_INVALID_WORKSPACE_URL);
    }

    #[tokio::test]
    async fn rejects_empty_account_id() {
        // `atlassian_sources_add` is a purely transactional op — the
        // frontend is required to call `validate_credentials` first
        // and pass the resulting `account_id` through. An empty
        // account_id here means the dialog skipped validation, which
        // would leave `source_identities` unseeded.
        let (state, _dir) = make_state().await;
        let err = atlassian_sources_add_impl(
            &state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(token()),
            "   ".into(),
            true,
            false,
            None,
        )
        .await
        .expect_err("empty account_id must be rejected");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING);
    }

    // -------------------------------------------------------------------
    // `atlassian_sources_reconnect_impl` IPC integration tests (DAY-87)
    //
    // The happy path routes through `discover_cloud`, which makes a
    // live HTTPS request and cannot be exercised from a pure unit
    // test without a recorded fixture harness we don't have here (the
    // sibling `atlassian_validate_credentials` ships with the same
    // constraint and is covered at the E2E layer instead). These
    // tests pin the pre-network branches: kind checks, missing-row
    // rejection, the v0.2 upgrade-row email-missing case, and the
    // secret-ref presence guard. Post-network invariants (identity
    // binding, keychain rotation, sibling enumeration) are pinned by
    // `e2e/features/happy-path/atlassian-reconnect.feature`.
    // -------------------------------------------------------------------

    async fn seed_jira_source(state: &AppState) -> Source {
        atlassian_sources_add_impl(
            state,
            "https://acme.atlassian.net".into(),
            "user@acme.com".into(),
            Some(token()),
            "acc-1".into(),
            true,
            false,
            None,
        )
        .await
        .expect("seed jira source")
        .into_iter()
        .next()
        .expect("one row")
    }

    #[tokio::test]
    async fn reconnect_rejects_empty_token() {
        let (state, _dir) = make_state().await;
        let source = seed_jira_source(&state).await;
        let err = atlassian_sources_reconnect_impl(&state, source.id, IpcSecretString::new("   "))
            .await
            .expect_err("whitespace-only token must fail before any network call");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING);
    }

    #[tokio::test]
    async fn reconnect_rejects_missing_source() {
        let (state, _dir) = make_state().await;
        let err = atlassian_sources_reconnect_impl(&state, Uuid::new_v4(), token())
            .await
            .expect_err("unknown source_id must be rejected");
        assert_eq!(err.code(), error_codes::IPC_SOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn reconnect_rejects_non_atlassian_source() {
        // Seed a LocalGit source directly through the repo — the IPC
        // `sources_add` path has its own surface and we only need
        // "an existing row whose kind is not Jira or Confluence".
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

        let err = atlassian_sources_reconnect_impl(&state, local.id, token())
            .await
            .expect_err("LocalGit source must not be reconnectable via the atlassian IPC");
        assert_eq!(err.code(), error_codes::IPC_SOURCE_CONFIG_KIND_MISMATCH);
    }

    #[tokio::test]
    async fn reconnect_rejects_source_without_secret_ref() {
        // A Jira row whose `secret_ref` is null is a pre-DAY-81 or
        // keychain-wiped artefact. Reconnect cannot rotate a token
        // that has no slot to land in; the user is steered toward
        // delete + re-add instead of forking the add codepath here.
        let (state, _dir) = make_state().await;
        let repo = SourceRepo::new(state.pool.clone());
        let ghost = Source {
            id: Uuid::new_v4(),
            kind: SourceKind::Jira,
            label: "Jira — ghost".into(),
            config: SourceConfig::Jira {
                workspace_url: "https://acme.atlassian.net".into(),
                email: "user@acme.com".into(),
            },
            secret_ref: None,
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        repo.insert(&ghost).await.expect("insert ghost");

        let err = atlassian_sources_reconnect_impl(&state, ghost.id, token())
            .await
            .expect_err("secret_ref-less jira row must not accept reconnect");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_REUSE_SECRET_MISSING);
    }

    #[tokio::test]
    async fn reconnect_rejects_source_with_empty_email() {
        // DOG-v0.2-04 upgrade artefact: a Confluence row whose email
        // defaulted to empty because no Jira sibling existed at
        // backfill time. `discover_cloud` would reject this with a
        // 400 shape-change error downstream; surface the structural
        // problem locally instead so the error reads "add a fresh
        // source" rather than "atlassian shape changed".
        let (state, _dir) = make_state().await;
        let repo = SourceRepo::new(state.pool.clone());
        let sr = new_atlassian_secret_ref();
        let key = secret_store_key(&sr);
        state
            .secrets
            .put(&key, Secret::new("ignored".to_string()))
            .expect("seed keychain");
        let row = Source {
            id: Uuid::new_v4(),
            kind: SourceKind::Confluence,
            label: "Confluence — backfill".into(),
            config: SourceConfig::Confluence {
                workspace_url: "https://acme.atlassian.net".into(),
                email: "".into(),
            },
            secret_ref: Some(sr),
            created_at: Utc::now(),
            last_sync_at: None,
            last_health: SourceHealth::unchecked(),
        };
        repo.insert(&row).await.expect("insert row");

        let err = atlassian_sources_reconnect_impl(&state, row.id, token())
            .await
            .expect_err("empty-email row must surface credentials_missing");
        assert_eq!(err.code(), error_codes::IPC_ATLASSIAN_CREDENTIALS_MISSING);
    }
}
