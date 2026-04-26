//! Per-source Outlook / Microsoft Graph configuration.
//!
//! A [`dayseam_core::SourceConfig::Outlook`] row stores two strings:
//! the Entra ID `tenant_id` (GUID) and the user's `user_principal_name`
//! (the UPN — an email-shaped string like `vedanth@contoso.com`). Both
//! are echoed back from Microsoft Graph's `GET /me` endpoint at
//! credential-validation time and persisted on the source row so the
//! walker can compose the Graph REST URLs without re-probing `/me` on
//! every sync. The OAuth tokens themselves live in the keychain via
//! the source's `secret_ref`; this config carries nothing secret.
//!
//! The [`OutlookConfig`] struct is the typed form of those two
//! strings. Hydration (at startup) parses the row into a
//! [`OutlookConfig`] and threads it into
//! [`crate::connector::OutlookConnector::new`] so the walker can
//! assume well-formed `tenant_id` / `upn` values without re-validating
//! on every request.
//!
//! The Graph API base URL is a fixed constant
//! (`https://graph.microsoft.com/v1.0`) rather than a per-source
//! field: Microsoft does not offer an on-premises Graph endpoint, so
//! unlike GitHub's Enterprise Server support there is no legitimate
//! reason to let a source override the host. Locking it down defends
//! against a typo or a compromised source row pointing the
//! OAuth-bearer walker at an attacker-controlled domain — the token
//! would travel, and Graph tokens are minted against `graph.microsoft.com`
//! so the endpoint is load-bearing for the token's audience check.

use url::Url;

/// Microsoft Graph v1.0 REST root. Fixed — not a per-source override.
/// Stored with a trailing slash so [`Url::join`] verbatim produces
/// `…/v1.0/me`, `…/v1.0/me/calendarView`, etc.
pub const GRAPH_API_BASE_URL: &str = "https://graph.microsoft.com/v1.0/";

/// Typed view of a [`dayseam_core::SourceConfig::Outlook`] row.
///
/// Constructed once at hydration time and threaded into
/// [`crate::connector::OutlookConnector::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlookConfig {
    /// Entra ID tenant GUID. Recorded on the source row so two Outlook
    /// connections under the same `dayseam.outlook` keychain service
    /// (e.g. two work tenants at the same company) remain distinguishable
    /// in logs and in the UI.
    pub tenant_id: String,
    /// User Principal Name — the email-like string Graph echoes back
    /// from `GET /me.userPrincipalName`. Used as the display label
    /// in the Sources sidebar and as the stable `SourceIdentityKind::OutlookUserPrincipalName`
    /// key in the self-filter.
    pub user_principal_name: String,
    /// Fixed Graph API base URL (parsed form of [`GRAPH_API_BASE_URL`]).
    /// Not persisted on the source row — derived at hydration so
    /// every in-memory `OutlookConfig` shares a single parsed
    /// [`Url`] instance.
    pub api_base_url: Url,
}

impl OutlookConfig {
    /// Construct a [`OutlookConfig`] from the raw
    /// `SourceConfig::Outlook` fields. Currently infallible because
    /// both inputs are free-form strings; a future refactor that
    /// validates the `tenant_id` as a GUID would widen this to
    /// `Result`.
    pub fn from_raw(tenant_id: impl Into<String>, user_principal_name: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            user_principal_name: user_principal_name.into(),
            api_base_url: Url::parse(GRAPH_API_BASE_URL)
                .expect("Graph v1.0 API base URL parses as a well-formed URL"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_raw_produces_well_formed_graph_urls() {
        let cfg = OutlookConfig::from_raw(
            "00000000-0000-0000-0000-000000000000",
            "vedanth@contoso.com",
        );
        assert_eq!(
            cfg.api_base_url.join("me").unwrap().as_str(),
            "https://graph.microsoft.com/v1.0/me"
        );
        assert_eq!(
            cfg.api_base_url.join("me/calendarView").unwrap().as_str(),
            "https://graph.microsoft.com/v1.0/me/calendarView"
        );
    }

    #[test]
    fn from_raw_preserves_tenant_and_upn_verbatim() {
        let cfg = OutlookConfig::from_raw("tenant-guid", "user@example.com");
        assert_eq!(cfg.tenant_id, "tenant-guid");
        assert_eq!(cfg.user_principal_name, "user@example.com");
    }
}
