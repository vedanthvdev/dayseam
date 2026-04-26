//! Authentication strategies.
//!
//! `AuthStrategy` is the **durable** per-source shape, not a v0.1
//! placeholder. v0.1 ships exactly one impl — [`PatAuth`] — because
//! that is what GitLab self-hosted wants and because local git needs no
//! auth at all. Later phases add [`OAuth2Auth`] and [`GitHubAppAuth`]
//! as additional impls; none of them require a trait rewrite.
//!
//! The trait itself is deliberately narrow: "attach yourself to this
//! outgoing request". Connectors never ask the auth strategy for a
//! token directly — that keeps secret strings off the connector's own
//! stack frames and lets the strategy implement refresh/rotation
//! (future OAuth2 work) without every connector learning about it.
//!
//! Secrets live in the OS keychain, loaded via `dayseam-secrets`. The
//! auth strategy itself only holds references (strings, opaque
//! handles) — never the raw token bytes for longer than a single
//! request.

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use dayseam_core::{error_codes, DayseamError};
use tokio::sync::Mutex;
use zeroize::Zeroize;

use crate::oauth::{self, SharedPersister, TokenPair};
use crate::{Clock, SystemClock};

/// Local, zero-dependency token wrapper used by the SDK's built-in
/// auth strategies.
///
/// We deliberately do **not** import `dayseam_secrets::Secret` here:
/// `tests/no_cross_crate_leak.rs` forbids `connectors-sdk` from
/// depending on `dayseam-secrets` so a third-party connector author
/// cannot reach past `AuthStrategy` to load raw tokens. This 20-line
/// wrapper gives us the same two guarantees that matter at this
/// layer — `Debug` never prints the value, and `Drop` zeroes it —
/// without pulling in the heavier dep.
///
/// `pub(crate)` rather than fully private because DAY-201's
/// `oauth` module reuses the same wrapper for PKCE code verifiers
/// and the reshaped `OAuthAuth` state behind its mutex. It stays
/// crate-private: there is no intentional external consumer.
pub(crate) struct SecretString(String);

impl SecretString {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Durable description of how a source authenticates. Serialises into
/// the source's `SourceConfig`; deserialising it reconstructs the
/// matching [`AuthStrategy`] impl without touching the keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDescriptor {
    /// No authentication required (local git, public endpoints the
    /// user has opted in to via a dedicated connector).
    None,
    /// Personal access token — valid v0.1 shape for GitLab and, later,
    /// Jira Data Center. The payload is a keychain handle (`service` +
    /// `account`), never the token itself.
    Pat {
        keychain_service: String,
        keychain_account: String,
    },
    /// HTTP Basic with an `email:api_token` pair — the v0.2 shape for
    /// Atlassian Cloud (Jira + Confluence).
    ///
    /// The descriptor carries **one** keychain handle per instance.
    /// Whether that handle is *shared* with another source (the common
    /// case — one Atlassian PAT unlocks both Jira and Confluence on the
    /// same tenant) or *unique* to this source (separate-PAT mode — a
    /// user who, e.g., manages Jira and Confluence on different
    /// tenants, or uses different service accounts per product) is a
    /// decision made at the IPC layer when `SourceConfig::{Jira,
    /// Confluence}` rows are persisted in DAY-76 / DAY-79. The
    /// `AuthStrategy` impl is agnostic: two instances with the same
    /// `(keychain_service, keychain_account)` produce identical
    /// `Authorization` headers for the same live token, and two
    /// instances with *different* handles each resolve independently.
    /// The ref-count guard in DAY-81 only matters in the shared-handle
    /// case; in the separate-handle case it degenerates to 1-per-row
    /// and delete-on-last-source behaves exactly as if the refcount
    /// did not exist.
    ///
    /// `email` is **not** a secret — it appears in Jira UI, JQL result
    /// sets, and Confluence version history — so we store it plain.
    /// The `api_token` bytes live only in the [`SecretString`] inside
    /// the matching [`BasicAuth`] instance and never touch the
    /// descriptor.
    Basic {
        email: String,
        keychain_service: String,
        keychain_account: String,
    },
    /// OAuth 2.0 bearer-token pair (access + refresh) plus the metadata
    /// needed to reconstruct the matching [`OAuthAuth`] strategy after a
    /// process restart without re-running the consent dance.
    ///
    /// The access token and refresh token live in the OS keychain under
    /// **separate rows** — same `keychain_service`, different
    /// `account` strings — so a compromise of one row does not unlock
    /// the other. Callers that read the pair use
    /// [`dayseam_secrets::oauth::get_access_token`] and
    /// [`dayseam_secrets::oauth::get_refresh_token`]; the naming
    /// convention (`.oauth.access` / `.oauth.refresh` suffix on the
    /// account field) is the caller's responsibility when the row is
    /// first written — the descriptor just records whatever account
    /// strings the orchestrator picked and reuses them on every load.
    ///
    /// DAY-201 closes the wire work: [`OAuthAuth`] now does a real
    /// single-flighted refresh against the IdP token endpoint on
    /// every `authenticate()` call whose stored access token has
    /// expired, and [`crate::oauth::exchange_code`] swaps an
    /// authorization code + PKCE verifier for an initial
    /// [`crate::oauth::TokenPair`]. No production connector
    /// reaches this variant between DAY-201 merge and DAY-202 merge
    /// (Outlook), so the refresh and exchange paths are exercised
    /// by `wiremock` integration tests only until then.
    OAuth {
        /// Issuer URL the tokens were minted against. Baked into the
        /// descriptor so a round trip through (descriptor → load tokens
        /// → rebuild strategy) is self-contained; the desktop's IPC
        /// layer never has to remember "which Entra tenant is this
        /// source wired to".
        ///
        /// For the Dayseam Outlook connector this is
        /// `https://login.microsoftonline.com/organizations/v2.0` — the
        /// work/school endpoint that blocks `outlook.com` /
        /// `hotmail.com` personal accounts by construction.
        issuer: String,
        /// Public client id from the Azure app registration. Not
        /// secret (it appears in the consent-URL query string) but
        /// committing-to: rotating it invalidates every existing user
        /// consent and re-prompts for re-auth.
        client_id: String,
        /// Delegated scopes granted at consent time. Recorded so a
        /// refresh call can re-ask for the same set; Microsoft may
        /// silently downgrade scopes if tenant admin policy has
        /// changed between consent and refresh, in which case the
        /// refresh response's `scope` field will differ and the
        /// orchestrator surfaces a "reconnect" prompt.
        scopes: Vec<String>,
        /// Keychain service name shared by the access + refresh rows.
        /// Conventionally `"dayseam.outlook"` for the v0.9 connector
        /// and `"dayseam.<provider>"` for any future OAuth provider.
        keychain_service: String,
        /// Keychain account for the access-token row. The SDK does not
        /// itself encode the `.oauth.access` suffix — the orchestrator
        /// picks and records the full account string when writing the
        /// row, and the descriptor reflects whatever was written so
        /// reads always match writes.
        access_keychain_account: String,
        /// Keychain account for the refresh-token row. Must be
        /// *different* from `access_keychain_account` so the two
        /// tokens sit under distinct rows — see struct-level docs for
        /// why isolation matters.
        refresh_keychain_account: String,
    },
}

