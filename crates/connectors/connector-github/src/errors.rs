//! Typed conversions from connector-local failure modes to the seven
//! `github.*` codes in [`dayseam_core::error_codes`].
//!
//! The registry itself lives in `dayseam-core` (DAY-93); this module is
//! the bridge that turns an HTTP status or a serde decode failure into
//! the right [`DayseamError`] variant + stable code so downstream log
//! parsers, UI copy, and the `SourceErrorCard` surface (DAY-99) have a
//! single source of truth.
//!
//! DAY-129: transport-layer classification (DNS, TLS, connect-refused,
//! timeout) is the SDK's responsibility — see
//! `connectors_sdk::http::classify_transport_error`. The pre-DAY-129
//! `map_transport_error` / `UrlDns` / `UrlTls` helpers in this module
//! were dead code (the GitHub connector has always routed through
//! [`connectors_sdk::HttpClient::send`]); dropping them removes one
//! class of "looks live but isn't" surface from the registry.
//!
//! Shape-mirrors `connector-gitlab::errors` and
//! `connector-atlassian-common::errors` — each connector family owns
//! its own `map_status` + `UpstreamError` enum so a future refactor in
//! one family's taxonomy cannot silently bleed into another's. The
//! cross-family invariant (`*.resource_gone` / `*.upstream_5xx` /
//! `*.rate_limited` cover every registered connector) is held by
//! `crates/dayseam-core/tests/error_codes.rs` rather than by any one
//! connector crate.

use dayseam_core::{error_codes, DayseamError};
use reqwest::StatusCode;

/// Connector-local categorisation of failure modes before we map them
/// to [`DayseamError`]. Not a public trait; it just gives the
/// [`map_status`] helper a structured switch to drive off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubUpstreamError {
    /// 401 — token rejected. `Auth` variant, non-retryable, hints at
    /// the Reconnect flow DAY-99 will own.
    AuthInvalidCredentials,
    /// 403 — token authenticates but lacks the required scopes
    /// (classic PAT: `read:user` + `repo` + `read:org`; fine-grained
    /// PAT: read-only `Metadata`, `Issues`, `Pull requests`,
    /// `Contents`). Same `Auth` variant, different code, same hint.
    AuthMissingScope,
    /// 404 on a GitHub endpoint — the resource (user, repo, issue,
    /// PR) isn't reachable. Split out from the catch-all so the UI
    /// surfaces "check the URL / scope" rather than "upstream is
    /// down", matching the GitLab + Atlassian taxonomies (CONS-v0.2-02
    /// symmetry). Maps to [`error_codes::GITHUB_RESOURCE_NOT_FOUND`]
    /// on the [`DayseamError::Network`] variant.
    ResourceNotFound { message: String },
    /// 410 Gone — the resource has been hard-deleted (archived repo
    /// force-deleted, rewritten history, account deletion). Distinct
    /// from 404 (which can be a permissions race) so the orchestrator
    /// never retries. Maps to [`error_codes::GITHUB_RESOURCE_GONE`]
    /// on the [`DayseamError::Network`] variant. Symmetric with
    /// GitLab's [`error_codes::GITLAB_RESOURCE_GONE`] and Atlassian's
    /// [`error_codes::JIRA_RESOURCE_GONE`] /
    /// [`error_codes::CONFLUENCE_RESOURCE_GONE`] so every connector
    /// routes deleted-upstream resources through the same
    /// terminal-error lane (DAY-89 CONS-v0.2-06 parity).
    ResourceGone { message: String },
    /// 429 — server asked us to slow down. The SDK's retry loop
    /// surfaces this before ever constructing a `DayseamError`;
    /// mapped here only when the retry budget is exhausted.
    RateLimited { retry_after_secs: u64 },
    /// 5xx that survived the retry budget.
    Upstream5xx { status: StatusCode, message: String },
    /// Response decoded but carried a shape the connector doesn't
    /// know how to interpret — an unknown event `type`, a missing
    /// required field. Produces
    /// [`error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED`] so the UI can
    /// tell "connector bug" from "credentials are wrong".
    ShapeChanged { message: String },
}

