//! Typed conversions from connector-local failure modes to the seven
//! `gitlab.*` codes in [`dayseam_core::error_codes`].
//!
//! The registry itself lives in `dayseam-core`; this module is the
//! bridge that turns a transport error, an HTTP status, or a serde
//! decode failure into the right [`DayseamError`] variant + stable
//! code so downstream log parsers, UI copy, and the error-card surface
//! (Task 3) have a single source of truth.

use dayseam_core::{error_codes, DayseamError};
use reqwest::StatusCode;

/// Connector-local categorisation of failure modes before we map them
/// to [`DayseamError`]. Not a public trait; it just gives the
/// [`map_status`] helper a structured switch to drive off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitlabUpstreamError {
    /// 401 — token rejected. `Auth` variant, non-retryable, hints at
    /// the Reconnect flow Task 3 will own.
    AuthInvalidToken,
    /// 403 — token authenticates but lacks the `read_api` scope (the
    /// minimum the Events API demands). Same variant, different code,
    /// same hint.
    AuthMissingScope,
    /// DNS resolution failed for the configured host.
    UrlDns { message: String },
    /// TLS handshake failed (bad cert, mismatched hostname, etc.).
    UrlTls { message: String },
    /// 429 — server asked us to slow down. The connector's rate-limit
    /// loop surfaces this before ever constructing a `DayseamError`;
    /// mapped here only when the retry budget is exhausted.
    RateLimited { retry_after_secs: u64 },
    /// 5xx that survived the retry budget.
    Upstream5xx { status: StatusCode, message: String },
    /// Response decoded but carried a shape the connector doesn't know
    /// how to interpret — an unknown target type, a missing required
    /// field. Produces [`error_codes::GITLAB_UPSTREAM_SHAPE_CHANGED`]
    /// so the UI can tell "connector bug" from "credentials are wrong".
    ShapeChanged { message: String },
    /// CONS-v0.2-02. 404 on a GitLab endpoint — the resource (project,
    /// group, user) isn't reachable. Split out from the catch-all so
    /// the UI surfaces "check the URL / scope" rather than
    /// "upstream is down", and so the code matches the atlassian
    /// 404 arm at the taxonomy level. Maps to
    /// [`error_codes::GITLAB_RESOURCE_NOT_FOUND`] on the
    /// [`DayseamError::Network`] variant.
    ResourceNotFound { message: String },
    /// DAY-89 CONS-v0.2-06. 410 Gone — the project, MR, or issue has
    /// been hard-deleted. Distinct from 404 (which can be a permissions
    /// race) so the orchestrator never retries. Maps to
    /// [`error_codes::GITLAB_RESOURCE_GONE`] on the
    /// [`DayseamError::Network`] variant. Symmetric with
    /// Atlassian's `ResourceGone`.
    ResourceGone { message: String },
}

impl From<GitlabUpstreamError> for DayseamError {
    fn from(value: GitlabUpstreamError) -> Self {
        match value {
            GitlabUpstreamError::AuthInvalidToken => DayseamError::Auth {
                code: error_codes::GITLAB_AUTH_INVALID_TOKEN.to_string(),
                message: "GitLab rejected the personal access token".to_string(),
                retryable: false,
                action_hint: Some(
                    "Open Settings, select this source, and click Reconnect to paste a fresh PAT."
                        .to_string(),
                ),
            },
            GitlabUpstreamError::AuthMissingScope => DayseamError::Auth {
                code: error_codes::GITLAB_AUTH_MISSING_SCOPE.to_string(),
                message: "PAT is valid but missing the `read_api` scope required by Dayseam"
                    .to_string(),
                retryable: false,
                action_hint: Some(
                    "Generate a fresh PAT with the `read_api` scope and reconnect this source."
                        .to_string(),
                ),
            },
            GitlabUpstreamError::UrlDns { message } => DayseamError::Network {
                code: error_codes::GITLAB_URL_DNS.to_string(),
                message,
            },
            GitlabUpstreamError::UrlTls { message } => DayseamError::Network {
                code: error_codes::GITLAB_URL_TLS.to_string(),
                message,
            },
            GitlabUpstreamError::RateLimited { retry_after_secs } => DayseamError::RateLimited {
                code: error_codes::GITLAB_RATE_LIMITED.to_string(),
                retry_after_secs,
            },
            GitlabUpstreamError::Upstream5xx { status, message } => DayseamError::Network {
                code: error_codes::GITLAB_UPSTREAM_5XX.to_string(),
                message: format!("GitLab returned {status}: {message}"),
            },
            GitlabUpstreamError::ShapeChanged { message } => DayseamError::UpstreamChanged {
                code: error_codes::GITLAB_UPSTREAM_SHAPE_CHANGED.to_string(),
                message,
            },
            GitlabUpstreamError::ResourceNotFound { message } => DayseamError::Network {
                code: error_codes::GITLAB_RESOURCE_NOT_FOUND.to_string(),
                message,
            },
            GitlabUpstreamError::ResourceGone { message } => DayseamError::Network {
                code: error_codes::GITLAB_RESOURCE_GONE.to_string(),
                message: format!("GitLab resource returned 410 Gone: {message}"),
            },
        }
    }
}

