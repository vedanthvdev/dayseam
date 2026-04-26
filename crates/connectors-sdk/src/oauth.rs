//! OAuth 2.0 helpers: PKCE, token exchange, and refresh-with-single-flight.
//!
//! This module closes the OAuth wire work DAY-200 left as a shape-only
//! scaffold. It ships the runtime logic — PKCE code-verifier/challenge
//! generation per RFC 7636, the authorization-code exchange against a
//! token endpoint, and a single-flight refresh path that [`OAuthAuth`]
//! itself calls on every `authenticate()` when the stored access token
//! has passed its `expires_at`.
//!
//! Layering discipline (see `tests/no_cross_crate_leak.rs`): this
//! module never imports `dayseam-secrets`. The refresh path writes new
//! tokens back to persistent storage through the [`TokenPersister`]
//! trait, which the orchestrator implements with whatever keychain
//! back-end is in force (macOS Keychain today; a Linux Secret Service
//! adapter later). The SDK only knows the shape of the callback, not
//! where the bytes land.
//!
//! Concurrency model for refresh: [`OAuthAuth`] holds its mutable
//! token state behind a `tokio::sync::Mutex`. A concurrent-sync
//! scenario where two connector walkers both expire the token at once
//! sees exactly one network round-trip to the IdP — the second caller
//! blocks on the mutex, then re-reads the now-fresh state and skips
//! the refresh entirely (double-checked locking, but with an async
//! mutex so it behaves correctly on a multi-threaded runtime). This
//! matters once v0.9's Outlook connector sits alongside the existing
//! GitHub / GitLab / Atlassian walkers: a cold start on day 1 fires
//! all five in parallel and the OAuth-backed one would otherwise
//! serialise behind itself for a chain of refresh calls equal to the
//! number of concurrent requests.
//!
//! Error-code contract: every non-transport failure on this module's
//! surface resolves to one of:
//! * [`error_codes::OAUTH_REFRESH_REJECTED`] — the IdP returned a
//!   terminal 4xx on either exchange or refresh (`invalid_grant`,
//!   consent revoked, refresh token expired past its absolute
//!   lifetime, etc.). Non-retryable; UI prompts reconnect.
//! * [`error_codes::OAUTH_SCOPE_DOWNGRADED`] — informational rather
//!   than fatal. The refresh succeeded, the new access token is
//!   usable, but the granted scopes are a strict subset of what the
//!   descriptor recorded. Surfaced so the orchestrator can compare
//!   against the connector's declared requirement and, if the
//!   intersection falls short, raise a non-fatal reconnect nudge on
//!   the next sync boundary. The refresh path does **not** error on
//!   downgrade — an in-flight sync with a narrower-but-valid token
//!   should finish what it started.
//! * Transport failures (DNS, TLS, connect, timeout, 5xx) reuse the
//!   existing `http.transport.*` and `http.retry.*` families from
//!   [`dayseam_core::error_codes`]; they are handled by the SDK's
//!   retry policy, not here.

use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use dayseam_core::{error_codes, DayseamError};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// RFC 7636 §4.1 code verifier: 43–128 characters from the
/// unreserved set `[A-Za-z0-9\-._~]`. We emit exactly 43 characters
/// drawn from 256 bits of crypto randomness, base64url-encoded with
/// no padding — the minimum valid length, which maximises the
/// fraction of the URL that is actually entropy rather than padding
/// noise, and which round-trips cleanly through Microsoft's
/// token-endpoint parser (documented to accept 43-char verifiers as
/// of 2025-06-17, verified in the `wiremock` happy-path test).
///
/// The verifier bytes are **secret**: leaking the verifier plus the
/// authorisation code is sufficient to mint a token pair. The type
/// therefore wraps its inner `String` in a zeroize-on-drop /
/// redacted-`Debug` envelope (the same local secret-string pattern
/// the surrounding `auth.rs` module uses for access and refresh
/// tokens, restated here to keep the OAuth module self-contained and
/// to avoid a re-export).
///
/// `CodeVerifier` is deliberately not `Clone` and not `Copy`: a
/// verifier is used exactly once, handed to exactly one
/// `exchange_code` call, and then dropped. Anything that wants two
/// verifiers generates two.
pub struct CodeVerifier(crate::auth::SecretString);

impl CodeVerifier {
    /// Expose the raw verifier string for the duration of a single
    /// `exchange_code` call. Intentionally `pub(crate)` so third
    /// parties cannot reach past the type to log the bytes; the only
    /// consumer is [`exchange_code`] in this same module.
    pub(crate) fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl std::fmt::Debug for CodeVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CodeVerifier(***)")
    }
}

