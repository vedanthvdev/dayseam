//! v0.2.1 Confluence-email recovery, generalised into a registered
//! [`SerdeDefaultRepair`].
//!
//! ## Background
//!
//! `SourceConfig::Confluence` shipped in v0.2.0 without an `email`
//! field. v0.2.1 added it as a required-at-auth-time field with
//! `#[serde(default)]` so old rows still deserialised — but the
//! default is `""`, which `build_source_auth` rejects as
//! `atlassian.auth.invalid_credentials`. Users who connected via
//! Journey A (shared PAT across Jira + Confluence) have a sibling
//! Jira row carrying the email the dialog collected; we copy it
//! across at boot so reports resume working without a manual
//! Reconnect.
//!
//! v0.2.1 implemented this as a free function in
//! `apps/desktop/src-tauri/src/startup.rs::backfill_atlassian_confluence_email`.
//! DAY-88 CORR-v0.2-08 moved it here behind the `SerdeDefaultRepair`
//! trait so future serde-default recoveries follow the same shape
//! instead of each inventing its own.
//!
//! ## Matching rule
//!
//! A Confluence row qualifies iff:
//! * `config` is `SourceConfig::Confluence` with an empty-after-trim
//!   `email`, **and**
//! * `secret_ref` is present, **and**
//! * some *other* row in the DB has the same `secret_ref`
//!   (`keychain_service` + `keychain_account`) and a non-empty
//!   Atlassian email in its `SourceConfig`.
//!
//! We deliberately do **not** fall back to matching on workspace
//! URL alone: that would risk copying the wrong email across two
//! independently-added Confluence instances on the same tenant.
//!
//! ## Idempotency
//!
//! Trivially idempotent: after the first run the candidate row has
//! a non-empty email, so it no longer qualifies. The second run
//! skips it in the first filter pass. A test in `startup.rs` and
//! [`ConfluenceEmailRepair::run`]'s own skip conditions enforce
//! this across boots.

use async_trait::async_trait;
use dayseam_core::SourceConfig;
use sqlx::SqlitePool;

use crate::repairs::SerdeDefaultRepair;
use crate::repos::sources::SourceRepo;
use crate::DbResult;

/// Boot-time repair: copy a sibling Atlassian email into a
/// Confluence row whose `email` field is empty (v0.2.0 data).
pub struct ConfluenceEmailRepair;

#[async_trait]
impl SerdeDefaultRepair for ConfluenceEmailRepair {
    fn name(&self) -> &'static str {
        "confluence_email"
    }

    async fn run(&self, pool: &SqlitePool) -> DbResult<()> {
        let sources = match SourceRepo::new(pool.clone()).list().await {
            Ok(sources) => sources,
            Err(err) => {
                // Log + swallow — the caller (startup) iterates
                // multiple repairs and one failing must not block
                // the others. A listing failure also surfaces at the
                // first IPC call that needs sources, so the
                // information is never lost.
                tracing::warn!(
                    %err,
                    repair = "confluence_email",
                    "source listing failed; skipping repair",
                );
                return Ok(());
            }
        };

        let repo = SourceRepo::new(pool.clone());
        for source in &sources {
            let workspace_url = match &source.config {
                SourceConfig::Confluence {
                    workspace_url,
                    email,
                } if email.trim().is_empty() => workspace_url.clone(),
                _ => continue,
            };

            let Some(secret_ref) = source.secret_ref.as_ref() else {
                tracing::warn!(
                    source_id = %source.id,
                    repair = "confluence_email",
                    "Confluence row has empty email and no secret_ref — \
                     user must reconnect manually",
                );
                continue;
            };

            let sibling_email = sources.iter().find_map(|other| {
                if other.id == source.id {
                    return None;
                }
                let other_ref = other.secret_ref.as_ref()?;
                if other_ref.keychain_service != secret_ref.keychain_service
                    || other_ref.keychain_account != secret_ref.keychain_account
                {
                    return None;
                }
                match &other.config {
                    SourceConfig::Jira { email, .. } | SourceConfig::Confluence { email, .. }
                        if !email.trim().is_empty() =>
                    {
                        Some(email.clone())
                    }
                    _ => None,
                }
            });

            let Some(email) = sibling_email else {
                tracing::warn!(
                    source_id = %source.id,
                    repair = "confluence_email",
                    "Confluence row has empty email and no Atlassian sibling with the \
                     same secret_ref — user must reconnect manually",
                );
                continue;
            };

            let new_config = SourceConfig::Confluence {
                workspace_url,
                email: email.clone(),
            };
            match repo.update_config(&source.id, &new_config).await {
                Ok(()) => {
                    tracing::info!(
                        source_id = %source.id,
                        repair = "confluence_email",
                        "copied sibling Atlassian email into Confluence row \
                         to recover from v0.2.0 upgrade",
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        %err,
                        source_id = %source.id,
                        repair = "confluence_email",
                        "update_config failed; user must reconnect",
                    );
                }
            }
        }
        Ok(())
    }
}
