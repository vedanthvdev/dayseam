//! Typed conversions from Microsoft Graph failures to the
//! `outlook.*` codes in [`dayseam_core::error_codes`].
//!
//! Shape-mirrors `connector-github::errors` and
//! `connector-gitlab::errors`: explicit arms for 401 / 403 / 404 /
//! 410 / 429 / 5xx, catch-all folded into `Upstream5xx` so an
//! unexpected status never silently masquerades as a success.
//!
//! The only Graph-specific wrinkle is that Microsoft returns 403 for
//! both "admin consent missing" and "scope not granted", and the
//! error body's `error.code` string distinguishes the two
//! (`AccessDenied` vs `ErrorAccessDenied` vs
//! `ErrorItemNotFound` etc). v0.9 keeps the classification
//! status-code-only — consent-required distinctions are surfaced
//! at the OAuth-consent phase, not here. A future refactor can read
//! the body if dogfood surfaces a false-positive.

use dayseam_core::{error_codes, DayseamError};
use reqwest::StatusCode;

/// Connector-local categorisation of Graph failure modes before we
/// map them to [`DayseamError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutlookUpstreamError {
    /// 401 — access token rejected (expired, revoked, or malformed).
    /// Non-retryable from the connector's POV; the orchestrator's
    /// OAuth refresh path is what actually rotates the token, and
    /// reaching this arm means refresh itself succeeded but the new
    /// token was immediately rejected — a reconnect signal.
    AuthInvalidCredentials,
    /// 403 — access token authenticates but lacks the required
    /// delegated scope (Calendars.Read, User.Read). Same `Auth`
    /// variant, different code, same hint ("reconnect and reconsent").
    AuthMissingScope,
    /// 404 on a Graph endpoint — the resource (event, user) isn't
    /// reachable. Split out from the catch-all so the UI surfaces
    /// "check the calendar" rather than "upstream is down".
    ResourceNotFound { message: String },
    /// 410 Gone — event was hard-deleted between list and fetch.
    /// Distinct from 404 so the orchestrator never retries.
    ResourceGone { message: String },
    /// 429 — Graph asked us to slow down. The SDK's retry loop
    /// surfaces this before ever constructing a `DayseamError`;
    /// mapped here only when the retry budget is exhausted.
    RateLimited { retry_after_secs: u64 },
    /// 5xx that survived the retry budget.
    Upstream5xx { status: StatusCode, message: String },
    /// Response decoded but carried a shape the connector doesn't
    /// know how to interpret — an unknown event type, a missing
    /// required field. Produces [`error_codes::OUTLOOK_UPSTREAM_SHAPE_CHANGED`]
    /// so the UI can tell "connector bug" from "credentials are wrong".
    ShapeChanged { message: String },
}

impl From<OutlookUpstreamError> for DayseamError {
    fn from(value: OutlookUpstreamError) -> Self {
        match value {
            OutlookUpstreamError::AuthInvalidCredentials => DayseamError::Auth {
                code: error_codes::OUTLOOK_AUTH_INVALID_CREDENTIALS.to_string(),
                message: "Microsoft Graph rejected the OAuth access token".to_string(),
                retryable: false,
                action_hint: Some(
                    "Open Settings, select this source, and click Reconnect to re-authenticate with Microsoft."
                        .to_string(),
                ),
            },
            OutlookUpstreamError::AuthMissingScope => DayseamError::Auth {
                code: error_codes::OUTLOOK_AUTH_MISSING_SCOPE.to_string(),
                message:
                    "OAuth token is valid but missing a delegated scope Dayseam needs \
                     (Calendars.Read + User.Read). If your tenant admin must approve, \
                     reconnect after they grant consent."
                        .to_string(),
                retryable: false,
                action_hint: Some(
                    "Reconnect this source; consent again with the required scopes.".to_string(),
                ),
            },
            OutlookUpstreamError::ResourceNotFound { message } => DayseamError::Network {
                code: error_codes::OUTLOOK_RESOURCE_NOT_FOUND.to_string(),
                message,
            },
            OutlookUpstreamError::ResourceGone { message } => DayseamError::Network {
                code: error_codes::OUTLOOK_RESOURCE_GONE.to_string(),
                message: format!("Microsoft Graph returned 410 Gone: {message}"),
            },
            OutlookUpstreamError::RateLimited { retry_after_secs } => DayseamError::RateLimited {
                code: error_codes::OUTLOOK_RATE_LIMITED.to_string(),
                retry_after_secs,
            },
            OutlookUpstreamError::Upstream5xx { status, message } => DayseamError::Network {
                code: error_codes::OUTLOOK_UPSTREAM_5XX.to_string(),
                message: format!("Microsoft Graph returned {status}: {message}"),
            },
            OutlookUpstreamError::ShapeChanged { message } => DayseamError::UpstreamChanged {
                code: error_codes::OUTLOOK_UPSTREAM_SHAPE_CHANGED.to_string(),
                message,
            },
        }
    }
}