/// RFC 7636 §4.2 code challenge, S256 method:
/// `base64url(sha256(verifier_bytes))`, no padding. Unlike the
/// verifier, the challenge is **not** secret — it travels over the
/// wire in the browser's authorize URL and shows up in user-agent
/// logs by design. Still, we avoid logging it ourselves: there is no
/// public reason for our own code to print one.
#[derive(Debug, Clone)]
pub struct CodeChallenge(String);

impl CodeChallenge {
    /// The challenge string as it travels in the `code_challenge`
    /// query parameter.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Generate a fresh PKCE pair. Uses the caller-supplied RNG so tests
/// can install a deterministic source; production call sites pass
/// [`rand::thread_rng`].
pub fn generate_pkce_pair<R: RngCore>(rng: &mut R) -> (CodeVerifier, CodeChallenge) {
    // 32 random bytes → 43-character base64url string (the 256-bit
    // minimum that keeps the verifier inside the 43..=128 RFC
    // window with no padding).
    let mut raw = [0u8; 32];
    rng.fill_bytes(&mut raw);
    let verifier_str = BASE64_URL_NO_PAD.encode(raw);

    let mut hasher = Sha256::new();
    hasher.update(verifier_str.as_bytes());
    let digest = hasher.finalize();
    let challenge_str = BASE64_URL_NO_PAD.encode(digest);

    (
        CodeVerifier(crate::auth::SecretString::new(verifier_str)),
        CodeChallenge(challenge_str),
    )
}

/// Freshly minted or freshly refreshed credentials as returned by the
/// IdP. Flows out of [`exchange_code`] and [`OAuthAuth::refresh_if_expired`]
/// and is the shape the orchestrator hands to a [`TokenPersister`] for
/// keychain write-back.
///
/// `access_token` and `refresh_token` are plain `String` at this
/// boundary — the whole point of the pair is that it just crossed an
/// HTTPS response and is about to cross a keychain write. The
/// consumer ([`OAuthAuth::refresh_if_expired`], which is the only
/// in-SDK consumer today) re-wraps both into the local
/// `SecretString` the instant the pair is deconstructed. Orchestrator
/// callers are expected to follow the same discipline; to make the
/// intent loud at the call site, the struct is `#[non_exhaustive]`
/// so a future field (`id_token` for the DAY-220 identity pass,
/// say) doesn't force every consumer to break their destructure
/// pattern silently.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TokenPair {
    /// Bearer token for calling the resource server. Short-lived
    /// (Microsoft Graph is 60–90 min in practice, tenant-configurable).
    pub access_token: String,
    /// Refresh token. Long-lived (Microsoft defaults to 24 h
    /// sliding and 90 d absolute for work/school). Exchanging it
    /// for a new access token rotates the refresh token too in
    /// Microsoft's current flow, so `OAuthAuth::refresh_if_expired`
    /// writes both fields back via the persister on every success.
    pub refresh_token: String,
    /// Absolute UTC instant at which the access token should be
    /// treated as expired. Computed by the refresh path as
    /// `clock.now() + expires_in_secs` at the moment the IdP
    /// response is parsed — using a relative `expires_in` on the
    /// wire and an absolute `expires_at` at rest, because the
    /// absolute form is what the storage layer actually needs.
    pub access_expires_at: DateTime<Utc>,
    /// Delegated scopes the IdP says this access token actually
    /// carries. Empty when the IdP omits the `scope` field on the
    /// refresh response (some tenants do this, per Microsoft's
    /// documented behaviour, to mean "the same scopes as before").
    /// Populated and compared against the descriptor's recorded
    /// scopes to detect a tenant-admin-driven downgrade.
    pub granted_scopes: Vec<String>,
}

impl TokenPair {
    /// Construct a [`TokenPair`] outside this crate. The struct is
    /// `#[non_exhaustive]` so callers (e.g. the desktop
    /// `build_source_auth` path that rehydrates tokens from the
    /// Keychain, or unit tests that fake a pair into the session
    /// registry) would otherwise have no way to make one. Adding new
    /// fields stays source-compatible because this constructor owns
    /// the full field list.
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        access_expires_at: DateTime<Utc>,
        granted_scopes: Vec<String>,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            access_expires_at,
            granted_scopes,
        }
    }
}

