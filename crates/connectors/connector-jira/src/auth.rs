//! Credential-validation + identity-seed helpers for the Jira
//! connector.
//!
//! These are the two entry points the IPC layer (DAY-82 Add-Source
//! dialog) calls when a user pastes an Atlassian email + API token:
//!
//! 1. [`validate_auth`] — cheap, read-only probe of
//!    `GET <workspace>/rest/api/3/myself`. Returns the account triple
//!    the dialog displays ("Connected as …") and the `accountId` the
//!    next step consumes. A 401/403/404 here is mapped to the
//!    registry-defined `atlassian.*` codes by
//!    [`connector_atlassian_common::discover_cloud`] so the dialog
//!    can render actionable messages without peeking at raw HTTP
//!    status codes.
//!
//! 2. [`list_identities`] — pure, synchronous transform that converts
//!    the account triple into the
//!    [`dayseam_core::SourceIdentity`] row the activity walker will
//!    later filter by. Kept out of `validate_auth` so the IPC layer
//!    can run the identity seed inside the same DB transaction that
//!    writes the new [`dayseam_core::Source`] row — mirrors
//!    `ensure_gitlab_self_identity` in the Tauri commands module.
//!
//! Both helpers delegate to [`connector_atlassian_common`]; the Jira
//! crate exists to keep the IPC call site crate-scoped (the
//! Confluence sibling will expose its own equivalents in DAY-79).

use connector_atlassian_common::{
    discover_cloud, seed_atlassian_identity, AtlassianAccountInfo, AtlassianCloud,
};
use connectors_sdk::{BasicAuth, HttpClient};
use dayseam_core::{DayseamError, SourceIdentity};
use dayseam_events::LogSender;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

/// Probe a Jira Cloud workspace with the supplied Basic-auth
/// credential.
///
/// Thin wrapper around
/// [`connector_atlassian_common::discover_cloud`]; kept in this crate
/// so the IPC layer has a single `connector_jira::validate_auth`
/// symbol to import rather than reaching into
/// `connector_atlassian_common` directly. That keeps the Confluence
/// sibling (DAY-79) a parallel concern: `connector_confluence` will
/// expose its own `validate_auth` that, per the spike, may probe a
/// *different* endpoint (`/wiki/rest/api/user/current`) when a
/// Confluence-only token is in play.
///
/// Errors surface verbatim from the common crate; see
/// [`discover_cloud`] for the full taxonomy.
pub async fn validate_auth(
    http: &HttpClient,
    auth: &BasicAuth,
    workspace_url: &Url,
    cancel: &CancellationToken,
    logs: Option<&LogSender>,
) -> Result<AtlassianCloud, DayseamError> {
    discover_cloud(http, auth, workspace_url, cancel, logs).await
}

/// Build the [`SourceIdentity`] rows a freshly-added Jira source
/// needs. Today that is exactly one row (the self-identity), so the
/// return shape is `Vec<SourceIdentity>` with a single element; the
/// `Vec` type is chosen so a future "also mirror the reporter's
/// alternate account" extension doesn't require a signature change
/// at every IPC caller.
///
/// Pure helper — no I/O, no DB writes. The caller (DAY-82 IPC
/// command) is responsible for persisting the returned rows inside
/// the same transaction that writes the source. Mirrors
/// `ensure_gitlab_self_identity` in spirit: identity-on-credentials
/// instead of identity-on-first-sync, because the v0.1 post-mortem
/// (DAY-71) showed the delayed-seed flow silently drops every event
/// produced before the first seed commits.
pub fn list_identities(
    info: &AtlassianAccountInfo,
    source_id: Uuid,
    person_id: Uuid,
    logs: Option<&LogSender>,
) -> Result<Vec<SourceIdentity>, DayseamError> {
    let identity = seed_atlassian_identity(info, source_id, person_id, logs)?;
    Ok(vec![identity])
}

#[cfg(test)]
mod tests {
    use super::*;
    use dayseam_core::{error_codes, SourceIdentityKind};

    fn sample_info(account_id: &str) -> AtlassianAccountInfo {
        AtlassianAccountInfo {
            account_id: account_id.into(),
            display_name: "Vedanth Vasudev".into(),
            email: Some("vedanth@acme.com".into()),
            cloud_id: None,
        }
    }

    #[test]
    fn list_identities_returns_exactly_one_row_on_happy_path() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info("5d53f3cbc6b9320d9ea5bdc2");
        let identities = list_identities(&info, source, person, None).unwrap();
        assert_eq!(identities.len(), 1);
        let row = &identities[0];
        assert_eq!(row.person_id, person);
        assert_eq!(row.source_id, Some(source));
        assert_eq!(row.kind, SourceIdentityKind::AtlassianAccountId);
        assert_eq!(row.external_actor_id, "5d53f3cbc6b9320d9ea5bdc2");
    }

    #[test]
    fn list_identities_propagates_malformed_account_id_error() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info("");
        let err = list_identities(&info, source, person, None).unwrap_err();
        assert_eq!(
            err.code(),
            error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID
        );
    }
}