impl From<GithubUpstreamError> for DayseamError {
    fn from(value: GithubUpstreamError) -> Self {
        match value {
            GithubUpstreamError::AuthInvalidCredentials => DayseamError::Auth {
                code: error_codes::GITHUB_AUTH_INVALID_CREDENTIALS.to_string(),
                message: "GitHub rejected the personal access token".to_string(),
                retryable: false,
                action_hint: Some(
                    "Open Settings, select this source, and click Reconnect to paste a fresh PAT."
                        .to_string(),
                ),
            },
            GithubUpstreamError::AuthMissingScope => DayseamError::Auth {
                code: error_codes::GITHUB_AUTH_MISSING_SCOPE.to_string(),
                message: "PAT is valid but missing a scope Dayseam needs \
                          (classic PAT: read:user + repo + read:org; \
                          fine-grained PAT: read-only Metadata, Issues, \
                          Pull requests, Contents)"
                    .to_string(),
                retryable: false,
                action_hint: Some(
                    "Generate a fresh PAT with the required scopes and reconnect this source."
                        .to_string(),
                ),
            },
            GithubUpstreamError::ResourceNotFound { message } => DayseamError::Network {
                code: error_codes::GITHUB_RESOURCE_NOT_FOUND.to_string(),
                message,
            },
            GithubUpstreamError::ResourceGone { message } => DayseamError::Network {
                code: error_codes::GITHUB_RESOURCE_GONE.to_string(),
                message: format!("GitHub resource returned 410 Gone: {message}"),
            },
            GithubUpstreamError::RateLimited { retry_after_secs } => DayseamError::RateLimited {
                code: error_codes::GITHUB_RATE_LIMITED.to_string(),
                retry_after_secs,
            },
            GithubUpstreamError::Upstream5xx { status, message } => DayseamError::Network {
                code: error_codes::GITHUB_UPSTREAM_5XX.to_string(),
                message: format!("GitHub returned {status}: {message}"),
            },
            GithubUpstreamError::ShapeChanged { message } => DayseamError::UpstreamChanged {
                code: error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED.to_string(),
                message,
            },
        }
    }
}