/// An authentication strategy the connector asks to attach credentials
/// to a request. The trait is async because future impls (OAuth2
/// refresh, GitHub App JWT minting) will fetch/rotate tokens, even
/// though v0.1's PAT impl is synchronous under the hood.
#[async_trait]
pub trait AuthStrategy: Send + Sync + std::fmt::Debug {
    /// A short, stable name for this strategy — logged and shown to
    /// the user ("PAT", "OAuth 2.0", "GitHub App"). Renaming this
    /// is a user-visible change.
    fn name(&self) -> &'static str;

    /// Attach credentials to an outbound request. The returned
    /// [`reqwest::RequestBuilder`] is what the connector sends.
    /// Returning a `DayseamError` here surfaces as an `Auth` variant
    /// in the UI with the corresponding error code.
    async fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, DayseamError>;

    /// The durable descriptor — persisted in `SourceConfig`. A round
    /// trip through `descriptor()` + the matching constructor rebuilds
    /// an equivalent strategy (assuming the keychain still holds the
    /// secret).
    fn descriptor(&self) -> AuthDescriptor;
}

/// No-op auth for sources that do not require authentication — most
/// notably local git, which only touches the filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoneAuth;

#[async_trait]
impl AuthStrategy for NoneAuth {
    fn name(&self) -> &'static str {
        "none"
    }

    async fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, DayseamError> {
        Ok(request)
    }

    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor::None
    }
}

/// Personal access token attached via HTTP header. Which header depends
/// on the source (`PRIVATE-TOKEN` for GitLab, `Authorization: Bearer`
/// for Jira DC), so the header name is configurable at construction.
///
/// The token is wrapped in [`SecretString`] so (a) it never appears in
/// `{:?}` output — a risk at every `tracing` span boundary — and (b)
/// its bytes are zeroed when `PatAuth` is dropped, bounding how long a
/// PAT lives in process memory to the sync run's lifetime. `PatAuth`
/// intentionally does not implement `Clone`: duplicating a secret
/// should be a deliberate act, not a side-effect of passing the
/// strategy to a helper.
pub struct PatAuth {
    header_name: &'static str,
    header_value: SecretString,
    descriptor: AuthDescriptor,
}

impl PatAuth {
    /// Construct a PAT auth with GitLab's `PRIVATE-TOKEN` header shape.
    /// The descriptor records which keychain row the token came from.
    pub fn gitlab(
        token: impl Into<String>,
        keychain_service: impl Into<String>,
        keychain_account: impl Into<String>,
    ) -> Self {
        Self {
            header_name: "PRIVATE-TOKEN",
            header_value: SecretString::new(token.into()),
            descriptor: AuthDescriptor::Pat {
                keychain_service: keychain_service.into(),
                keychain_account: keychain_account.into(),
            },
        }
    }

    /// Generic bearer-token PAT (used by later connectors such as
    /// Jira Data Center). We bake the `Bearer ` prefix in at
    /// construction so the raw token is never materialised outside
    /// the [`SecretString`] after this call returns.
    pub fn bearer(
        token: impl Into<String>,
        keychain_service: impl Into<String>,
        keychain_account: impl Into<String>,
    ) -> Self {
        Self {
            header_name: "Authorization",
            header_value: SecretString::new(format!("Bearer {}", token.into())),
            descriptor: AuthDescriptor::Pat {
                keychain_service: keychain_service.into(),
                keychain_account: keychain_account.into(),
            },
        }
    }

    /// Construct a PAT auth with GitHub's `Authorization: Bearer <token>`
    /// header shape.
    ///
    /// This is a thin wrapper over [`PatAuth::bearer`] — GitHub's REST
    /// API accepts the same bearer shape Jira DC uses, so the
    /// implementation delegates. The reason `github(…)` exists as a
    /// distinct constructor, rather than every GitHub caller reaching
    /// for `bearer(…)` directly, is twofold:
    ///
    /// 1. **Discoverability.** A connector author scanning `PatAuth`'s
    ///    associated functions sees `gitlab`, `bearer`, `github` and
    ///    can pick by source kind without having to know GitHub's
    ///    header spelling. This mirrors the v0.1 `gitlab`/`bearer` pair
    ///    where `bearer` was left as the escape hatch and `gitlab` was
    ///    the named-by-source factory.
    /// 2. **Future-proofing.** If GitHub's auth shape diverges (e.g.
    ///    fine-grained PATs add a `X-GitHub-Api-Version` companion
    ///    header, or GitHub Enterprise requires a per-host `User-Agent`
    ///    override), the changes land inside this constructor without
    ///    touching callers. Keeping the named constructor is cheaper
    ///    than inlining `bearer(…)` into every call site and then
    ///    regretting it a quarter later.
    ///
    /// Token bytes are wrapped in a [`SecretString`] immediately; the
    /// raw `token` argument is consumed by `format!` inside `bearer`
    /// and does not outlive this call. See
    /// [`crate::dtos`](crate::dtos) for the persisted-vs-wire-format
    /// convention this connector family follows.
    pub fn github(
        token: impl Into<String>,
        keychain_service: impl Into<String>,
        keychain_account: impl Into<String>,
    ) -> Self {
        Self::bearer(token, keychain_service, keychain_account)
    }
}

// Manual `Debug` — the derived impl would have printed `header_value`
// unredacted via `String`'s `Debug`. `SecretString` already renders as
// `***`, but spelling the redaction out here defends against someone
// later swapping the field type back to a bare `String`.
impl std::fmt::Debug for PatAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatAuth")
            .field("header_name", &self.header_name)
            .field("header_value", &"***")
            .field("descriptor", &self.descriptor)
            .finish()
    }
}

