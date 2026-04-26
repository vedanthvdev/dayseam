//! [`SourceConnector`] implementation + per-source multiplexer.
//!
//! The shape mirrors [`connector_github::GithubMux`] one-for-one:
//!
//! 1. The orchestrator registry is keyed by [`SourceKind`]. Outlook
//!    needs one inner handle per configured source (each carries its
//!    own tenant id + UPN), so the registered value is an
//!    [`OutlookMux`] that dispatches [`SourceConnector::sync`] by
//!    `ctx.source_id` to the right [`OutlookConnector`] instance.
//! 2. `OutlookConnector::sync` routes `SyncRequest::Day` through
//!    [`crate::walk::walk_day`]; every other [`SyncRequest`] variant
//!    returns `Unsupported` until v0.10's incremental scheduler
//!    lands.
//! 3. `healthcheck` issues `GET {api_base_url}/me` against whatever
//!    [`AuthStrategy`] the orchestrator attached to the context —
//!    identical to how the GitHub / GitLab / Jira connectors work.
//!    A green probe proves the refresh-token is still valid and the
//!    Graph endpoint still resolves. We deliberately do **not** call
//!    [`crate::auth::validate_auth`] here: that helper is
//!    specialised to the Add-Source flow; `healthcheck` operates on
//!    the generic `Arc<dyn AuthStrategy>` the orchestrator hands us.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{FixedOffset, Utc};
use connectors_sdk::{ConnCtx, SourceConnector, SyncRequest, SyncResult, SyncStats};
use dayseam_core::{error_codes, DayseamError, ProgressPhase, SourceHealth, SourceId, SourceKind};
use reqwest::StatusCode;
use tokio::sync::RwLock;

use crate::config::OutlookConfig;
use crate::errors::map_status;
use crate::walk::walk_day;

/// One configured Outlook source. Holds the per-source configuration
/// (tenant id, UPN, API base URL); the OAuth tokens themselves live
/// in the keychain and surface through the [`ConnCtx::auth`] the
/// orchestrator attaches. Cloning is cheap.
///
/// `local_tz` is the user's configured timezone, threaded through
/// from [`OutlookMux::new`] so the walker can compute the correct
/// UTC window for a local day.
#[derive(Debug, Clone)]
pub struct OutlookConnector {
    config: OutlookConfig,
    local_tz: FixedOffset,
}

impl OutlookConnector {
    /// Construct a connector handle for a single Outlook source.
    /// `local_tz` defaults to UTC when the connector is built outside
    /// an [`OutlookMux`]; production paths always go through the mux
    /// and inherit the orchestrator's configured offset.
    #[must_use]
    pub fn new(config: OutlookConfig) -> Self {
        Self::with_local_tz(config, FixedOffset::east_opt(0).expect("0 offset"))
    }

    /// Construct a connector handle with an explicit `local_tz`.
    #[must_use]
    pub fn with_local_tz(config: OutlookConfig, local_tz: FixedOffset) -> Self {
        Self { config, local_tz }
    }

    /// Borrow the configured API base URL + tenant metadata. Exposed
    /// for the Settings UI so it can render "Connected as `<upn>`"
    /// without having to reach into the keychain.
    #[must_use]
    pub fn config(&self) -> &OutlookConfig {
        &self.config
    }
}

#[async_trait]
impl SourceConnector for OutlookConnector {
    fn kind(&self) -> SourceKind {
        SourceKind::Outlook
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        let url = self
            .config
            .api_base_url
            .join("me")
            .map_err(|e| DayseamError::InvalidConfig {
                code: "outlook.config.bad_api_base_url".to_string(),
                message: format!("cannot join `/me` onto API base URL: {e}"),
            })?;
        let request = ctx
            .http
            .reqwest()
            .get(url)
            .header("Accept", "application/json");
        let request = ctx.auth.authenticate(request).await?;
        let response = ctx
            .http
            .send(request, &ctx.cancel, Some(&ctx.progress), Some(&ctx.logs))
            .await;

        match response {
            Ok(r) if r.status().is_success() => Ok(SourceHealth {
                ok: true,
                checked_at: Some(Utc::now()),
                last_error: None,
            }),
            Ok(r) => {
                let status: StatusCode = r.status();
                let body = r
                    .text()
                    .await
                    .unwrap_or_else(|_| String::new())
                    .chars()
                    .take(4096)
                    .collect::<String>();
                let err: DayseamError = map_status(status, body).into();
                Ok(SourceHealth {
                    ok: false,
                    checked_at: Some(Utc::now()),
                    last_error: Some(err),
                })
            }
            Err(err) => Ok(SourceHealth {
                ok: false,
                checked_at: Some(Utc::now()),
                last_error: Some(err),
            }),
        }
    }

