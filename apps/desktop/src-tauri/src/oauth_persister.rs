//! Desktop-side [`TokenPersister`] implementation that writes refreshed
//! token pairs to the OS Keychain via [`dayseam_secrets`].
//!
//! The SDK defines [`TokenPersister`] in `connectors-sdk::oauth` and
//! deliberately doesn't depend on `dayseam-secrets` (the cross-crate
//! layering guard forbids it). The desktop binary is the first layer
//! that *does* know about the Keychain, so this is where the trait
//! gets implemented against real storage.
//!
//! One [`KeychainTokenPersister`] binds exactly one
//! `(service, access_account, refresh_account)` triple, so
//! [`OAuthAuth::refresh_if_expired`] always writes both rows for the
//! same source. DAY-203's `outlook_sources_add` names those accounts
//! from the source's database id so the Keychain row survives
//! renames/relabels without an additional schema column pointing at
//! the secret slot.
//!
//! [`TokenPersister`]: connectors_sdk::TokenPersister
//! [`OAuthAuth::refresh_if_expired`]: connectors_sdk::OAuthAuth

use std::sync::Arc;

use async_trait::async_trait;
use connectors_sdk::{TokenPair, TokenPersister};
use dayseam_core::{error_codes, DayseamError};
use dayseam_secrets::{oauth as oauth_secrets, Secret, SecretStore};

/// Persister bound to one Outlook source's two Keychain rows
/// (`service::access_account` and `service::refresh_account`).
/// Construct via [`KeychainTokenPersister::for_source`].
#[derive(Clone)]
pub struct KeychainTokenPersister {
    secrets: Arc<dyn SecretStore>,
    service: String,
    access_account: String,
    refresh_account: String,
}

impl KeychainTokenPersister {
    /// Build a persister for the two Keychain rows the desktop layer
    /// writes on behalf of `source_id`. DAY-203 uses the stable-id
    /// naming convention (`source:{id}.oauth.access` /
    /// `source:{id}.oauth.refresh`) so the `build_source_auth` read
    /// path can derive the same pair at app start without a schema
    /// column round-trip.
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        service: impl Into<String>,
        access_account: impl Into<String>,
        refresh_account: impl Into<String>,
    ) -> Self {
        Self {
            secrets,
            service: service.into(),
            access_account: access_account.into(),
            refresh_account: refresh_account.into(),
        }
    }
}

#[async_trait]
impl TokenPersister for KeychainTokenPersister {
    async fn persist_pair(&self, pair: &TokenPair) -> Result<(), DayseamError> {
        // The keychain backend is synchronous (macOS Security
        // Framework). Capture the two strings we need, hand them to
        // `spawn_blocking`, and bridge its `Result` back into an
        // async answer. Cloning the `Arc<dyn SecretStore>` is cheap
        // and keeps the blocking closure `'static`.
        let secrets = Arc::clone(&self.secrets);
        let service = self.service.clone();
        let access_account = self.access_account.clone();
        let refresh_account = self.refresh_account.clone();
        let access_secret = Secret::new(pair.access_token.clone());
        let refresh_secret = Secret::new(pair.refresh_token.clone());

        tokio::task::spawn_blocking(move || -> Result<(), DayseamError> {
            oauth_secrets::put_access_token(
                secrets.as_ref(),
                &service,
                &access_account,
                access_secret,
            )
            .map_err(|err| DayseamError::Internal {
                code: error_codes::IPC_OUTLOOK_KEYCHAIN_WRITE_FAILED.to_string(),
                message: format!("failed to write Outlook access token: {err}"),
            })?;
            oauth_secrets::put_refresh_token(
                secrets.as_ref(),
                &service,
                &refresh_account,
                refresh_secret,
            )
            .map_err(|err| DayseamError::Internal {
                code: error_codes::IPC_OUTLOOK_KEYCHAIN_WRITE_FAILED.to_string(),
                message: format!("failed to write Outlook refresh token: {err}"),
            })?;
            Ok(())
        })
        .await
        .map_err(|err| DayseamError::Internal {
            code: error_codes::IPC_OUTLOOK_KEYCHAIN_WRITE_FAILED.to_string(),
            message: format!("keychain write task panicked: {err}"),
        })??;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use dayseam_secrets::InMemoryStore;

    fn stub_pair() -> TokenPair {
        TokenPair::new(
            "access-token-value",
            "refresh-token-value",
            Utc::now() + chrono::Duration::hours(1),
            vec!["Calendars.Read".to_string(), "offline_access".to_string()],
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn persist_pair_writes_both_rows() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryStore::new());
        let persister = KeychainTokenPersister::new(
            Arc::clone(&store),
            "dayseam.outlook",
            "source:abc.oauth.access",
            "source:abc.oauth.refresh",
        );

        persister
            .persist_pair(&stub_pair())
            .await
            .expect("persist succeeds");

        let access = oauth_secrets::get_access_token(
            store.as_ref(),
            "dayseam.outlook",
            "source:abc.oauth.access",
        )
        .expect("get access ok")
        .expect("access row present");
        assert_eq!(access.expose_secret(), "access-token-value");

        let refresh = oauth_secrets::get_refresh_token(
            store.as_ref(),
            "dayseam.outlook",
            "source:abc.oauth.refresh",
        )
        .expect("get refresh ok")
        .expect("refresh row present");
        assert_eq!(refresh.expose_secret(), "refresh-token-value");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn persist_pair_overwrites_existing_rows() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryStore::new());
        let persister = KeychainTokenPersister::new(
            Arc::clone(&store),
            "dayseam.outlook",
            "source:abc.oauth.access",
            "source:abc.oauth.refresh",
        );

        persister.persist_pair(&stub_pair()).await.unwrap();
        let second = TokenPair::new(
            "rotated-access",
            "rotated-refresh",
            Utc::now() + chrono::Duration::hours(2),
            vec!["Calendars.Read".to_string()],
        );
        persister.persist_pair(&second).await.unwrap();

        let access = oauth_secrets::get_access_token(
            store.as_ref(),
            "dayseam.outlook",
            "source:abc.oauth.access",
        )
        .unwrap()
        .unwrap();
        assert_eq!(access.expose_secret(), "rotated-access");
    }
}