/// Map a non-success HTTP status from a GitLab endpoint to a typed
/// [`GitlabUpstreamError`]. Callers have already read the body (or
/// chosen to skip it) by the time they call this; `message` carries
/// whatever context the caller wants surfaced to the UI.
pub fn map_status(status: StatusCode, message: impl Into<String>) -> GitlabUpstreamError {
    let message = message.into();
    match status {
        StatusCode::UNAUTHORIZED => GitlabUpstreamError::AuthInvalidToken,
        StatusCode::FORBIDDEN => GitlabUpstreamError::AuthMissingScope,
        // CONS-v0.2-02. Explicit 404 and 429 arms mirror the
        // atlassian-common `map_status` taxonomy. Without these,
        // 429 landed in the `_` catch-all and was misclassified as
        // `Upstream5xx` — the UI then showed a transient-outage
        // card for what is really a "slow down" signal, and the
        // connector's rate-limit loop never got to see the right
        // variant. 404 in the catch-all was a "here's a 5xx" lie
        // that hid the real cause (wrong `base_url`, stale project
        // path, or scope-miss on a private group).
        StatusCode::NOT_FOUND => GitlabUpstreamError::ResourceNotFound { message },
        StatusCode::GONE => GitlabUpstreamError::ResourceGone { message },
        StatusCode::TOO_MANY_REQUESTS => GitlabUpstreamError::RateLimited {
            // The SDK's retry loop already honoured the `Retry-After`
            // header before calling us — when we see 429 here, the
            // retry budget is exhausted and the original header is
            // no longer authoritative. Zero is the conservative
            // default; callers that still have a fresher value can
            // construct `RateLimited` directly.
            retry_after_secs: 0,
        },
        s if s.is_server_error() => GitlabUpstreamError::Upstream5xx { status: s, message },
        _ => GitlabUpstreamError::Upstream5xx {
            status,
            message: format!("unexpected status {status}: {message}"),
        },
    }
}

/// Classify a [`reqwest::Error`] as DNS, TLS, or a generic transport
/// failure. We surface DNS and TLS separately because they drive
/// different UI surfaces (Task 3's error card has different copy for
/// each); everything else lumps into `UrlDns` because, from the user's
/// perspective, "we couldn't reach the host" is the actionable signal
/// regardless of the precise socket-layer cause.
pub fn map_transport_error(err: &reqwest::Error) -> GitlabUpstreamError {
    let msg = err.to_string();
    let lower = msg.to_lowercase();
    if is_tls_error(&lower) {
        GitlabUpstreamError::UrlTls { message: msg }
    } else {
        GitlabUpstreamError::UrlDns { message: msg }
    }
}