/// Callback shape for persisting a freshly refreshed token pair back
/// to the OS keychain (or whatever durable store the orchestrator
/// wires in). Lives here in `connectors-sdk` so the SDK can call it
/// without importing `dayseam-secrets`, which the cross-crate
/// layering guard forbids.
///
/// Implementations are expected to persist **both** the access and
/// refresh token fields on every call — Microsoft's token endpoint
/// rotates the refresh token on each grant, so a write that only
/// covered the access token would leave the user one refresh away
/// from a forced reconnect.
///
/// The trait is `async` via `async_trait::async_trait` because the
/// real implementation (macOS Keychain via `security-framework`)
/// blocks, and the orchestrator dispatches it through `spawn_blocking`.
#[async_trait::async_trait]
pub trait TokenPersister: Send + Sync {
    /// Persist `pair` under the keychain rows the originating
    /// descriptor named. Returns on successful durable write; errors
    /// propagate as `DayseamError::Internal` because from the SDK's
    /// point of view a failed persist is a platform bug, not a user-
    /// actionable OAuth problem.
    async fn persist_pair(&self, pair: &TokenPair) -> Result<(), DayseamError>;
}

/// Wire-format of the token endpoint's success response. Private
/// because it is an implementation detail of the exchange/refresh
/// glue; the typed surface the rest of the crate sees is
/// [`TokenPair`].
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
}

/// Wire-format of the token endpoint's failure response. RFC 6749
/// §5.2 mandates `error` and treats `error_description` and
/// `error_uri` as optional. We parse all three so diagnostics survive
/// being rendered as a `DayseamError::Auth` `message`.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Exchange an authorisation code (plus the verifier the Tauri shell
/// generated at the start of the flow) for a fresh [`TokenPair`].
/// One-shot: called exactly once per consent dance. If it fails, the
/// user sees a reconnect-prompting toast; there is no "retry the
/// exchange" path, since the auth code itself is one-use.
///
/// `redirect_uri` must match the value sent in the original authorise
/// URL — Microsoft's token endpoint rejects mismatches with
/// `invalid_grant`, which we surface as
/// [`error_codes::OAUTH_REFRESH_REJECTED`]. (The code name is
/// historical to "refresh-rejected" but covers both exchange and
/// refresh; see module docs.)
pub async fn exchange_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    verifier: &CodeVerifier,
    redirect_uri: &str,
    clock: &dyn crate::Clock,
) -> Result<TokenPair, DayseamError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier.expose()),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
    ];

    let response = http
        .post(token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| DayseamError::Network {
            code: error_codes::HTTP_TRANSPORT.to_string(),
            message: format!("token endpoint unreachable for exchange: {e}"),
        })?;

    parse_token_response(response, clock, "token exchange").await
}

/// Internal: turn a token-endpoint response into a [`TokenPair`] or
/// the right [`DayseamError`]. Shared by [`exchange_code`] and
/// [`OAuthAuth::refresh_if_expired`] because the parse rules are
/// identical at the wire level (RFC 6749 §5.1/5.2).
async fn parse_token_response(
    response: reqwest::Response,
    clock: &dyn crate::Clock,
    op: &str,
) -> Result<TokenPair, DayseamError> {
    let status = response.status();
    let body = response.bytes().await.map_err(|e| DayseamError::Network {
        code: error_codes::HTTP_TRANSPORT.to_string(),
        message: format!("failed to read token endpoint body during {op}: {e}"),
    })?;

    if status.is_success() {
        let parsed: TokenResponse =
            serde_json::from_slice(&body).map_err(|e| DayseamError::Auth {
                code: error_codes::OAUTH_REFRESH_REJECTED.to_string(),
                message: format!(
                    "{op} returned 2xx but body did not parse as an OAuth token response: {e}"
                ),
                retryable: false,
                action_hint: Some("re-authenticate via the source's Reconnect flow".to_string()),
            })?;

        let access_expires_at = clock.now() + Duration::seconds(parsed.expires_in);
        let granted_scopes = parsed
            .scope
            .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default();

        return Ok(TokenPair {
            access_token: parsed.access_token,
            // Microsoft rotates refresh tokens on every grant; some
            // other IdPs omit the field when the existing refresh is
            // still good. Fall back to an empty string here and let
            // the caller decide whether to keep the previous refresh
            // (OAuthAuth::refresh_if_expired does exactly that).
            refresh_token: parsed.refresh_token.unwrap_or_default(),
            access_expires_at,
            granted_scopes,
        });
    }

    // Treat any 4xx as terminal. 5xx shouldn't reach here because the
    // SDK's retry policy handles them at the HttpClient layer, but
    // on the off-chance one slips through (direct reqwest::Client is
    // used here for the token endpoint, not HttpClient), surface it
    // as a terminal auth error rather than an opaque 500 — the user
    // reaction is still "reconnect when ready".
    if let Ok(err_body) = serde_json::from_slice::<TokenErrorResponse>(&body) {
        return Err(DayseamError::Auth {
            code: error_codes::OAUTH_REFRESH_REJECTED.to_string(),
            message: format!(
                "{op} rejected by IdP: {code}{desc}",
                code = err_body.error,
                desc = err_body
                    .error_description
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            ),
            retryable: false,
            action_hint: Some("re-authenticate via the source's Reconnect flow".to_string()),
        });
    }

    // Unparseable body — still treat as terminal, but include the
    // status code so bug reports have enough context.
    Err(DayseamError::Auth {
        code: error_codes::OAUTH_REFRESH_REJECTED.to_string(),
        message: format!("{op} failed with HTTP {status} and an unparseable token-endpoint body"),
        retryable: false,
        action_hint: Some("re-authenticate via the source's Reconnect flow".to_string()),
    })
}

