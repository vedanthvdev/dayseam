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
}