fn is_tls_error(lower_msg: &str) -> bool {
    // `reqwest` does not expose a stable `is_tls()`; we sniff the
    // message for the rustls-side markers. False negatives degrade
    // into `UrlDns` which is safe — both codes surface a "can't reach
    // the host" card, just with different copy.
    lower_msg.contains("tls")
        || lower_msg.contains("certificate")
        || lower_msg.contains("handshake")
        || lower_msg.contains("ssl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_invalid_token_maps_to_gitlab_code_and_auth_variant() {
        let err: DayseamError = GitlabUpstreamError::AuthInvalidToken.into();
        assert_eq!(err.code(), error_codes::GITLAB_AUTH_INVALID_TOKEN);
        assert_eq!(err.variant(), "Auth");
    }

    #[test]
    fn auth_missing_scope_maps_to_gitlab_code_and_auth_variant() {
        let err: DayseamError = GitlabUpstreamError::AuthMissingScope.into();
        assert_eq!(err.code(), error_codes::GITLAB_AUTH_MISSING_SCOPE);
        assert_eq!(err.variant(), "Auth");
    }

    #[test]
    fn url_dns_and_tls_each_map_to_network_variant() {
        let dns: DayseamError = GitlabUpstreamError::UrlDns {
            message: "dns".into(),
        }
        .into();
        assert_eq!(dns.code(), error_codes::GITLAB_URL_DNS);
        assert_eq!(dns.variant(), "Network");

        let tls: DayseamError = GitlabUpstreamError::UrlTls {
            message: "bad cert".into(),
        }
        .into();
        assert_eq!(tls.code(), error_codes::GITLAB_URL_TLS);
        assert_eq!(tls.variant(), "Network");
    }

    #[test]
    fn rate_limited_preserves_retry_after_and_code() {
        let err: DayseamError = GitlabUpstreamError::RateLimited {
            retry_after_secs: 30,
        }
        .into();
        assert_eq!(err.code(), error_codes::GITLAB_RATE_LIMITED);
        if let DayseamError::RateLimited {
            retry_after_secs, ..
        } = err
        {
            assert_eq!(retry_after_secs, 30);
        } else {
            panic!("expected RateLimited variant");
        }
    }

    #[test]
    fn upstream_5xx_carries_status_and_code() {
        let err: DayseamError = GitlabUpstreamError::Upstream5xx {
            status: StatusCode::BAD_GATEWAY,
            message: "boom".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::GITLAB_UPSTREAM_5XX);
        assert_eq!(err.variant(), "Network");
    }

    #[test]
    fn shape_changed_maps_to_upstream_changed_variant() {
        let err: DayseamError = GitlabUpstreamError::ShapeChanged {
            message: "unknown target_type".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::GITLAB_UPSTREAM_SHAPE_CHANGED);
        assert_eq!(err.variant(), "UpstreamChanged");
    }

    #[test]
    fn map_status_routes_401_and_403_to_auth_buckets() {
        assert_eq!(
            map_status(StatusCode::UNAUTHORIZED, "nope"),
            GitlabUpstreamError::AuthInvalidToken
        );
        assert_eq!(
            map_status(StatusCode::FORBIDDEN, "nope"),
            GitlabUpstreamError::AuthMissingScope
        );
    }

    #[test]
    fn map_status_routes_5xx_to_upstream_5xx() {
        let e = map_status(StatusCode::INTERNAL_SERVER_ERROR, "down");
        assert!(matches!(e, GitlabUpstreamError::Upstream5xx { .. }));
    }

    /// CONS-v0.2-02 parity with atlassian-common: 404 must surface as
    /// a typed `ResourceNotFound`, and 429 as `RateLimited` with the
    /// conservative zero-second retry-after default. Pre-v0.2.1 both
    /// fell into the `_` catch-all and were misclassified as
    /// `Upstream5xx`, which lies to the UI about both the cause and
    /// whether a retry is likely to help.
    #[test]
    fn map_status_routes_404_to_resource_not_found() {
        let e = map_status(StatusCode::NOT_FOUND, "no such project");
        match e {
            GitlabUpstreamError::ResourceNotFound { message } => {
                assert!(message.contains("no such project"));
            }
            other => panic!("expected ResourceNotFound, got {other:?}"),
        }
        let err: DayseamError = map_status(StatusCode::NOT_FOUND, "missing").into();
        assert_eq!(err.code(), error_codes::GITLAB_RESOURCE_NOT_FOUND);
        assert_eq!(err.variant(), "Network");
    }

    #[test]
    fn map_status_routes_429_to_rate_limited_with_zero_retry_after_as_conservative_default() {
        let e = map_status(StatusCode::TOO_MANY_REQUESTS, "slow down");
        assert_eq!(
            e,
            GitlabUpstreamError::RateLimited {
                retry_after_secs: 0
            }
        );
        let err: DayseamError = e.into();
        assert_eq!(err.code(), error_codes::GITLAB_RATE_LIMITED);
        match err {
            DayseamError::RateLimited {
                retry_after_secs, ..
            } => {
                assert_eq!(retry_after_secs, 0);
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    /// DAY-89 CONS-v0.2-06. 410 Gone now routes through `ResourceGone`
    /// (symmetric with Atlassian's `ResourceGone`), not the
    /// `_ => Upstream5xx` catch-all that would misreport a deleted
    /// upstream resource as a transient server outage.
    #[test]
    fn map_status_routes_410_to_resource_gone() {
        let e = map_status(StatusCode::GONE, "project deleted");
        match e {
            GitlabUpstreamError::ResourceGone { ref message } => {
                assert!(message.contains("project deleted"));
            }
            other => panic!("expected ResourceGone, got {other:?}"),
        }
        let err: DayseamError = e.into();
        assert_eq!(err.code(), error_codes::GITLAB_RESOURCE_GONE);
        assert_eq!(err.variant(), "Network");
    }

    /// The codes this module maps into must exist in the central
    /// [`error_codes::ALL`] registry — a rename on either side of the
    /// edge is caught by the `registry_snapshot` test in `dayseam-core`,
    /// but a *silent drop* here (adding a code without mapping it)
    /// would not be. This test holds the taxonomy-completeness line.
    #[test]
    fn error_taxonomy_matches_design() {
        let expected = [
            error_codes::GITLAB_AUTH_INVALID_TOKEN,
            error_codes::GITLAB_AUTH_MISSING_SCOPE,
            error_codes::GITLAB_URL_DNS,
            error_codes::GITLAB_URL_TLS,
            error_codes::GITLAB_RATE_LIMITED,
            error_codes::GITLAB_UPSTREAM_5XX,
            error_codes::GITLAB_UPSTREAM_SHAPE_CHANGED,
            error_codes::GITLAB_RESOURCE_NOT_FOUND,
            error_codes::GITLAB_RESOURCE_GONE,
        ];
        for code in expected {
            assert!(
                error_codes::ALL.contains(&code),
                "{code} missing from registry"
            );
        }
    }
}
