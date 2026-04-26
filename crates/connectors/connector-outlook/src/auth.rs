//! Credential-validation + identity-seed helpers for the Outlook
//! connector.
//!
//! Two entry points for the IPC layer
//! (`apps/desktop/src-tauri/src/ipc/outlook.rs`):
//!
//! 1. [`validate_auth`] — probes Microsoft Graph's `GET /me`
//!    endpoint with whatever [`AuthStrategy`] the caller supplies
//!    (production: a freshly-built [`OAuthAuth`] using the token pair
//!    the DAY-201 PKCE flow just minted). Returns the
//!    [`OutlookUserInfo`] triple the Add-Source dialog displays and
//!    the numeric `id` the identity seed consumes. 401 / 403 / 404
//!    here are mapped to registry-defined `outlook.*` codes by
//!    [`crate::errors::map_status`] so the dialog renders actionable
//!    messages without peeking at raw HTTP status codes.
//!
//! 2. [`list_identities`] — pure, synchronous transform that converts
//!    the `OutlookUserInfo` triple into the
//!    [`dayseam_core::SourceIdentity`] row the activity walker will
//!    filter by. Mirrors
//!    [`connector_github::auth::list_identities`]: emits **both** a
//!    `OutlookUserObjectId` row (the object GUID, stable across
//!    renames) and a `OutlookUserPrincipalName` row (the UPN,
//!    stable until the user's email handle changes).
//!
//! The access token is never borrowed as a `&str` by this module: it
//! lives inside the supplied [`AuthStrategy`] throughout, so the raw
//! bytes never leave the `SecretString` wrapper. Matches
//! `connector-github::auth::validate_auth`'s `&PatAuth` shape — v0.9
//! just widens the argument to `&dyn AuthStrategy` so the same helper
//! serves both OAuth and PAT flows in testing.

use connectors_sdk::{AuthStrategy, HttpClient};
use dayseam_core::{DayseamError, SourceIdentity, SourceIdentityKind};
use dayseam_events::LogSender;
use reqwest::StatusCode;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::errors::{map_status, OutlookUpstreamError};

/// Shape returned by Microsoft Graph's `GET /me` endpoint.
///
/// Graph's real response carries 30+ fields (phone numbers, office
/// location, preferred language, …). We keep only the four the
/// add-source flow needs.
///
/// `id` is the authoritative identity anchor. A user's `userPrincipalName`
/// can change when IT renames an account (mergers, acquisitions), but
/// `id` — the Entra object GUID — is stable for the lifetime of the
/// account. So the self-filter in the walker keys off `id`, and the
/// UPN is the human-readable label and the email-address fallback for
/// calendar-event organizer / attendee matching.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OutlookUserInfo {
    /// Entra ID object GUID. Stable across rename; the primary
    /// `SourceIdentityKind::OutlookUserObjectId` row is seeded off
    /// this.
    pub id: String,
    /// User Principal Name. Email-shaped; rendered in the Sources
    /// sidebar.
    #[serde(rename = "userPrincipalName")]
    pub user_principal_name: String,
    /// Display name — Graph always populates this for work/school
    /// accounts, but it's marked `Option` in case a future tenant
    /// configuration returns null.
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    /// Tenant GUID — not in the `/me` response itself but convenient
    /// to carry on the validated shape. The IPC layer fills it in
    /// from the id-token claims (DAY-202 persist path); tests can
    /// leave it as the empty string.
    #[serde(default)]
    pub tenant_id: String,
}

/// Probe a Microsoft Graph tenant with the supplied
/// [`AuthStrategy`].
///
/// Runs `GET {api_base_url}/me` and returns the echoed
/// `OutlookUserInfo` on success. Non-success statuses are funnelled
/// through [`map_status`] into a typed `DayseamError` with a
/// registered `outlook.*` code.
///
/// Matches the shape of [`connector_github::auth::validate_auth`]
/// — single request, bounded body capture on error, transport
/// failures handled by the SDK retry loop.
pub async fn validate_auth(
    http: &HttpClient,
    auth: &dyn AuthStrategy,
    api_base_url: &Url,
    cancel: &CancellationToken,
    logs: Option<&LogSender>,
) -> Result<OutlookUserInfo, DayseamError> {
    let url = api_base_url
        .join("me")
        .map_err(|e| DayseamError::InvalidConfig {
            code: "outlook.config.bad_api_base_url".to_string(),
            message: format!("cannot join `/me` onto Graph API base URL: {e}"),
        })?;

    let request = http.reqwest().get(url).header("Accept", "application/json");
    let request = auth.authenticate(request).await?;
    let response = http.send(request, cancel, None, logs).await?;

    let status = response.status();
    if status.is_success() {
        let info: OutlookUserInfo = response.json().await.map_err(|err| {
            DayseamError::from(OutlookUpstreamError::ShapeChanged {
                message: format!("failed to decode /me response: {err}"),
            })
        })?;
        return Ok(info);
    }

    let body_snippet = response
        .text()
        .await
        .unwrap_or_else(|_| String::new())
        .chars()
        .take(4096)
        .collect::<String>();

    let context = match status {
        StatusCode::NOT_FOUND => format!(
            "GET /me returned 404 for {api_base_url}; the Graph API base URL is likely wrong \
             (v0.9 only supports the fixed https://graph.microsoft.com/v1.0 endpoint): \
             {body_snippet}"
        ),
        _ => body_snippet,
    };
    Err(map_status(status, context).into())
}

