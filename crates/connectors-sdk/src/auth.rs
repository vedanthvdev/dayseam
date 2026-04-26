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

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use dayseam_core::{error_codes, DayseamError};
use zeroize::Zeroize;

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
struct SecretString(String);

impl SecretString {
    fn new(value: String) -> Self {
        Self(value)
    }

    fn expose(&self) -> &str {
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
    /// DAY-200 ships this variant alongside a scaffold [`OAuthAuth`]
    /// impl that attaches the access token as a bearer header when
    /// non-expired and returns a specific `oauth.token_expired`
    /// [`DayseamError::Auth`] otherwise. Code-for-token exchange and
    /// automatic refresh land in DAY-201; no production connector uses
    /// this variant between DAY-200 merge and DAY-202 merge (Outlook),
    /// so the unrefreshable path is exercised by tests only.
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

/// OAuth 2.0 bearer-token authentication. DAY-200 ships this as a
/// **scaffold** — [`OAuthAuth::authenticate`] attaches the stored
/// access token as a `Bearer` header when the token is still within
/// its lifetime and returns a specific
/// [`error_codes::OAUTH_TOKEN_EXPIRED`] [`DayseamError::Auth`] when it
/// isn't. Token refresh and the initial code-for-token exchange land
/// in DAY-201.
///
/// No production connector reaches this code between DAY-200 merge
/// and DAY-202 merge (Outlook) — the first OAuth connector's
/// first-run consent path produces a non-expired token by
/// construction, and DAY-201 replaces the expired-error branch with a
/// refresh-then-bearer happy path before any connector actually calls
/// `authenticate()` with an aged token. The expired-error branch is
/// exercised by unit tests only in this PR, and is kept as a real
/// error (not an `unreachable!`) so tests can prove the error code is
/// stable and DAY-201's patch is a straight substitution rather than
/// an enum rewrite.
///
/// Both tokens are wrapped in [`SecretString`] the instant they cross
/// the constructor boundary: `Debug` renders them as `***`, `Drop`
/// zeroes them. `OAuthAuth` intentionally does not implement `Clone`
/// (matching [`PatAuth`] and [`BasicAuth`]): duplicating a token pair
/// should be a deliberate act, not a side-effect of passing the
/// strategy to a helper. Callers that need two live copies reach into
/// the keychain a second time through
/// [`dayseam_secrets::oauth::get_access_token`] and
/// [`dayseam_secrets::oauth::get_refresh_token`].
///
/// The `access_expires_at` field is the **absolute** UTC instant at
/// which the token is considered expired — `authenticate()` treats
/// `now >= access_expires_at` as expired with zero skew compensation.
/// A negative-skew policy (refresh proactively when `expires_at - now
/// < N`) is a DAY-201 decision; baking a skew window into the
/// scaffold would be a premature call that DAY-201 would then have to
/// unpick.
pub struct OAuthAuth {
    access_token: SecretString,
    refresh_token: SecretString,
    access_expires_at: DateTime<Utc>,
    descriptor: AuthDescriptor,
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
    /// the two [`SecretString`] fields.
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        access_expires_at: DateTime<Utc>,
        descriptor: AuthDescriptor,
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
            access_token: SecretString::new(access_token.into()),
            refresh_token: SecretString::new(refresh_token.into()),
            access_expires_at,
            descriptor,
        })
    }

    /// Whether the stored access token is past its `expires_at`
    /// instant. Uses the provided `now` rather than [`Utc::now`] so
    /// tests can exercise the boundary deterministically without
    /// threading a `Clock` through the constructor — the DAY-201
    /// refresh implementation is the right place to wire a real
    /// [`crate::Clock`] in if needed.
    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.access_expires_at
    }
}