#[async_trait]
impl AuthStrategy for PatAuth {
    fn name(&self) -> &'static str {
        "pat"
    }

    async fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, DayseamError> {
        // `expose` is the only reader; the resulting `&str` lives only
        // until `header()` copies it into `reqwest`'s internal buffer.
        Ok(request.header(self.header_name, self.header_value.expose()))
    }

    fn descriptor(&self) -> AuthDescriptor {
        self.descriptor.clone()
    }
}

/// HTTP Basic authentication with a pre-encoded `email:api_token` pair,
/// used by Atlassian Cloud (Jira + Confluence). The encoded header
/// value is baked at construction time so the plain `email:token`
/// bytes never outlive the constructor stack frame — what survives is
/// a single [`SecretString`] holding `"Basic <base64…>"`.
///
/// Each `BasicAuth` instance owns **one** keychain handle. In the
/// common shared-PAT case, two sources (one Jira, one Confluence) each
/// hold a `BasicAuth` whose descriptor points at the *same*
/// `(keychain_service, keychain_account)` row — both instances produce
/// the same `Authorization` header for the same live token. In the
/// separate-PAT case (different service accounts per product, or
/// tenants on different Atlassian instances), each source's
/// `BasicAuth` points at a *different* keychain row and the two
/// strategies are fully independent. The implementation does not care
/// which mode the caller picked — the sharing decision lives at the
/// IPC / DB layer (DAY-81's refcount) and not here.
///
/// Like [`PatAuth`], `BasicAuth` intentionally does not implement
/// `Clone`: duplicating a credential should be a deliberate act, not
/// a side-effect of passing the strategy to a helper.
pub struct BasicAuth {
    header_value: SecretString,
    descriptor: AuthDescriptor,
}

impl BasicAuth {
    /// Construct an Atlassian Cloud Basic-auth strategy.
    ///
    /// The `email`/`api_token` pair is base64-encoded per RFC 7617 and
    /// wrapped in a [`SecretString`] before the function returns; the
    /// plain `api_token` argument is consumed by
    /// [`String::into`]/`format!` and is not held in a named binding
    /// past this call. The descriptor records the keychain handle so a
    /// later round trip (app restart, `secret_ref` serialisation) can
    /// rebuild an equivalent strategy.
    pub fn atlassian(
        email: impl Into<String>,
        api_token: impl Into<String>,
        keychain_service: impl Into<String>,
        keychain_account: impl Into<String>,
    ) -> Self {
        let email = email.into();
        // `pair` is formatted once, encoded, then dropped at the end of
        // this expression; the encoded bytes live in `encoded` which
        // feeds directly into the final `SecretString`. We don't keep
        // `pair` in a named binding so it can't accidentally outlive
        // its single use.
        let encoded = BASE64_STANDARD.encode(format!("{}:{}", email, api_token.into()));
        let header_value = SecretString::new(format!("Basic {encoded}"));
        Self {
            header_value,
            descriptor: AuthDescriptor::Basic {
                email,
                keychain_service: keychain_service.into(),
                keychain_account: keychain_account.into(),
            },
        }
    }
}

// Manual `Debug` — derived would have reached inside `descriptor` to
// print `email` (fine; not secret) and `header_value` (not fine — it
// prints "***" via `SecretString`'s manual impl, but spelling the
// redaction out here defends against the field type being swapped
// back to a bare `String`).
impl std::fmt::Debug for BasicAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasicAuth")
            .field("header_value", &"***")
            .field("descriptor", &self.descriptor)
            .finish()
    }
}

#[async_trait]
impl AuthStrategy for BasicAuth {
    fn name(&self) -> &'static str {
        "basic"
    }

    async fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, DayseamError> {
        Ok(request.header("Authorization", self.header_value.expose()))
    }

    fn descriptor(&self) -> AuthDescriptor {
        self.descriptor.clone()
    }
}

/// OAuth 2.0 bearer-token authentication.
///
/// [`OAuthAuth::authenticate`] attaches the stored access token as a
/// `Bearer` header; if the token is past its `expires_at`, it first
/// calls [`OAuthAuth::refresh_if_expired`], which hits the IdP's
/// token endpoint with a `refresh_token` grant, rewrites the state
/// fields in place, and persists the new pair back through the
/// [`crate::oauth::TokenPersister`] callback before returning. The
/// refresh path is single-flighted by a `tokio::Mutex` so N
/// concurrent `authenticate()` calls on the same expired token
/// collapse into one IdP round-trip.
///
/// Construction: [`OAuthAuth::new`] takes a descriptor, the token
/// endpoint URL (orchestrator-derived — see `token_endpoint`
/// doc comment), a `reqwest::Client`, a shared persister, and a
/// [`Clock`]. A slimmer [`OAuthAuth::new_for_test`] is available
/// for unit tests that don't want to thread all four through.
///
/// Both tokens are wrapped in [`SecretString`] the instant they cross
/// the constructor boundary: `Debug` renders them as `***`, `Drop`
/// zeroes them. The mutable state lives behind a
/// [`tokio::sync::Mutex`] so interior mutability for refresh is
/// safe and single-flighted. `OAuthAuth` intentionally does not
/// implement `Clone` (matching [`PatAuth`] and [`BasicAuth`]):
/// duplicating a token pair should be a deliberate act, not a
/// side-effect of passing the strategy to a helper. Callers that
/// need two live copies reach into the keychain a second time
/// through [`dayseam_secrets::oauth::get_access_token`] and
/// [`dayseam_secrets::oauth::get_refresh_token`] and build a
/// second `OAuthAuth`.
///
/// The `access_expires_at` field is the **absolute** UTC instant at
/// which the token is considered expired — `authenticate()` treats
/// `now >= access_expires_at` as expired with zero skew compensation
/// for simplicity. A negative-skew policy (refresh proactively when
/// `expires_at - now < N`) is deliberately left out of v0.9 because
/// Microsoft Graph's 60–90 minute access-token lifetime already
/// gives a natural cushion and a proactive refresh would double the
/// IdP traffic on a cold start.
///
/// Terminal failure: if the refresh endpoint returns an RFC 6749
/// `invalid_grant` (user revoked consent, tenant admin removed the
/// grant, refresh token past its absolute lifetime, etc.), the
/// refresh path returns [`error_codes::OAUTH_REFRESH_REJECTED`] as
/// [`DayseamError::Auth`] with `retryable: false` — the UI renders
/// a Reconnect card and the user walks the consent dance again.
/// Mutable state that a refresh round-trip rewrites atomically.
/// Lives behind a [`tokio::sync::Mutex`] in [`OAuthAuth`] so the
/// check-expiry → call-IdP → write-state sequence is one critical
/// section, and so two concurrent `authenticate()` calls on the same
/// expired token collapse into exactly one network round-trip
/// (double-checked locking in async form).
struct OAuthState {
    access_token: SecretString,
    refresh_token: SecretString,
    access_expires_at: DateTime<Utc>,
}