/// Build the [`SourceIdentity`] rows a freshly-added Outlook source
/// needs. Returns **two** rows — one keyed off the Entra object
/// GUID (the filter-time key used when an event's `organizer.user.id`
/// is populated), and one keyed off the UPN (the filter-time key
/// used when an event only carries `organizer.emailAddress`). Both
/// rows are required for [`crate::walk::walk_day`] to self-filter
/// correctly across the two shapes Graph uses.
///
/// Shape-mirrors [`connector_github::auth::list_identities`] and its
/// two-row seed. Graph's `/me` response always populates both fields
/// for a work/school account, so missing either is treated as a
/// shape corruption.
///
/// Pure helper — no I/O, no DB writes. The caller (IPC command) is
/// responsible for persisting the returned rows inside the same
/// transaction that writes the source.
pub fn list_identities(
    info: &OutlookUserInfo,
    source_id: Uuid,
    person_id: Uuid,
    _logs: Option<&LogSender>,
) -> Result<Vec<SourceIdentity>, DayseamError> {
    if info.id.trim().is_empty() {
        return Err(DayseamError::from(OutlookUpstreamError::ShapeChanged {
            message: "Microsoft Graph /me returned an empty object id; refusing to seed an \
                      identity row that would silently match no events"
                .to_string(),
        }));
    }
    if info.user_principal_name.trim().is_empty() {
        return Err(DayseamError::from(OutlookUpstreamError::ShapeChanged {
            message: format!(
                "Microsoft Graph /me returned an empty userPrincipalName for object id {}; \
                 refusing to seed a UPN identity row that would silently match no events",
                info.id
            ),
        }));
    }
    let object_id_row = SourceIdentity {
        id: Uuid::new_v4(),
        person_id,
        source_id: Some(source_id),
        kind: SourceIdentityKind::OutlookUserObjectId,
        external_actor_id: info.id.clone(),
    };
    let upn_row = SourceIdentity {
        id: Uuid::new_v4(),
        person_id,
        source_id: Some(source_id),
        kind: SourceIdentityKind::OutlookUserPrincipalName,
        external_actor_id: info.user_principal_name.clone(),
    };
    Ok(vec![object_id_row, upn_row])
}

#[cfg(test)]
mod tests {
    use super::*;
    use dayseam_core::error_codes;

    fn sample_info() -> OutlookUserInfo {
        OutlookUserInfo {
            id: "11111111-2222-3333-4444-555555555555".into(),
            user_principal_name: "vedanth@contoso.com".into(),
            display_name: Some("Vedanth Vasudev".into()),
            tenant_id: "00000000-0000-0000-0000-000000000000".into(),
        }
    }

    #[test]
    fn outlook_user_info_decodes_and_drops_extra_fields() {
        // Graph's real `/me` response returns ~30 fields. We only
        // keep id + userPrincipalName + displayName and must silently
        // drop the rest.
        let padded = r#"{
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users/$entity",
            "id": "11111111-2222-3333-4444-555555555555",
            "userPrincipalName": "vedanth@contoso.com",
            "displayName": "Vedanth Vasudev",
            "mail": "vedanth@contoso.com",
            "jobTitle": "Engineer",
            "officeLocation": "SF/3",
            "preferredLanguage": "en-US",
            "mobilePhone": null,
            "businessPhones": []
        }"#;
        let back: OutlookUserInfo =
            serde_json::from_str(padded).expect("serde should ignore unknown fields by default");
        assert_eq!(back.id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(back.user_principal_name, "vedanth@contoso.com");
        assert_eq!(back.display_name.as_deref(), Some("Vedanth Vasudev"));
    }

    #[test]
    fn outlook_user_info_display_name_is_optional() {
        let no_name = r#"{"id":"abc","userPrincipalName":"u@e.com","displayName":null}"#;
        let back: OutlookUserInfo =
            serde_json::from_str(no_name).expect("null displayName deserialises");
        assert!(back.display_name.is_none());

        let missing = r#"{"id":"abc","userPrincipalName":"u@e.com"}"#;
        let back: OutlookUserInfo =
            serde_json::from_str(missing).expect("missing displayName deserialises");
        assert!(back.display_name.is_none());
    }

    #[test]
    fn list_identities_returns_both_object_id_and_upn_rows_on_happy_path() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info();
        let identities = list_identities(&info, source, person, None).unwrap();
        assert_eq!(identities.len(), 2);

        let obj_row = identities
            .iter()
            .find(|r| r.kind == SourceIdentityKind::OutlookUserObjectId)
            .expect("OutlookUserObjectId row present");
        assert_eq!(obj_row.person_id, person);
        assert_eq!(obj_row.source_id, Some(source));
        assert_eq!(
            obj_row.external_actor_id,
            "11111111-2222-3333-4444-555555555555"
        );

        let upn_row = identities
            .iter()
            .find(|r| r.kind == SourceIdentityKind::OutlookUserPrincipalName)
            .expect("OutlookUserPrincipalName row present");
        assert_eq!(upn_row.person_id, person);
        assert_eq!(upn_row.source_id, Some(source));
        assert_eq!(upn_row.external_actor_id, "vedanth@contoso.com");
    }

    #[test]
    fn list_identities_rejects_empty_object_id_as_shape_change() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let mut info = sample_info();
        info.id = "".into();
        let err = list_identities(&info, source, person, None).unwrap_err();
        assert_eq!(err.code(), error_codes::OUTLOOK_UPSTREAM_SHAPE_CHANGED);
        assert_eq!(err.variant(), "UpstreamChanged");
    }

    #[test]
    fn list_identities_rejects_empty_upn_as_shape_change() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let mut info = sample_info();
        info.user_principal_name = "".into();
        let err = list_identities(&info, source, person, None).unwrap_err();
        assert_eq!(err.code(), error_codes::OUTLOOK_UPSTREAM_SHAPE_CHANGED);
    }
}
