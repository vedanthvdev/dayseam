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
use dayseam_core::DayseamError;
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
}