pub struct OAuthAuth {
    state: Mutex<OAuthState>,
    descriptor: AuthDescriptor,
    /// The IdP's RFC 6749 token endpoint, fully qualified. Not on
    /// the descriptor because the descriptor is the *durable*
    /// identity of the credential (issuer + scopes + keychain rows)
    /// and the token endpoint is an orchestrator-supplied runtime
    /// derivation — for Microsoft Entra ID it's
    /// `https://login.microsoftonline.com/organizations/oauth2/v2.0/token`,
    /// derived from the descriptor's `issuer`. Future OAuth IdPs
    /// (hypothetical Google workspace integration, say) would
    /// derive it differently; baking Microsoft's layout into the
    /// descriptor would lock us in.
    token_endpoint: String,
    /// Our own HTTP client for hitting the token endpoint. Separate
    /// from the [`HttpClient`] any connector wraps around its
    /// resource-server traffic because (a) the token endpoint is a
    /// different host with a different retry policy and (b) we want
    /// refresh traffic to flow even if a connector's HttpClient is
    /// mid-retry-backoff. `reqwest::Client` is `Clone`-cheap and
    /// backed by an `Arc`, so holding one per `OAuthAuth` is fine.
    http: reqwest::Client,
    /// Keychain write-back callback. `Arc<dyn TokenPersister>` so
    /// the desktop-singleton persister can be shared across every
    /// `OAuthAuth` the orchestrator builds without each one holding
    /// an independent copy.
    persister: SharedPersister,
    /// Injectable wall clock for expiry checks and `expires_at`
    /// computation after a successful refresh. Production code
    /// passes `Arc::new(SystemClock)`; tests pass a mock clock so
    /// they can step past an `expires_at` deterministically.
    clock: Arc<dyn Clock>,
}

impl OAuthAuth {
    /// Construct an `OAuthAuth` from a freshly loaded token pair plus
    /// its matching descriptor. Returns an
    /// [`error_codes::OAUTH_DESCRIPTOR_MISMATCH`] if `descriptor` is
    /// not the [`AuthDescriptor::OAuth`] variant — the guard makes it
    /// impossible to accidentally wire a PAT descriptor to an OAuth
    /// strategy on a round trip through storage.
    ///
    /// Token bytes are consumed by the two `into()` calls and are not
    /// held in a named binding past this function; what survives is
    /// the two [`SecretString`] fields inside the state mutex.
    ///
    /// `token_endpoint` is the orchestrator-derived URL to POST both
    /// `authorization_code` and `refresh_token` grants against. For
    /// the v0.9 Outlook connector, the desktop computes it as
    /// `<issuer base>/oauth2/v2.0/token`; the SDK does not try to
    /// divine it from the descriptor so IdPs that don't follow
    /// Microsoft's URL convention can plug in without a trait
    /// rewrite later.
    ///
    /// Eight arguments is above the default clippy threshold (7),
    /// but bundling the last four into an `OAuthRuntime` struct
    /// would buy nothing: there is exactly one production caller
    /// (the orchestrator) and one test caller (`new_for_test`),
    /// and both already see these four as a conceptually distinct
    /// "ambient runtime" group at the call site. Allowing the lint
    /// here is deliberate and localised.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        access_expires_at: DateTime<Utc>,
        descriptor: AuthDescriptor,
        token_endpoint: impl Into<String>,
        http: reqwest::Client,
        persister: SharedPersister,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, DayseamError> {
        if !matches!(&descriptor, AuthDescriptor::OAuth { .. }) {
            return Err(DayseamError::InvalidConfig {
                code: error_codes::OAUTH_DESCRIPTOR_MISMATCH.to_string(),
                message: format!(
                    "OAuthAuth requires AuthDescriptor::OAuth, got a different variant: {descriptor:?}"
                ),
            });
        }
        Ok(Self {
            state: Mutex::new(OAuthState {
                access_token: SecretString::new(access_token.into()),
                refresh_token: SecretString::new(refresh_token.into()),
                access_expires_at,
            }),
            descriptor,
            token_endpoint: token_endpoint.into(),
            http,
            persister,
            clock,
        })
    }

    /// Test-oriented convenience constructor: builds an `OAuthAuth`
    /// with a default `reqwest::Client`, the [`SystemClock`], and
    /// the [`oauth::NoopTokenPersister`]. Behaves identically to
    /// [`OAuthAuth::new`] for descriptor validation and initial
    /// token storage, but sidesteps the four extra arguments that
    /// production construction needs. Kept `cfg(any(test, feature =
    /// "test-support"))`-free — it's a regular `pub` fn because the
    /// downstream Tauri integration test also uses it.
    pub fn new_for_test(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        access_expires_at: DateTime<Utc>,
        descriptor: AuthDescriptor,
    ) -> Result<Self, DayseamError> {
        Self::new(
            access_token,
            refresh_token,
            access_expires_at,
            descriptor,
            "https://example.invalid/oauth2/v2.0/token",
            reqwest::Client::new(),
            Arc::new(oauth::NoopTokenPersister),
            Arc::new(SystemClock),
        )
    }