    async fn sync(&self, ctx: &ConnCtx, request: SyncRequest) -> Result<SyncResult, DayseamError> {
        ctx.bail_if_cancelled()?;

        let day = match request {
            SyncRequest::Day(d) => d,
            SyncRequest::Range { .. } | SyncRequest::Since(_) => {
                return Err(DayseamError::Unsupported {
                    code: error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST.to_string(),
                    message: "outlook connector v0.9 only services SyncRequest::Day; \
                             Range + Since land with v0.10's incremental scheduler"
                        .to_string(),
                });
            }
        };

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Starting {
                message: format!("Fetching Outlook calendar for {day}"),
            },
        );

        let outcome = walk_day(
            &ctx.http,
            ctx.auth.clone(),
            &self.config.api_base_url,
            ctx.source_id,
            &ctx.source_identities,
            day,
            self.local_tz,
            &ctx.cancel,
            Some(&ctx.progress),
            Some(&ctx.logs),
        )
        .await?;

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Completed {
                message: format!(
                    "Outlook fetched {} row(s), emitted {} event(s)",
                    outcome.fetched_count,
                    outcome.events.len(),
                ),
            },
        );

        let stats = SyncStats {
            fetched_count: outcome.fetched_count,
            filtered_by_identity: outcome.filtered_by_identity,
            filtered_by_date: outcome.filtered_by_status,
            http_retries: 0,
        };

        Ok(SyncResult {
            events: outcome.events,
            artifacts: Vec::new(),
            checkpoint: None,
            stats,
            warnings: Vec::new(),
            raw_refs: Vec::new(),
        })
    }
}

/// Per-source configuration the [`OutlookMux`] needs to hydrate one
/// [`OutlookConnector`]. One entry per
/// [`dayseam_core::SourceConfig::Outlook`] row; populated at startup
/// (boot-only hydration, ARC-01) and updated by the Add-Source /
/// Reconnect flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlookSourceCfg {
    pub source_id: SourceId,
    pub config: OutlookConfig,
}

/// Multiplexing [`SourceConnector`] for Outlook. Semantically
/// identical to [`connector_github::GithubMux`].
#[derive(Debug, Clone)]
pub struct OutlookMux {
    local_tz: FixedOffset,
    inner: Arc<RwLock<HashMap<SourceId, OutlookConnector>>>,
}

impl Default for OutlookMux {
    fn default() -> Self {
        Self::new(
            FixedOffset::east_opt(0).expect("0 offset"),
            std::iter::empty(),
        )
    }
}

impl OutlookMux {
    /// Build a mux pre-populated with `sources`. Empty iterators are
    /// the common case at boot on a brand-new install.
    #[must_use]
    pub fn new(local_tz: FixedOffset, sources: impl IntoIterator<Item = OutlookSourceCfg>) -> Self {
        let mut map = HashMap::new();
        for cfg in sources {
            map.insert(
                cfg.source_id,
                OutlookConnector::with_local_tz(cfg.config, local_tz),
            );
        }
        Self {
            local_tz,
            inner: Arc::new(RwLock::new(map)),
        }
    }

    /// Add or replace the inner connector for `cfg.source_id`.
    pub async fn upsert(&self, cfg: OutlookSourceCfg) {
        let conn = OutlookConnector::with_local_tz(cfg.config, self.local_tz);
        self.inner.write().await.insert(cfg.source_id, conn);
    }

    /// Remove the inner connector for `source_id`, if any.
    pub async fn remove(&self, source_id: SourceId) {
        self.inner.write().await.remove(&source_id);
    }

    /// Test-only: how many sources are currently registered.
    #[doc(hidden)]
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Test-only: whether the mux has any sources registered.
    #[doc(hidden)]
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

#[async_trait]
impl SourceConnector for OutlookMux {
    fn kind(&self) -> SourceKind {
        SourceKind::Outlook
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.healthcheck(ctx).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no Outlook source registered for id {}", ctx.source_id),
            }),
        }
    }

    async fn sync(&self, ctx: &ConnCtx, request: SyncRequest) -> Result<SyncResult, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.sync(ctx, request).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no Outlook source registered for id {}", ctx.source_id),
            }),
        }
    }
}