/// Internal helper: run a `refresh_token` grant against the endpoint
/// baked into the current `OAuthAuth`'s descriptor. Called by
/// [`OAuthAuth::refresh_if_expired`] under the state mutex.
pub(crate) async fn run_refresh(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    refresh_token: &str,
    scopes: &[String],
    clock: &dyn crate::Clock,
) -> Result<TokenPair, DayseamError> {
    // RFC 6749 §6: `grant_type=refresh_token`. The `scope` parameter
    // is optional; we always send the originally-consented set so
    // that a silent narrowing by the IdP (tenant admin removed a
    // scope) shows up as a mismatch on the response `scope` field
    // rather than as a silently-succeeding noop.
    let scope_string = scopes.join(" ");
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if !scope_string.is_empty() {
        form.push(("scope", &scope_string));
    }

    let response = http
        .post(token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| DayseamError::Network {
            code: error_codes::HTTP_TRANSPORT.to_string(),
            message: format!("token endpoint unreachable for refresh: {e}"),
        })?;

    parse_token_response(response, clock, "token refresh").await
}

/// Compare the scope set the IdP actually granted with what the
/// descriptor recorded. Returns `true` iff the granted set is a
/// strict subset of the requested set — i.e. a downgrade worth
/// telling the orchestrator about. Equality and over-grant (rare
/// but legal for IdPs that silently widen) both return `false`:
/// only narrowing is interesting.
///
/// Exposed as `pub` rather than `pub(crate)` because the
/// orchestrator (which lives in a downstream crate) implements
/// [`TokenPersister`] and needs one canonical place to evaluate
/// whether a freshly persisted pair crossed the downgrade
/// threshold before raising the reconnect nudge in the UI. Keeping
/// the comparator here means the "what counts as a downgrade"
/// rule has exactly one definition across the workspace.
pub fn is_scope_downgrade(requested: &[String], granted: &[String]) -> bool {
    if granted.is_empty() {
        // Empty `scope` on the response means "same as before" per
        // Microsoft's documented behaviour — explicitly not a
        // downgrade signal.
        return false;
    }
    let granted_set: std::collections::BTreeSet<&String> = granted.iter().collect();
    let mut any_missing = false;
    for want in requested {
        if !granted_set.contains(want) {
            any_missing = true;
            break;
        }
    }
    any_missing
}

/// Construct a [`DayseamError::Auth`] carrying
/// [`error_codes::OAUTH_SCOPE_DOWNGRADED`]. Not used as a hard
/// failure by this module (the refresh still returns `Ok(())`); the
/// orchestrator builds the same shape when it decides the downgrade
/// has crossed the threshold that warrants a reconnect nudge.
///
/// Lives here so there is exactly one place that knows the
/// `action_hint` copy for this code.
pub fn scope_downgrade_error(requested: &[String], granted: &[String]) -> DayseamError {
    DayseamError::Auth {
        code: error_codes::OAUTH_SCOPE_DOWNGRADED.to_string(),
        message: format!(
            "IdP granted a narrower scope set than requested: granted={granted:?}, requested={requested:?}"
        ),
        retryable: false,
        action_hint: Some(
            "admin policy may have tightened; prompt the user to reconnect the source".to_string(),
        ),
    }
}

/// Noop persister for tests and as a default for contexts that don't
/// yet have a keychain attached. Holds no state; discards any pair
/// it's given. Publicly exposed because the Tauri shell sometimes
/// wants to build an `OAuthAuth` for a wiremock-backed integration
/// test before the real persister is wired up.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopTokenPersister;

#[async_trait::async_trait]
impl TokenPersister for NoopTokenPersister {
    async fn persist_pair(&self, _pair: &TokenPair) -> Result<(), DayseamError> {
        Ok(())
    }
}