/// Map a non-success HTTP status from a Microsoft Graph endpoint to a
/// typed [`OutlookUpstreamError`]. Symmetric with
/// `connector-github::errors::map_status` and
/// `connector-gitlab::errors::map_status`.
pub fn map_status(status: StatusCode, message: impl Into<String>) -> OutlookUpstreamError {
    let message = message.into();
    match status {
        StatusCode::UNAUTHORIZED => OutlookUpstreamError::AuthInvalidCredentials,
        StatusCode::FORBIDDEN => OutlookUpstreamError::AuthMissingScope,
        StatusCode::NOT_FOUND => OutlookUpstreamError::ResourceNotFound { message },
        StatusCode::GONE => OutlookUpstreamError::ResourceGone { message },
        StatusCode::TOO_MANY_REQUESTS => OutlookUpstreamError::RateLimited {
            // Matches GitHub's map_status 429 arm: the SDK's retry loop
            // already honoured the Retry-After header before calling
            // us, so the header is no longer authoritative by the time
            // we see 429 here.
            retry_after_secs: 0,
        },
        s if s.is_server_error() => OutlookUpstreamError::Upstream5xx { status: s, message },
        _ => OutlookUpstreamError::Upstream5xx {
            status,
            message: format!("unexpected status {status}: {message}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_invalid_credentials_maps_to_outlook_code_and_auth_variant() {
        let err: DayseamError = OutlookUpstreamError::AuthInvalidCredentials.into();
        assert_eq!(err.code(), error_codes::OUTLOOK_AUTH_INVALID_CREDENTIALS);
        assert_eq!(err.variant(), "Auth");
    }

    #[test]
    fn auth_missing_scope_maps_to_outlook_code_and_auth_variant() {
        let err: DayseamError = OutlookUpstreamError::AuthMissingScope.into();
        assert_eq!(err.code(), error_codes::OUTLOOK_AUTH_MISSING_SCOPE);
        assert_eq!(err.variant(), "Auth");
    }

    #[test]
    fn rate_limited_preserves_retry_after_and_code() {
        let err: DayseamError = OutlookUpstreamError::RateLimited {
            retry_after_secs: 42,
        }
        .into();
        assert_eq!(err.code(), error_codes::OUTLOOK_RATE_LIMITED);
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
        let err: DayseamError = OutlookUpstreamError::Upstream5xx {
            status: StatusCode::BAD_GATEWAY,
            message: "boom".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::OUTLOOK_UPSTREAM_5XX);
        assert_eq!(err.variant(), "Network");
    }

    #[test]
    fn shape_changed_maps_to_upstream_changed_variant() {
        let err: DayseamError = OutlookUpstreamError::ShapeChanged {
            message: "unknown event type".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::OUTLOOK_UPSTREAM_SHAPE_CHANGED);
        assert_eq!(err.variant(), "UpstreamChanged");
    }

    #[test]
    fn map_status_routes_401_and_403_to_auth_buckets() {
        assert_eq!(
            map_status(StatusCode::UNAUTHORIZED, "nope"),
            OutlookUpstreamError::AuthInvalidCredentials
        );
        assert_eq!(
            map_status(StatusCode::FORBIDDEN, "nope"),
            OutlookUpstreamError::AuthMissingScope
        );
    }

    #[test]
    fn map_status_routes_404_to_resource_not_found() {
        let e = map_status(StatusCode::NOT_FOUND, "no such event");
        match e {
            OutlookUpstreamError::ResourceNotFound { message } => {
                assert!(message.contains("no such event"));
            }
            other => panic!("expected ResourceNotFound, got {other:?}"),
        }
    }

    #[test]
    fn map_status_routes_410_to_resource_gone() {
        let e = map_status(StatusCode::GONE, "event deleted");
        match e {
            OutlookUpstreamError::ResourceGone { ref message } => {
                assert!(message.contains("event deleted"));
            }
            other => panic!("expected ResourceGone, got {other:?}"),
        }
        let err: DayseamError = e.into();
        assert_eq!(err.code(), error_codes::OUTLOOK_RESOURCE_GONE);
    }

    #[test]
    fn map_status_routes_429_to_rate_limited_with_zero_retry_after_default() {
        let e = map_status(StatusCode::TOO_MANY_REQUESTS, "slow down");
        assert_eq!(
            e,
            OutlookUpstreamError::RateLimited {
                retry_after_secs: 0
            }
        );
    }

    #[test]
    fn map_status_routes_5xx_to_upstream_5xx() {
        let e = map_status(StatusCode::INTERNAL_SERVER_ERROR, "down");
        assert!(matches!(e, OutlookUpstreamError::Upstream5xx { .. }));
        let e = map_status(StatusCode::SERVICE_UNAVAILABLE, "maintenance");
        assert!(matches!(e, OutlookUpstreamError::Upstream5xx { .. }));
    }

    #[test]
    fn map_status_unexpected_status_falls_through_to_upstream_5xx() {
        let e = map_status(StatusCode::IM_A_TEAPOT, "wat");
        match e {
            OutlookUpstreamError::Upstream5xx { status, message } => {
                assert_eq!(status, StatusCode::IM_A_TEAPOT);
                assert!(message.contains("unexpected status"));
            }
            other => panic!("expected Upstream5xx catch-all, got {other:?}"),
        }
    }

    #[test]
    fn error_taxonomy_matches_registry() {
        let expected = [
            error_codes::OUTLOOK_AUTH_INVALID_CREDENTIALS,
            error_codes::OUTLOOK_AUTH_MISSING_SCOPE,
            error_codes::OUTLOOK_RESOURCE_NOT_FOUND,
            error_codes::OUTLOOK_RESOURCE_GONE,
            error_codes::OUTLOOK_RATE_LIMITED,
            error_codes::OUTLOOK_UPSTREAM_5XX,
            error_codes::OUTLOOK_UPSTREAM_SHAPE_CHANGED,
        ];
        for code in expected {
            assert!(
                error_codes::ALL.contains(&code),
                "{code} missing from registry"
            );
        }
    }
}
