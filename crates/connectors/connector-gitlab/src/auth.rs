//! PAT validation helper.
//!
//! The *durable* auth strategy — the one the connector reaches for
//! inside `sync` — is [`connectors_sdk::PatAuth`]. This module owns
//! the one-shot "is this PAT any good?" probe that runs when the user
//! pastes a token in the add-source / Reconnect dialog (Task 3). The
//! probe calls `GET <base_url>/api/v4/user` and, on success, returns
//! the numeric `user_id` + `username` the add-source flow stores on
//! the [`dayseam_core::SourceConfig::GitLab`] row.
//!
//! The PAT is accepted as a `&str` and the function never logs it —
//! neither in `tracing`, nor in `DayseamError::Auth::message`, nor via
//! `Debug` on any local value. Every error path passes only the host
//! and the HTTP status upward.

use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;

use crate::errors::{map_status, map_transport_error, GitlabUpstreamError};
use dayseam_core::DayseamError;

/// Shape returned by GitLab's `/user` endpoint. We only keep the two
/// fields the add-source flow persists.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitlabUserInfo {
    /// Stable numeric id — the authoritative identity anchor. v0.1's
    /// walker matches events by this, never by username.
    pub id: i64,
    pub username: String,
}

/// Validate a GitLab PAT against `host`. Returns the `(user_id,
/// username)` pair GitLab echoed back, or one of the seven
/// `gitlab.*` [`DayseamError`] codes — `auth.invalid_token`,
/// `auth.missing_scope`, `url.dns`, or `url.tls` in practice. A
/// 429 / 5xx is exceptionally rare on `/user` and is surfaced as
/// `upstream_5xx` without any retry (the caller is an interactive
/// dialog; surfacing the retry there is a Task 3 UX concern).
///
/// `host` is the user-facing base URL with or without a trailing
/// slash — we normalise it.
pub async fn validate_pat(host: &str, pat: &str) -> Result<GitlabUserInfo, DayseamError> {
    let base = host.trim_end_matches('/');
    let url = format!("{base}/api/v4/user");

    let client = reqwest::Client::builder()
        .user_agent(concat!("dayseam/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| DayseamError::Network {
            code: dayseam_core::error_codes::GITLAB_URL_TLS.to_string(),
            message: format!("failed to build HTTP client: {e}"),
        })?;

    let response = client
        .get(&url)
        .header("PRIVATE-TOKEN", pat)
        .send()
        .await
        .map_err(|err| DayseamError::from(map_transport_error(&err)))?;

    let status = response.status();
    if status.is_success() {
        let user: GitlabUserInfo = response.json().await.map_err(|err| {
            DayseamError::from(GitlabUpstreamError::ShapeChanged {
                message: format!("failed to decode /user response: {err}"),
            })
        })?;
        return Ok(user);
    }

    // 401 → invalid_token, 403 → missing_scope, 5xx → upstream_5xx.
    // A 429 from /user is out-of-band and mapped to upstream_5xx so
    // it surfaces as "try again" rather than silently retrying inside
    // an interactive probe.
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(GitlabUpstreamError::RateLimited {
            retry_after_secs: 0,
        }
        .into());
    }
    Err(map_status(status, "PAT validation failed").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitlab_user_info_decodes_and_drops_extra_fields() {
        // GitLab returns avatar_url, state, created_at, and many
        // other fields on `/user`. We only need `id` + `username`
        // and must silently drop the rest.
        let padded = r#"{
            "id": 17,
            "username": "vedanth",
            "name": "Vedanth",
            "avatar_url": "https://gitlab.example/avatar/17.png",
            "state": "active",
            "created_at": "2021-01-01T00:00:00.000Z"
        }"#;
        let back: GitlabUserInfo =
            serde_json::from_str(padded).expect("serde should ignore unknown fields by default");
        assert_eq!(back.id, 17);
        assert_eq!(back.username, "vedanth");
    }
}