/// Map a non-success HTTP status from a GitHub endpoint to a typed
/// [`GithubUpstreamError`]. Callers have already decided whether to
/// read the body (or skip it) by the time they call this; `message`
/// carries whatever context the caller wants surfaced to the UI.
///
/// Symmetric with `connector-gitlab::errors::map_status` and
/// `connector-atlassian-common::errors::map_status`: explicit arms for
/// 401 / 403 / 404 / 410 / 429 / 5xx, catch-all folded into
/// `Upstream5xx` so an unexpected status never silently masquerades as
/// a success. The `_ => Upstream5xx` arm is deliberately coarse — the
/// only statuses the GitHub REST API returns in practice are the ones
/// enumerated here, plus 2xx; anything else is either a proxy / LB
/// rewrite or an upstream bug, both of which the user experiences as
/// "something's wrong on the server side".
pub fn map_status(status: StatusCode, message: impl Into<String>) -> GithubUpstreamError {
    let message = message.into();
    match status {
        StatusCode::UNAUTHORIZED => GithubUpstreamError::AuthInvalidCredentials,
        StatusCode::FORBIDDEN => GithubUpstreamError::AuthMissingScope,
        StatusCode::NOT_FOUND => GithubUpstreamError::ResourceNotFound { message },
        StatusCode::GONE => GithubUpstreamError::ResourceGone { message },
        StatusCode::TOO_MANY_REQUESTS => GithubUpstreamError::RateLimited {
            // The SDK's retry loop already honoured the `Retry-After`
            // header before calling us — when we see 429 here the
            // retry budget is exhausted and the original header is no
            // longer authoritative. Zero is the conservative default,
            // matching GitLab's `map_status` 429 arm; callers that
            // still have a fresher value can construct `RateLimited`
            // directly.
            retry_after_secs: 0,
        },
        s if s.is_server_error() => GithubUpstreamError::Upstream5xx { status: s, message },
        _ => GithubUpstreamError::Upstream5xx {
            status,
            message: format!("unexpected status {status}: {message}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_invalid_credentials_maps_to_github_code_and_auth_variant() {
        let err: DayseamError = GithubUpstreamError::AuthInvalidCredentials.into();
        assert_eq!(err.code(), error_codes::GITHUB_AUTH_INVALID_CREDENTIALS);
        assert_eq!(err.variant(), "Auth");
    }

    #[test]
    fn auth_missing_scope_maps_to_github_code_and_auth_variant() {
        let err: DayseamError = GithubUpstreamError::AuthMissingScope.into();
        assert_eq!(err.code(), error_codes::GITHUB_AUTH_MISSING_SCOPE);
        assert_eq!(err.variant(), "Auth");
    }

    #[test]
    fn rate_limited_preserves_retry_after_and_code() {
        let err: DayseamError = GithubUpstreamError::RateLimited {
            retry_after_secs: 42,
        }
        .into();
        assert_eq!(err.code(), error_codes::GITHUB_RATE_LIMITED);
        if let DayseamError::RateLimited {
            retry_after_secs, ..
        } = err
        {
            assert_eq!(retry_after_secs, 42);
        } else {
            panic!("expected RateLimited variant");
        }
    }

    #[test]
    fn upstream_5xx_carries_status_and_code() {
        let err: DayseamError = GithubUpstreamError::Upstream5xx {
            status: StatusCode::BAD_GATEWAY,
            message: "boom".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::GITHUB_UPSTREAM_5XX);
        assert_eq!(err.variant(), "Network");
    }

    #[test]
    fn shape_changed_maps_to_upstream_changed_variant() {
        let err: DayseamError = GithubUpstreamError::ShapeChanged {
            message: "unknown event type".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED);
        assert_eq!(err.variant(), "UpstreamChanged");
    }

    #[test]
    fn map_status_routes_401_and_403_to_auth_buckets() {
        assert_eq!(
            map_status(StatusCode::UNAUTHORIZED, "nope"),
            GithubUpstreamError::AuthInvalidCredentials
        );
        assert_eq!(
            map_status(StatusCode::FORBIDDEN, "nope"),
            GithubUpstreamError::AuthMissingScope
        );
    }

    #[test]
    fn map_status_routes_404_to_resource_not_found() {
        let e = map_status(StatusCode::NOT_FOUND, "no such repo");
        match e {
            GithubUpstreamError::ResourceNotFound { message } => {
                assert!(message.contains("no such repo"));
            }
            other => panic!("expected ResourceNotFound, got {other:?}"),
        }
        let err: DayseamError = map_status(StatusCode::NOT_FOUND, "missing").into();
        assert_eq!(err.code(), error_codes::GITHUB_RESOURCE_NOT_FOUND);
        assert_eq!(err.variant(), "Network");
    }

    /// Symmetric with `connector-gitlab::errors::map_status_routes_410_to_resource_gone`
    /// and the atlassian-common equivalent. 410 is a *terminal* status
    /// — the orchestrator must never retry — so routing it through
    /// `ResourceGone` (not `Upstream5xx`, which is retryable) is
    /// load-bearing for "deleted-upstream resources don't mask as
    /// transient outage noise."
    #[test]
    fn map_status_routes_410_to_resource_gone() {
        let e = map_status(StatusCode::GONE, "repo deleted");
        match e {
            GithubUpstreamError::ResourceGone { ref message } => {
                assert!(message.contains("repo deleted"));
            }
            other => panic!("expected ResourceGone, got {other:?}"),
        }
        let err: DayseamError = e.into();
        assert_eq!(err.code(), error_codes::GITHUB_RESOURCE_GONE);
        assert_eq!(err.variant(), "Network");
    }

    #[test]
    fn map_status_routes_429_to_rate_limited_with_zero_retry_after_default() {
        let e = map_status(StatusCode::TOO_MANY_REQUESTS, "slow down");
        assert_eq!(
            e,
            GithubUpstreamError::RateLimited {
                retry_after_secs: 0
            }
        );
    }

    #[test]
    fn map_status_routes_5xx_to_upstream_5xx() {
        let e = map_status(StatusCode::INTERNAL_SERVER_ERROR, "down");
        assert!(matches!(e, GithubUpstreamError::Upstream5xx { .. }));
        let e = map_status(StatusCode::SERVICE_UNAVAILABLE, "maintenance");
        assert!(matches!(e, GithubUpstreamError::Upstream5xx { .. }));
    }

    #[test]
    fn map_status_unexpected_status_falls_through_to_upstream_5xx() {
        // A 418 (I'm a teapot) or similarly unexpected 4xx should
        // not silently masquerade as a success. Route into the
        // coarse `Upstream5xx` lane with a labelled message so the
        // operator can see which status slipped through.
        let e = map_status(StatusCode::IM_A_TEAPOT, "wat");
        match e {
            GithubUpstreamError::Upstream5xx { status, message } => {
                assert_eq!(status, StatusCode::IM_A_TEAPOT);
                assert!(message.contains("unexpected status"));
            }
            other => panic!("expected Upstream5xx catch-all, got {other:?}"),
        }
    }

    /// The codes this module maps into must exist in the central
    /// [`error_codes::ALL`] registry — a rename on either side of the
    /// edge is caught by the `registry_snapshot` test in
    /// `dayseam-core`, but a *silent drop* here (adding a variant
    /// without mapping it) would not be. This test holds the
    /// taxonomy-completeness line.
    #[test]
    fn error_taxonomy_matches_design() {
        let expected = [
            error_codes::GITHUB_AUTH_INVALID_CREDENTIALS,
            error_codes::GITHUB_AUTH_MISSING_SCOPE,
            error_codes::GITHUB_RESOURCE_NOT_FOUND,
            error_codes::GITHUB_RESOURCE_GONE,
            error_codes::GITHUB_RATE_LIMITED,
            error_codes::GITHUB_UPSTREAM_5XX,
            error_codes::GITHUB_UPSTREAM_SHAPE_CHANGED,
        ];
        for code in expected {
            assert!(
                error_codes::ALL.contains(&code),
                "{code} missing from registry"
            );
        }
    }
}
