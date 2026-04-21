//! Cloud-identity discovery for Atlassian workspaces.
//!
//! Under Basic auth, the workspace URL (e.g.
//! `https://modulrfinance.atlassian.net`) is already the tenant
//! identity — both Jira (`/rest/api/3/…`) and Confluence
//! (`/wiki/api/v2/…`) accept the hostname directly. We therefore
//! don't strictly need the opaque `cloudId` UUID for request
//! routing (that matters for OAuth under v0.3+).
//!
//! What we **do** need, and what `discover_cloud` provides, is:
//!
//! 1. A credential-validation pass that proves the email + API
//!    token actually authenticates, *before* any per-product walker
//!    burns rate budget on a doomed sync. `GET /rest/api/3/myself` is
//!    the cheapest shared endpoint that returns both the auth result
//!    and the `accountId` we need for identity seeding.
//!
//! 2. An `accountId` + `displayName` + optional `emailAddress` triple
//!    the identity-seed layer (DAY-75 [`identity`], DAY-82 IPC) uses
//!    to bootstrap a `SourceIdentity` row. The returned
//!    [`AtlassianAccountInfo`] is that triple.
//!
//! Classification is kept out of `HttpClient` per CORR-01 — the
//! 401 path here goes through [`crate::errors::map_status`] which
//! emits `atlassian.auth.invalid_credentials`, and 403 /
//! 404 / 5xx surface through the same funnel.
//!
//! [`crate::errors::map_status`]: crate::errors::map_status

use connectors_sdk::{BasicAuth, HttpClient};
use dayseam_core::{DayseamError, LogLevel};
use dayseam_events::LogSender;
use reqwest::StatusCode;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::errors::{map_status, validate_account_id, AtlassianError, Product};

/// Result of an Atlassian cloud-identity probe.
///
/// `cloud_id` is [`None`] under Basic auth — the workspace URL is
/// already the tenant identifier and the `GET /_edge/tenant_info`
/// call that surfaces the opaque UUID is OAuth-scoped. The field is
/// carried forward as [`Option<Uuid>`] so a v0.3+ OAuth path can
/// populate it without breaking the public shape this crate exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlassianAccountInfo {
    /// Opaque Atlassian Cloud account id (e.g.
    /// `5d53f3cbc6b9320d9ea5bdc2`). The identity-seed layer uses this
    /// as `SourceIdentity::external_actor_id` under kind
    /// [`dayseam_core::SourceIdentityKind::AtlassianAccountId`].
    pub account_id: String,
    /// Display name the workspace shows for this account. Surfaced
    /// only for the onboarding toast ("Connected as …"); never used
    /// for matching.
    pub display_name: String,
    /// Email the workspace has on file for this account. Optional —
    /// GDPR-compliant Atlassian accounts may omit it. Never used for
    /// matching; stored for future heuristics (e.g. matching a Jira
    /// comment to a GitLab commit by email).
    pub email: Option<String>,
    /// Under Basic auth: always [`None`]. Reserved for OAuth-era
    /// discovery; see module-level docs.
    pub cloud_id: Option<Uuid>,
}

/// Paired with [`AtlassianAccountInfo`] in callers that need to
/// carry the workspace URL alongside the account triple. The
/// identity-seed + walker layers use this struct to avoid threading
/// a `(Url, AtlassianAccountInfo)` tuple through every function.
#[derive(Debug, Clone)]
pub struct AtlassianCloud {
    /// Canonical workspace URL, e.g. `https://modulrfinance.atlassian.net`.
    pub workspace_url: Url,
    /// Everything `/myself` gave us.
    pub account: AtlassianAccountInfo,
}

#[derive(Deserialize)]
struct MyselfResponse {
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "emailAddress")]
    email_address: Option<String>,
}

/// Probe a workspace with a Basic-auth credential and return the
/// account triple `GET /rest/api/3/myself` carries.
///
/// Failure modes (all routed through `AtlassianError`):
///
/// * 401 → [`AtlassianError::AuthInvalidCredentials`] with code
///   `atlassian.auth.invalid_credentials`.
/// * 403 → [`AtlassianError::AuthMissingScope`] with code
///   `atlassian.auth.missing_scope` (Jira product — Confluence never
///   reaches here; the caller hits `/wiki/rest/api/user/current`
///   when the credential is Confluence-only).
/// * 404 → [`AtlassianError::CloudResourceNotFound`] with code
///   `atlassian.cloud.resource_not_found`. This is the classic
///   "user typed `foo.atlassian.net` when they meant
///   `bar.atlassian.net`" signal.
/// * 429 / 5xx → the generic walk-shape-changed / rate-limited buckets.
/// * Malformed `accountId` in a 200 response →
///   [`AtlassianError::IdentityMalformedAccountId`] with code
///   `atlassian.identity.malformed_account_id`, mirroring the DAY-72
///   `GitLabUserId` shape check.
pub async fn discover_cloud(
    http: &HttpClient,
    auth: &BasicAuth,
    workspace_url: &Url,
    cancel: &CancellationToken,
    logs: Option<&LogSender>,
) -> Result<AtlassianCloud, DayseamError> {
    let url = workspace_url
        .join("rest/api/3/myself")
        .map_err(|e| DayseamError::Internal {
            code: "atlassian.cloud.bad_workspace_url".to_string(),
            message: format!("cannot join `/rest/api/3/myself` onto workspace URL: {e}"),
        })?;

    let request = http.reqwest().get(url).header("Accept", "application/json");
    let request = connectors_sdk::AuthStrategy::authenticate(auth, request).await?;
    let response = http.send(request, cancel, None, logs).await?;

    let status = response.status();
    if !status.is_success() {
        // Pull the body for the error message — bounded to 4 KiB so a
        // pathological upstream cannot balloon the error log.
        let body_snippet = response
            .text()
            .await
            .unwrap_or_else(|_| String::new())
            .chars()
            .take(4096)
            .collect::<String>();
        let error = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                map_status(Product::Jira, status, body_snippet)
            }
            StatusCode::NOT_FOUND => AtlassianError::CloudResourceNotFound {
                message: format!(
                    "GET /rest/api/3/myself returned 404 for {workspace_url}; the workspace \
                     URL is likely mistyped"
                ),
            },
            _ => map_status(Product::Jira, status, body_snippet),
        };
        return Err(error.into());
    }

    let parsed: MyselfResponse =
        response
            .json()
            .await
            .map_err(|e| AtlassianError::WalkShapeChanged {
                product: Product::Jira,
                message: format!("could not parse /myself response: {e}"),
            })?;

    if let Err(err) = validate_account_id(&parsed.account_id) {
        if let Some(sender) = logs {
            sender.send(
                LogLevel::Warn,
                None,
                "atlassian: /myself returned a malformed accountId".to_string(),
                serde_json::json!({
                    "observed": parsed.account_id,
                }),
            );
        }
        return Err(err.into());
    }

    Ok(AtlassianCloud {
        workspace_url: workspace_url.clone(),
        account: AtlassianAccountInfo {
            account_id: parsed.account_id,
            display_name: parsed.display_name,
            email: parsed.email_address,
            cloud_id: None,
        },
    })
}