    /// Refresh the access token iff it is past its `expires_at`.
    /// Safe to call concurrently: a `tokio::Mutex` on the state
    /// collapses N parallel callers on the same `OAuthAuth` into
    /// exactly one network round-trip, and the post-lock
    /// double-check means the second caller observes the refreshed
    /// token and returns immediately.
    ///
    /// On success, both the in-memory state and the persister's
    /// durable copy are updated before the call returns — there is
    /// no window in which the process can crash with a refreshed
    /// access token in memory and the old pair still in the
    /// keychain.
    ///
    /// On terminal IdP rejection (`invalid_grant` and friends),
    /// returns [`error_codes::OAUTH_REFRESH_REJECTED`] as
    /// [`DayseamError::Auth`] with `retryable: false`; the
    /// in-memory state is left unchanged so a later retry after the
    /// user re-consents can reuse the same `OAuthAuth` handle by
    /// rebuilding only its state.
    pub async fn refresh_if_expired(&self) -> Result<(), DayseamError> {
        let mut state = self.state.lock().await;

        // Double-check under the lock: a peer task may have already
        // refreshed us while we were queued on the mutex. This is
        // the single-flight collapse.
        if self.clock.now() < state.access_expires_at {
            return Ok(());
        }

        // Pull the scopes + client_id out of the descriptor — we
        // validated at construction that descriptor is the OAuth
        // variant, so the destructure below is infallible.
        let (client_id, scopes) = match &self.descriptor {
            AuthDescriptor::OAuth {
                client_id, scopes, ..
            } => (client_id.as_str(), scopes.as_slice()),
            _ => unreachable!("descriptor validated as OAuth at construction"),
        };

        let new_pair = oauth::run_refresh(
            &self.http,
            &self.token_endpoint,
            client_id,
            state.refresh_token.expose(),
            scopes,
            self.clock.as_ref(),
        )
        .await?;

        // Microsoft rotates refresh tokens on every grant; other IdPs
        // sometimes omit the field to mean "keep the old one".
        // `TokenPair::refresh_token` is empty in the latter case, so
        // only overwrite if the IdP actually gave us a new one.
        let persisted_refresh = if new_pair.refresh_token.is_empty() {
            state.refresh_token.expose().to_string()
        } else {
            new_pair.refresh_token.clone()
        };

        state.access_token = SecretString::new(new_pair.access_token.clone());
        if !new_pair.refresh_token.is_empty() {
            state.refresh_token = SecretString::new(new_pair.refresh_token.clone());
        }
        state.access_expires_at = new_pair.access_expires_at;

        // Persist *before* releasing the lock so a concurrent peer
        // that enters just after us always sees the refreshed state
        // backed by a durable write. The persister is expected to
        // cover both keychain rows in one logical operation.
        let to_persist = TokenPair {
            access_token: new_pair.access_token,
            refresh_token: persisted_refresh,
            access_expires_at: new_pair.access_expires_at,
            granted_scopes: new_pair.granted_scopes,
        };
        self.persister.persist_pair(&to_persist).await?;
        Ok(())
    }
}

// Manual Debug — the token state hides behind the mutex, and we
// explicitly render `***` for each field so a future refactor that
// exposes either token via a public accessor has to rewrite this
// impl rather than silently start leaking tokens via `tracing`
// spans. We also reach into the state via `try_lock` so the non-
// async Debug path never blocks; when the lock is contended we fall
// back to a "{locked}" marker (the interesting thing to a human
// reader of a log line is the descriptor anyway).
impl std::fmt::Debug for OAuthAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("OAuthAuth");
        dbg.field("descriptor", &self.descriptor);
        dbg.field("token_endpoint", &self.token_endpoint);
        match self.state.try_lock() {
            Ok(state) => {
                dbg.field("access_token", &state.access_token);
                dbg.field("refresh_token", &state.refresh_token);
                dbg.field("access_expires_at", &state.access_expires_at);
            }
            Err(_) => {
                dbg.field("state", &"{locked}");
            }
        }
        dbg.finish()
    }
}