// Manual Debug — derives would delegate to `SecretString`'s own
// `Debug`, which already prints `***`; we also reference the fields
// directly here so the compiler can't mistake them for dead code, and
// so a future refactor that swaps either field's type back to a bare
// `String` has to rewrite this impl rather than silently start
// leaking tokens via `tracing` spans.
impl std::fmt::Debug for OAuthAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthAuth")
            .field("access_token", &self.access_token)
            .field("refresh_token", &self.refresh_token)
            .field("access_expires_at", &self.access_expires_at)
            .field("descriptor", &self.descriptor)
            .finish()
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
        // DAY-200 scaffold: no refresh logic. DAY-201 will replace the
        // conditional below with a call into `refresh_if_expired(&mut
        // self, ...)`; the error path here is the short-term contract
        // for "someone reached for a token this build can't refresh".
        // Tests exercise both branches so the DAY-201 patch is a
        // straight code swap, not a behaviour change.
        if self.is_expired(Utc::now()) {
            return Err(DayseamError::Auth {
                code: error_codes::OAUTH_TOKEN_EXPIRED.to_string(),
                message:
                    "access token expired; automatic refresh is not yet wired in this SDK build"
                        .to_string(),
                retryable: false,
                action_hint: Some("re-authenticate via the source's Reconnect flow".to_string()),
            });
        }
        Ok(request.bearer_auth(self.access_token.expose()))
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
    // DAY-200 — OAuthAuth scaffold
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

    /// The happy path for the DAY-200 scaffold: an unexpired token
    /// renders into a standard `Authorization: Bearer <token>` header.
    /// `reqwest::RequestBuilder::bearer_auth` produces the canonical
    /// spelling, so we just assert the output header value matches.
    #[tokio::test]
    async fn oauth_auth_attaches_bearer_when_not_expired() {
        let strat = OAuthAuth::new(
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

    /// The expired-token path returns a specific
    /// [`error_codes::OAUTH_TOKEN_EXPIRED`] error. DAY-201 will swap
    /// this branch for a refresh-then-bearer happy path, so pinning
    /// the error code here means DAY-201's migration is a straight
    /// substitution — the test breaks loudly if the replacement
    /// accidentally changes the emitted code (e.g. to a
    /// connector-specific one) without a matching UI copy update.
    #[tokio::test]
    async fn oauth_auth_errors_when_expired_with_stable_code() {
        let strat = OAuthAuth::new(
            "stale-access-token",
            "refresh-token",
            Utc::now() - ChronoDuration::seconds(1),
            sample_oauth_descriptor(),
        )
        .expect("valid descriptor");
        let client = reqwest::Client::new();
        let err = strat
            .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
            .await
            .expect_err("expired token must error");
        match err {
            DayseamError::Auth {
                code,
                retryable,
                action_hint,
                ..
            } => {
                assert_eq!(code, error_codes::OAUTH_TOKEN_EXPIRED);
                assert!(
                    !retryable,
                    "an expired token is a re-auth trigger, not a retry"
                );
                assert!(
                    action_hint.is_some(),
                    "UI needs a non-empty action_hint to render the Reconnect card"
                );
            }
            other => panic!("expected Auth variant, got {other:?}"),
        }
    }

    /// Exactly-at-expiry is treated as expired (`now >=
    /// access_expires_at`, not strictly greater-than), matching the
    /// boundary Microsoft's `expires_in` field implies when the
    /// token's second of grace has already ticked over.
    #[tokio::test]
    async fn oauth_auth_treats_exact_expiry_as_expired() {
        let now = Utc::now();
        let strat = OAuthAuth::new(
            "token",
            "refresh",
            now, // exactly now — must be treated as expired
            sample_oauth_descriptor(),
        )
        .expect("valid descriptor");
        let client = reqwest::Client::new();
        let err = strat
            .authenticate(client.get("https://graph.microsoft.com/v1.0/me"))
            .await
            .expect_err("exact-expiry must error");
        assert_eq!(err.code(), error_codes::OAUTH_TOKEN_EXPIRED);
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
            let err = OAuthAuth::new("a", "r", Utc::now() + ChronoDuration::hours(1), bad.clone())
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
        let strat = OAuthAuth::new(
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
    /// via `descriptor()`. DAY-201's refresh code will rebuild the
    /// strategy from the descriptor alone plus the keychain rows it
    /// points at, so this identity is load-bearing.
    #[test]
    fn oauth_auth_descriptor_round_trips() {
        let desc = sample_oauth_descriptor();
        let strat = OAuthAuth::new(
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
        let a = OAuthAuth::new(
            "access-A",
            "refresh-A",
            Utc::now() + ChronoDuration::hours(1),
            desc.clone(),
        )
        .expect("a");
        let b = OAuthAuth::new(
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
