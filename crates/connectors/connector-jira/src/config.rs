//! Per-source Jira configuration carried on a
//! [`dayseam_core::SourceConfig::Jira`] row once it has been parsed
//! into a `Url`.
//!
//! The core-types row holds `workspace_url` as a `String` (because the
//! serde-round-trip tests want `SourceConfig` to stay `PartialEq` +
//! `Eq`, and `url::Url` is not); this crate parses that string into a
//! stricter [`Url`] the moment the source is hydrated, so every
//! downstream call in `auth.rs` / `connector.rs` / (DAY-77) `walk.rs`
//! can assume a well-formed URL with a scheme and a host.

use url::Url;

/// Typed view of a [`dayseam_core::SourceConfig::Jira`] row.
///
/// Constructed once at hydration time (DAY-82 IPC / startup backfill)
/// and threaded into [`crate::connector::JiraConnector::new`]. The
/// `email` is redundant against `BasicAuth::descriptor`'s email, but
/// carrying it here too lets `validate_auth` and `list_identities`
/// take a single `&JiraConfig` rather than a `(Url, email)` tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraConfig {
    /// Canonical Jira Cloud workspace URL, e.g.
    /// `https://acme.atlassian.net/`. Stored with a trailing slash so
    /// [`Url::join`] does not silently drop the last path segment
    /// when appending `rest/api/3/…`.
    pub workspace_url: Url,
    /// The email the Basic-auth header will be built from. Kept here
    /// (rather than solely on [`connectors_sdk::BasicAuth::descriptor`])
    /// so the durable `SourceConfig::Jira` row is self-describing —
    /// the IPC layer can rebuild an equivalent `BasicAuth` from
    /// `(email, keychain_secret_ref)` after a restart without needing
    /// to reach into the auth strategy's internals.
    pub email: String,
}

impl JiraConfig {
    /// Construct a [`JiraConfig`] from the raw `SourceConfig::Jira`
    /// fields. Ensures `workspace_url` carries a trailing slash so
    /// every caller can use [`Url::join`] verbatim.
    pub fn from_raw(
        workspace_url: &str,
        email: impl Into<String>,
    ) -> Result<Self, url::ParseError> {
        let with_slash = if workspace_url.ends_with('/') {
            workspace_url.to_string()
        } else {
            format!("{workspace_url}/")
        };
        Ok(Self {
            workspace_url: Url::parse(&with_slash)?,
            email: email.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_raw_pads_trailing_slash_so_url_join_is_safe() {
        let cfg = JiraConfig::from_raw("https://acme.atlassian.net", "vedanth@acme.com").unwrap();
        // `Url::join` on a URL without a trailing slash would drop
        // the last path segment; the padding here defends against
        // that silent footgun.
        let joined = cfg.workspace_url.join("rest/api/3/myself").unwrap();
        assert_eq!(
            joined.as_str(),
            "https://acme.atlassian.net/rest/api/3/myself"
        );
    }

    #[test]
    fn from_raw_is_idempotent_across_trailing_slashes() {
        let a = JiraConfig::from_raw("https://acme.atlassian.net", "v@acme.com").unwrap();
        let b = JiraConfig::from_raw("https://acme.atlassian.net/", "v@acme.com").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn from_raw_rejects_malformed_urls() {
        assert!(JiraConfig::from_raw("not a url", "v@acme.com").is_err());
    }
}