#[async_trait]
impl AuthStrategy for OAuthAuth {
    fn name(&self) -> &'static str {
        "oauth2"
    }

    async fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, DayseamError> {
        // DAY-201: the DAY-200 expired-error branch becomes a
        // refresh-then-bearer call. `refresh_if_expired` is a no-op
        // when the token is still live; otherwise it single-flights
        // the refresh and rewrites state in place.
        self.refresh_if_expired().await?;
        let state = self.state.lock().await;
        Ok(request.bearer_auth(state.access_token.expose()))
    }

    fn descriptor(&self) -> AuthDescriptor {
        self.descriptor.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn none_auth_does_not_modify_request() {
        let client = reqwest::Client::new();
        let req = client.get("https://example.com");
        let out = NoneAuth.authenticate(req).await.expect("ok");
        let built = out.build().expect("build");
        assert!(
            !built.headers().contains_key("PRIVATE-TOKEN")
                && !built.headers().contains_key("Authorization"),
            "NoneAuth should not inject auth headers"
        );
    }

    #[tokio::test]
    async fn gitlab_pat_attaches_private_token_header() {
        let client = reqwest::Client::new();
        let req = client.get("https://gitlab.example/api/v4/user");
        let strat = PatAuth::gitlab("secret-123", "dayseam.gitlab", "acme");
        let out = strat.authenticate(req).await.expect("ok");
        let built = out.build().expect("build");
        assert_eq!(
            built
                .headers()
                .get("PRIVATE-TOKEN")
                .map(|v| v.to_str().unwrap()),
            Some("secret-123")
        );
        assert_eq!(strat.name(), "pat");
    }

    #[tokio::test]
    async fn bearer_pat_attaches_authorization_header() {
        let client = reqwest::Client::new();
        let req = client.get("https://jira.example/rest/api/2/myself");
        let strat = PatAuth::bearer("secret-xyz", "dayseam.jira", "acme");
        let out = strat.authenticate(req).await.expect("ok");
        let built = out.build().expect("build");
        assert_eq!(
            built
                .headers()
                .get("Authorization")
                .map(|v| v.to_str().unwrap()),
            Some("Bearer secret-xyz")
        );
    }

    #[test]
    fn descriptor_round_trips_keychain_handle() {
        let strat = PatAuth::gitlab("t", "svc", "acct");
        assert_eq!(
            strat.descriptor(),
            AuthDescriptor::Pat {
                keychain_service: "svc".into(),
                keychain_account: "acct".into(),
            }
        );
        assert_eq!(NoneAuth.descriptor(), AuthDescriptor::None);
    }

    #[test]
    fn debug_does_not_leak_token() {
        let strat = PatAuth::gitlab("super-secret-pat", "svc", "acct");
        let rendered = format!("{strat:?}");
        assert!(
            !rendered.contains("super-secret-pat"),
            "PAT leaked via Debug: {rendered}"
        );
        assert!(rendered.contains("***"), "missing redaction: {rendered}");
    }

    #[test]
    fn bearer_debug_does_not_leak_token() {
        let strat = PatAuth::bearer("super-secret-bearer", "svc", "acct");
        let rendered = format!("{strat:?}");
        assert!(
            !rendered.contains("super-secret-bearer"),
            "bearer token leaked via Debug: {rendered}"
        );
        // The `Bearer ` prefix also lives inside the `Secret`, so it
        // must not appear in the redacted output either.
        assert!(
            !rendered.contains("Bearer "),
            "header prefix leaked via Debug: {rendered}"
        );
    }

    // --------------------------------------------------------------
    // DAY-94 — PatAuth::github (GitHub REST / Enterprise Server)
    // --------------------------------------------------------------

    /// GitHub documents `Authorization: Bearer <token>` as the forward
    /// shape for classic PATs, fine-grained PATs, and GitHub App
    /// installation tokens. `github(…)` delegates to `bearer(…)`, so a
    /// regression that changes either the header name or the
    /// `"Bearer "` prefix is caught here rather than at integration
    /// time against the live API.
    #[tokio::test]
    async fn github_pat_attaches_bearer_authorization_header() {
        let client = reqwest::Client::new();
        let req = client.get("https://api.github.com/user");
        let strat = PatAuth::github("ghp_abc123", "dayseam.github", "acme");
        let out = strat.authenticate(req).await.expect("ok");
        let built = out.build().expect("build");
        assert_eq!(
            built
                .headers()
                .get("Authorization")
                .map(|v| v.to_str().unwrap()),
            Some("Bearer ghp_abc123")
        );
        // The strategy name is still `pat` (not `github`): the trait's
        // `name()` is the strategy-family label the UI renders, not a
        // per-source discriminator. Renaming it would be a
        // user-visible change and is explicitly out of scope.
        assert_eq!(strat.name(), "pat");
    }

    /// `Debug` on a GitHub `PatAuth` must not leak either the raw
    /// `ghp_*` token bytes or the baked `Bearer <token>` header value.
    /// This is the same invariant [`bearer_debug_does_not_leak_token`]
    /// asserts for the generic constructor — spelled out separately so
    /// a future refactor that carves `github(…)` into its own struct
    /// has to re-prove it rather than inheriting the guarantee
    /// implicitly.
    #[test]
    fn github_pat_debug_does_not_leak_token() {
        let strat = PatAuth::github("ghp_super_secret_token", "dayseam.github", "acme");
        let rendered = format!("{strat:?}");
        assert!(
            !rendered.contains("ghp_super_secret_token"),
            "GitHub PAT leaked via Debug: {rendered}"
        );
        assert!(
            !rendered.contains("Bearer "),
            "Bearer prefix leaked via Debug: {rendered}"
        );
        assert!(rendered.contains("***"), "missing redaction: {rendered}");
    }

    /// Descriptor round-trip: a `PatAuth::github` with keychain handle
    /// `(svc, acct)` deserialises back into a matching
    /// `AuthDescriptor::Pat { keychain_service: "svc", keychain_account:
    /// "acct" }`. The `Pat` descriptor variant is shared across all
    /// PAT-shaped strategies (GitLab PRIVATE-TOKEN, Jira DC bearer,
    /// GitHub bearer) — the `AuthDescriptor` enum deliberately does
    /// not carry a per-source discriminator because the keychain
    /// handle is already a unique-per-source identifier.
    #[test]
    fn github_pat_descriptor_round_trips_keychain_handle() {
        let strat = PatAuth::github("ghp_t", "dayseam.github", "acme");
        assert_eq!(
            strat.descriptor(),
            AuthDescriptor::Pat {
                keychain_service: "dayseam.github".into(),
                keychain_account: "acme".into(),
            }
        );
    }

    /// Two `PatAuth::github` instances sharing the same keychain handle
    /// (e.g. a user connecting two GitHub Enterprise Server hosts that
    /// happen to use the same service account) round-trip into equal
    /// descriptors. The DB-level `secret_ref` uniqueness guard
    /// (DAY-73 `0005`) hangs off this invariant; a regression that
    /// made two identical handles compare unequal would let the same
    /// token bytes be written twice into the keychain.
    #[test]
    fn github_pat_same_keychain_handle_produces_equal_descriptors() {
        let a = PatAuth::github("token-A", "dayseam.github", "acme");
        let b = PatAuth::github("token-B", "dayseam.github", "acme");
        assert_eq!(a.descriptor(), b.descriptor());
    }

    // --------------------------------------------------------------
    // DAY-74 — BasicAuth (Atlassian Cloud)
    // --------------------------------------------------------------

    /// RFC 7617 §2: "The user-id and password MUST NOT contain any
    /// control characters […] the user-id and password is combined
    /// with a single colon (":") character. Within the header field,
    /// the resulting string is encoded using Base64". The Atlassian
    /// spike (`docs/spikes/2026-04-20-atlassian-connectors-data-shape.md`
    /// §3.1) pinned this exact shape.
    #[tokio::test]
    async fn basic_auth_header_is_base64_email_colon_token() {
        let strat = BasicAuth::atlassian("foo@example.com", "bar", "svc", "acct");
        let client = reqwest::Client::new();
        let req = client.get("https://company.atlassian.net/rest/api/3/myself");
        let out = strat.authenticate(req).await.expect("ok");
        let built = out.build().expect("build");
        assert_eq!(
            built
                .headers()
                .get("Authorization")
                .map(|v| v.to_str().unwrap()),
            // base64("foo@example.com:bar") == "Zm9vQGV4YW1wbGUuY29tOmJhcg=="
            Some("Basic Zm9vQGV4YW1wbGUuY29tOmJhcg==")
        );
        assert_eq!(strat.name(), "basic");
    }

    /// `Debug` must redact both the encoded header value *and* prove
    /// that the plain-text `email:token` bytes never re-emerge — a
    /// regression here would show up in every `tracing` span that
    /// carries an `AuthStrategy` handle.
    #[test]
    fn basic_auth_debug_does_not_leak_token_or_encoded_header() {
        let strat = BasicAuth::atlassian(
            "user@company.com",
            "super-secret-api-token",
            "dayseam.atlassian",
            "acme",
        );
        let rendered = format!("{strat:?}");
        assert!(
            !rendered.contains("super-secret-api-token"),
            "plain api_token leaked via Debug: {rendered}"
        );
        // The full `Basic <base64>` header value must also not appear.
        // We check the canonical base64 of `user@company.com:super-secret-api-token`.
        let plain = "user@company.com:super-secret-api-token";
        let encoded = BASE64_STANDARD.encode(plain);
        assert!(
            !rendered.contains(&encoded),
            "encoded header leaked via Debug: {rendered}"
        );
        assert!(rendered.contains("***"), "missing redaction: {rendered}");
        // `email` is plain (not secret) and is expected to appear via
        // the descriptor; spell that out so a future refactor that
        // moves email into the SecretString has to justify why.
        assert!(
            rendered.contains("user@company.com"),
            "descriptor should still render the (non-secret) email: {rendered}"
        );
    }

    /// Two `BasicAuth` instances with the same `(service, account)`
    /// pair round-trip into identical descriptors — this is the
    /// shared-PAT invariant the DAY-81 refcount guard hangs off. The
    /// sharing decision lives at the IPC/DB layer; here we only prove
    /// that the auth strategy itself is shape-equal in the shared
    /// case and shape-distinct in the separate case.
    #[test]
    fn basic_auth_same_keychain_handle_produces_equal_descriptors() {
        let a = BasicAuth::atlassian("shared@company.com", "token-A", "dayseam.atlassian", "acme");
        let b = BasicAuth::atlassian(
            "shared@company.com",
            "token-B", // different live token value, same keychain handle
            "dayseam.atlassian",
            "acme",
        );
        assert_eq!(
            a.descriptor(),
            b.descriptor(),
            "two sources pointing at the same keychain row should serialise identically"
        );
    }

    /// The separate-PAT mode: two sources referencing different
    /// keychain rows (different service or account) must produce
    /// distinct descriptors so the DB-level `secret_ref`s don't get
    /// accidentally unified on write. This is the invariant that lets
    /// companies with per-product service accounts use Dayseam without
    /// leaking one product's PAT into the other product's error
    /// surface.
    #[test]
    fn basic_auth_different_keychain_handles_stay_independent() {
        let jira = BasicAuth::atlassian(
            "jira-bot@company.com",
            "jira-token",
            "dayseam.atlassian",
            "acme-jira",
        );
        let confluence = BasicAuth::atlassian(
            "confluence-bot@company.com",
            "confluence-token",
            "dayseam.atlassian",
            "acme-confluence",
        );
        assert_ne!(
            jira.descriptor(),
            confluence.descriptor(),
            "separate keychain rows must serialise distinctly"
        );
        // Also prove the emails round-trip, since each source can have
        // a product-specific service-account email.
        match jira.descriptor() {
            AuthDescriptor::Basic { email, .. } => {
                assert_eq!(email, "jira-bot@company.com");
            }
            other => panic!("expected Basic descriptor, got {other:?}"),
        }
        match confluence.descriptor() {
            AuthDescriptor::Basic { email, .. } => {
                assert_eq!(email, "confluence-bot@company.com");
            }
            other => panic!("expected Basic descriptor, got {other:?}"),
        }
    }

    /// Descriptor round-trips preserve the three fields that together
    /// identify the credential row: email (for display), service +
    /// account (for keychain lookup).
    #[test]
    fn basic_auth_descriptor_round_trips_all_three_fields() {
        let strat = BasicAuth::atlassian("u@e.com", "t", "svc", "acct");
        assert_eq!(
            strat.descriptor(),
            AuthDescriptor::Basic {
                email: "u@e.com".to_string(),
                keychain_service: "svc".to_string(),
                keychain_account: "acct".to_string(),
            }
        );
    }

    /// Unicode in the email (some Atlassian Cloud tenants allow
    /// non-ASCII addresses for SSO-federated accounts) must survive
    /// base64 encoding intact — a silent UTF-8 corruption here would
    /// surface as `401 invalid_credentials` with a message that points
    /// at the wrong root cause.
    #[tokio::test]
    async fn basic_auth_preserves_utf8_in_email_and_token() {
        let email = "vedanth.vasudev+🌊@company.com";
        let token = "token-with-🔑-emoji";
        let strat = BasicAuth::atlassian(email, token, "svc", "acct");
        let client = reqwest::Client::new();
        let out = strat
            .authenticate(client.get("https://example.invalid"))
            .await
            .expect("ok");
        let built = out.build().expect("build");
        let header = built
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .expect("header present");
        let expected = format!(
            "Basic {}",
            BASE64_STANDARD.encode(format!("{email}:{token}"))
        );
        assert_eq!(header, expected);
    }

    // --------------------------------------------------------------
    // DAY-200 / DAY-201 — OAuthAuth
    // --------------------------------------------------------------

    use chrono::Duration as ChronoDuration;

    fn sample_oauth_descriptor() -> AuthDescriptor {
        AuthDescriptor::OAuth {
            issuer: "https://login.microsoftonline.com/organizations/v2.0".into(),
            client_id: "00000000-0000-0000-0000-000000000000".into(),
            scopes: vec![
                "offline_access".into(),
                "Calendars.Read".into(),
                "User.Read".into(),
            ],
            keychain_service: "dayseam.outlook".into(),
            access_keychain_account: "user@contoso.com.oauth.access".into(),
            refresh_keychain_account: "user@contoso.com.oauth.refresh".into(),
        }
    }

    /// The happy path: an unexpired token renders into a standard
    /// `Authorization: Bearer <token>` header.
    /// `reqwest::RequestBuilder::bearer_auth` produces the canonical
    /// spelling, so we just assert the output header value matches.
    /// DAY-201 keeps this assertion identical to DAY-200 so the
    /// refactor does not silently change the live-token header.
    #[tokio::test]
    async fn oauth_auth_attaches_bearer_when_not_expired() {
        let strat = OAuthAuth::new_for_test(
            "access-token-abc",
            "refresh-token-xyz",
            Utc::now() + ChronoDuration::hours(1),
            sample_oauth_descriptor(),
        )
        .expect("valid descriptor");
        let client = reqwest::Client::new();
        let req = client.get("https://graph.microsoft.com/v1.0/me/calendar");
        let out = strat.authenticate(req).await.expect("bearer attaches");
        let built = out.build().expect("build");
        assert_eq!(
            built
                .headers()
                .get("Authorization")
                .map(|v| v.to_str().unwrap()),
            Some("Bearer access-token-abc"),
            "expected canonical Bearer header from reqwest::bearer_auth"
        );
        assert_eq!(strat.name(), "oauth2");
    }

    /// `OAuthAuth::new` returns an [`error_codes::OAUTH_DESCRIPTOR_MISMATCH`]
    /// error if handed a non-OAuth descriptor — the guard catches
    /// orchestrator bugs where a PAT or Basic descriptor was
    /// accidentally routed through the OAuth constructor on a round
    /// trip through storage. Surfaced as `InvalidConfig` (a
    /// programming error) rather than `Auth` (a user-facing credential
    /// failure).
    #[test]
    fn oauth_auth_rejects_non_oauth_descriptors() {
        for bad in [
            AuthDescriptor::None,
            AuthDescriptor::Pat {
                keychain_service: "svc".into(),
                keychain_account: "acct".into(),
            },
            AuthDescriptor::Basic {
                email: "u@e.com".into(),
                keychain_service: "svc".into(),
                keychain_account: "acct".into(),
            },
        ] {
            let err = OAuthAuth::new_for_test(
                "a",
                "r",
                Utc::now() + ChronoDuration::hours(1),
                bad.clone(),
            )
            .expect_err(&format!("expected rejection for {bad:?}"));
            match err {
                DayseamError::InvalidConfig { code, .. } => {
                    assert_eq!(code, error_codes::OAUTH_DESCRIPTOR_MISMATCH);
                }
                other => panic!("expected InvalidConfig for {bad:?}, got {other:?}"),
            }
        }
    }

    /// `Debug` on `OAuthAuth` must redact both tokens — a regression
    /// here would show up in every `tracing` span that carries an
    /// `OAuthAuth` handle. The descriptor's non-secret fields
    /// (`client_id`, `issuer`, keychain handles) are expected to
    /// render; the access and refresh token *values* must not.
    #[test]
    fn oauth_auth_debug_does_not_leak_either_token() {
        let strat = OAuthAuth::new_for_test(
            "super-secret-access-token-value",
            "super-secret-refresh-token-value",
            Utc::now() + ChronoDuration::hours(1),
            sample_oauth_descriptor(),
        )
        .expect("valid descriptor");
        let rendered = format!("{strat:?}");
        assert!(
            !rendered.contains("super-secret-access-token-value"),
            "access token leaked via Debug: {rendered}"
        );
        assert!(
            !rendered.contains("super-secret-refresh-token-value"),
            "refresh token leaked via Debug: {rendered}"
        );
        assert!(
            rendered.contains("***"),
            "missing redaction marker in: {rendered}"
        );
        // Descriptor fields are *not* secret — the public client_id
        // shows in the consent-URL query string and is expected to
        // render in debug output. If a future refactor moves
        // client_id into the secret area, this assertion forces a
        // conscious decision.
        assert!(
            rendered.contains("00000000-0000-0000-0000-000000000000"),
            "descriptor's public client_id should still render: {rendered}"
        );
    }

    /// Descriptor round-trip: an `OAuthAuth` built around a given
    /// `AuthDescriptor::OAuth` round-trips into an equal descriptor
    /// via `descriptor()`. Rebuilding the strategy from the descriptor
    /// alone plus the keychain rows it points at relies on this
    /// identity.
    #[test]
    fn oauth_auth_descriptor_round_trips() {
        let desc = sample_oauth_descriptor();
        let strat = OAuthAuth::new_for_test(
            "a",
            "r",
            Utc::now() + ChronoDuration::hours(1),
            desc.clone(),
        )
        .expect("valid descriptor");
        assert_eq!(strat.descriptor(), desc);
    }

    /// Two `OAuthAuth` instances with the same descriptor but
    /// different live tokens still round-trip into equal descriptors
    /// — mirrors the `basic_auth_same_keychain_handle_produces_equal_descriptors`
    /// invariant for the OAuth shape. The durable identity is the
    /// descriptor; the tokens themselves are ephemeral.
    #[test]
    fn oauth_auth_same_descriptor_produces_equal_descriptors() {
        let desc = sample_oauth_descriptor();
        let a = OAuthAuth::new_for_test(
            "access-A",
            "refresh-A",
            Utc::now() + ChronoDuration::hours(1),
            desc.clone(),
        )
        .expect("a");
        let b = OAuthAuth::new_for_test(
            "access-B",
            "refresh-B",
            Utc::now() + ChronoDuration::hours(2),
            desc.clone(),
        )
        .expect("b");
        assert_eq!(
            a.descriptor(),
            b.descriptor(),
            "same descriptor, different tokens ⇒ equal descriptor round-trip"
        );
    }

    /// `refresh_if_expired` is a no-op when the access token is still
    /// live — it acquires the state mutex, double-checks expiry, and
    /// returns without ever touching the HTTP client. Proves the
    /// single-flight pattern's fast path works before any real
    /// wiremock traffic. Uses the test constructor, which wires a
    /// `NoopTokenPersister` and a deliberately-invalid token endpoint
    /// — a live refresh attempt would fail with a transport error, so
    /// `Ok(())` is meaningful evidence that the fast path fired.
    #[tokio::test]
    async fn oauth_auth_refresh_is_noop_when_fresh() {
        let strat = OAuthAuth::new_for_test(
            "fresh-access",
            "fresh-refresh",
            Utc::now() + ChronoDuration::hours(1),
            sample_oauth_descriptor(),
        )
        .expect("valid descriptor");
        strat
            .refresh_if_expired()
            .await
            .expect("non-expired token must never trigger a network call");
    }

    /// Two descriptors with different access accounts (e.g. two
    /// Outlook connections for two different work users on the same
    /// tenant) must serialise distinctly — without this guard, the
    /// orchestrator's per-source secret-ref uniqueness check could
    /// silently unify the rows and hand user-A's access token to
    /// user-B's connector instance.
    #[test]
    fn oauth_auth_different_account_strings_stay_independent() {
        let alice = AuthDescriptor::OAuth {
            issuer: "https://login.microsoftonline.com/organizations/v2.0".into(),
            client_id: "client-id".into(),
            scopes: vec!["Calendars.Read".into()],
            keychain_service: "dayseam.outlook".into(),
            access_keychain_account: "alice@acme.com.oauth.access".into(),
            refresh_keychain_account: "alice@acme.com.oauth.refresh".into(),
        };
        let bob = AuthDescriptor::OAuth {
            issuer: "https://login.microsoftonline.com/organizations/v2.0".into(),
            client_id: "client-id".into(),
            scopes: vec!["Calendars.Read".into()],
            keychain_service: "dayseam.outlook".into(),
            access_keychain_account: "bob@acme.com.oauth.access".into(),
            refresh_keychain_account: "bob@acme.com.oauth.refresh".into(),
        };
        assert_ne!(alice, bob);
    }
}
