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
//!
//! DAY-129: transport failures (DNS, TLS, connect-refused, timeout)
//! go through the SDK's shared [`HttpClient::send`] classifier rather
//! than this crate's legacy `map_transport_error`. That fold means the
//! PAT-validation lane emits the same `http.transport.*` sub-codes as
//! the in-sync GitLab calls instead of collapsing every transport
//! failure into `gitlab.url.dns` (and occasionally `gitlab.url.tls`).
//! The message still names the host — see
//! `connectors_sdk::http::format_transport_error` — so the
//! `SourceErrorCard` fallback renders "couldn't reach
//! `gitlab.example.com` after 1 attempts: …" on the Add-Source dialog
//! without needing bespoke per-connector copy for transport codes.

use reqwest::StatusCode;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::errors::{map_status, GitlabUpstreamError};
use connectors_sdk::{HttpClient, RetryPolicy};
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
/// username)` pair GitLab echoed back, or one of the `gitlab.*`
/// [`DayseamError`] codes — `auth.invalid_token`,
/// `auth.missing_scope`, `resource_not_found`, `upstream_5xx` — when
/// GitLab answers with a non-success status. Transport failures
/// (DNS, TLS, refused connect, timeout) come back as `http.transport.*`
/// sub-codes (DAY-129) classified by [`HttpClient::send`] rather than
/// the now-deleted connector-local `map_transport_error`. 429 is
/// exceptionally rare on `/user`; when it does land here the SDK has
/// already exhausted its retry budget, so we surface it as
/// `RateLimited` with a conservative zero-second retry-after.
///
/// `host` is the user-facing base URL with or without a trailing
/// slash — we normalise it.
///
/// The interactive Add-Source dialog can't plausibly benefit from a
/// five-attempt retry ladder before the user sees feedback, so the
/// probe uses [`RetryPolicy::instant`] with `max_attempts = 1` — we
/// want a single honest try, not a multi-second wait before
/// surfacing "couldn't reach `gitlab.example.com`" back to the form.
/// The durable in-sync path keeps the SDK's default policy.
pub async fn validate_pat(host: &str, pat: &str) -> Result<GitlabUserInfo, DayseamError> {
    let base = host.trim_end_matches('/');
    let url = format!("{base}/api/v4/user");

    let http = HttpClient::new()?.with_policy(RetryPolicy {
        max_attempts: 1,
        ..RetryPolicy::instant()
    });
    let cancel = CancellationToken::new();

    let request = http.reqwest().get(&url).header("PRIVATE-TOKEN", pat);
    let response = http.send(request, &cancel, None, None).await?;

    let status = response.status();
    if status.is_success() {
        let user: GitlabUserInfo = response.json().await.map_err(|err| {
            DayseamError::from(GitlabUpstreamError::ShapeChanged {
                message: format!("failed to decode /user response: {err}"),
            })
        })?;
        return Ok(user);
    }

    // 401 → invalid_token, 403 → missing_scope, 404 → resource_not_found,
    // 429 → rate_limited, 5xx → upstream_5xx. The `map_status` helper
    // owns the routing table; we just hand it the status and a
    // context string. CONS-v0.2-02: the explicit 429 arm below is
    // now redundant (map_status handles it), but we keep it as a
    // defensive layer — a future refactor that accidentally turns
    // the map_status 429 arm into the `_` catch-all would still be
    // caught here.
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