/// Shared-pointer alias used by [`OAuthAuth`] so the strategy can be
/// handed around cheaply while its persister stays put. The real
/// orchestrator stashes one persister on its desktop-singleton
/// context; every `OAuthAuth` it builds gets an `Arc::clone` of it.
pub type SharedPersister = Arc<dyn TokenPersister>;

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn pkce_verifier_is_rfc7636_length_and_alphabet() {
        let mut rng = StdRng::seed_from_u64(42);
        let (verifier, _challenge) = generate_pkce_pair(&mut rng);
        let s = verifier.expose();
        assert_eq!(s.len(), 43, "verifier should be 43 base64url chars");
        assert!(
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "verifier must use only the unreserved base64url alphabet, got: {s}"
        );
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let mut rng = StdRng::seed_from_u64(7);
        let (verifier, challenge) = generate_pkce_pair(&mut rng);

        let mut hasher = Sha256::new();
        hasher.update(verifier.expose().as_bytes());
        let expected = BASE64_URL_NO_PAD.encode(hasher.finalize());

        assert_eq!(challenge.as_str(), expected);
        // Challenge is also 43 chars because 32-byte digest →
        // 43-char base64url (no-pad) string.
        assert_eq!(challenge.as_str().len(), 43);
    }

    #[test]
    fn pkce_is_deterministic_for_a_given_rng_seed() {
        let mut rng_a = StdRng::seed_from_u64(99);
        let mut rng_b = StdRng::seed_from_u64(99);
        let (va, ca) = generate_pkce_pair(&mut rng_a);
        let (vb, cb) = generate_pkce_pair(&mut rng_b);
        assert_eq!(va.expose(), vb.expose());
        assert_eq!(ca.as_str(), cb.as_str());
    }

    #[test]
    fn pkce_differs_across_calls_with_an_unseeded_rng() {
        let mut rng = rand::thread_rng();
        let (v1, _c1) = generate_pkce_pair(&mut rng);
        let (v2, _c2) = generate_pkce_pair(&mut rng);
        // Astronomically unlikely to collide on two 256-bit draws;
        // this is a smoke test for "we actually called the RNG".
        assert_ne!(v1.expose(), v2.expose());
    }

    #[test]
    fn code_verifier_debug_is_redacted() {
        let mut rng = StdRng::seed_from_u64(1);
        let (verifier, _) = generate_pkce_pair(&mut rng);
        let dbg = format!("{verifier:?}");
        assert_eq!(dbg, "CodeVerifier(***)");
        assert!(
            !dbg.contains(verifier.expose()),
            "Debug must not contain the raw verifier"
        );
    }

    #[test]
    fn scope_downgrade_detects_strict_subset() {
        let requested = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let granted = vec!["a".to_string(), "b".to_string()];
        assert!(is_scope_downgrade(&requested, &granted));
    }

    #[test]
    fn scope_downgrade_ignores_empty_granted_set() {
        // Empty `scope` on the IdP response means "same as before"
        // per Microsoft's docs — not a downgrade.
        let requested = vec!["a".to_string(), "b".to_string()];
        let granted: Vec<String> = vec![];
        assert!(!is_scope_downgrade(&requested, &granted));
    }

    #[test]
    fn scope_downgrade_returns_false_on_equal_sets() {
        let requested = vec!["a".to_string(), "b".to_string()];
        let granted = vec!["b".to_string(), "a".to_string()];
        assert!(!is_scope_downgrade(&requested, &granted));
    }

    #[test]
    fn scope_downgrade_returns_false_on_over_grant() {
        let requested = vec!["a".to_string()];
        let granted = vec!["a".to_string(), "b".to_string()];
        assert!(!is_scope_downgrade(&requested, &granted));
    }

    #[test]
    fn noop_persister_is_ready_to_use() {
        let p = NoopTokenPersister;
        tokio_test_block_on(async {
            let pair = TokenPair {
                access_token: "a".to_string(),
                refresh_token: "r".to_string(),
                access_expires_at: Utc::now(),
                granted_scopes: vec![],
            };
            p.persist_pair(&pair)
                .await
                .expect("noop persist must succeed");
        });
    }

    fn tokio_test_block_on<F: std::future::Future<Output = ()>>(f: F) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(f);
    }

    #[test]
    fn scope_downgrade_error_carries_expected_code() {
        let err = scope_downgrade_error(&["a".into(), "b".into()], &["a".into()]);
        match err {
            DayseamError::Auth { code, .. } => {
                assert_eq!(code, error_codes::OAUTH_SCOPE_DOWNGRADED);
            }
            other => panic!("expected DayseamError::Auth, got {other:?}"),
        }
    }
}
