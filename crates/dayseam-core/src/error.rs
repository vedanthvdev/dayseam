//! Error taxonomy. Every error the core, connectors, or sinks surface to
//! the UI is a `DayseamError` variant with a stable string `code` so the
//! frontend can key messages, help links, and retry logic off it without
//! parsing prose.
//!
//! The serde representation uses `#[serde(tag = "variant", content =
//! "data")]` so each error serialises as
//! `{"variant": "Auth", "data": {"code": "...", ...}}` — stable both for
//! IPC to the frontend and for logging to disk.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

/// The single error type used by every Dayseam crate at its public
/// boundary. Internal modules may use their own error types but must map
/// into `DayseamError` before reaching core APIs.
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "variant", content = "data")]
pub enum DayseamError {
    #[error("auth error [{code}]: {message}")]
    Auth {
        code: String,
        message: String,
        retryable: bool,
        action_hint: Option<String>,
    },
    #[error("network error [{code}]: {message}")]
    Network { code: String, message: String },
    #[error("rate-limited [{code}]: retry after {retry_after_secs}s")]
    RateLimited { code: String, retry_after_secs: u64 },
    #[error("upstream changed [{code}]: {message}")]
    UpstreamChanged { code: String, message: String },
    #[error("invalid config [{code}]: {message}")]
    InvalidConfig { code: String, message: String },
    #[error("io error [{code}]: {message}")]
    Io {
        code: String,
        path: Option<PathBuf>,
        message: String,
    },
    #[error("internal error [{code}]: {message}")]
    Internal { code: String, message: String },
}

impl DayseamError {
    /// Stable machine-readable code for this error. The frontend uses this
    /// to decide which help text to show and whether to offer a retry.
    /// Renaming a code is a breaking change and must bump semver.
    pub fn code(&self) -> &str {
        match self {
            Self::Auth { code, .. }
            | Self::Network { code, .. }
            | Self::RateLimited { code, .. }
            | Self::UpstreamChanged { code, .. }
            | Self::InvalidConfig { code, .. }
            | Self::Io { code, .. }
            | Self::Internal { code, .. } => code,
        }
    }

    /// Variant name as a stable string — useful for metrics/telemetry
    /// buckets that shouldn't churn when we add new codes.
    pub fn variant(&self) -> &'static str {
        match self {
            Self::Auth { .. } => "Auth",
            Self::Network { .. } => "Network",
            Self::RateLimited { .. } => "RateLimited",
            Self::UpstreamChanged { .. } => "UpstreamChanged",
            Self::InvalidConfig { .. } => "InvalidConfig",
            Self::Io { .. } => "Io",
            Self::Internal { .. } => "Internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_codes;

    #[test]
    fn code_accessor_returns_inner_code() {
        let e = DayseamError::Auth {
            code: error_codes::GITLAB_AUTH_INVALID_TOKEN.to_string(),
            message: "bad token".into(),
            retryable: false,
            action_hint: None,
        };
        assert_eq!(e.code(), error_codes::GITLAB_AUTH_INVALID_TOKEN);
        assert_eq!(e.variant(), "Auth");
    }

    #[test]
    fn serialises_with_variant_and_data_envelope() {
        let e = DayseamError::RateLimited {
            code: error_codes::GITLAB_RATE_LIMITED.to_string(),
            retry_after_secs: 30,
        };
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["variant"], "RateLimited");
        assert_eq!(json["data"]["code"], error_codes::GITLAB_RATE_LIMITED);
        assert_eq!(json["data"]["retry_after_secs"], 30);
    }

    #[test]
    fn round_trips_through_json() {
        let e = DayseamError::Io {
            code: "sink.fs.not_writable".into(),
            path: Some(PathBuf::from("/tmp/dayseam/report.md")),
            message: "permission denied".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: DayseamError = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
