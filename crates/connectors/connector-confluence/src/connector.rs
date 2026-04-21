//! [`SourceConnector`] implementation + per-source multiplexer for
//! Confluence.
//!
//! The shape mirrors [`connector_jira::JiraMux`] one-for-one; see
//! that type's docs for the "why a mux per kind" rationale.
//! [`SourceConnector::sync`] routes
//! [`SyncRequest::Day`] into [`crate::walk::walk_day`] (DAY-80) and
//! continues to return [`DayseamError::Unsupported`] for
//! `Range` / `Since` until v0.3's incremental scheduler — identical
//! split to the DAY-76 → DAY-77 Jira sequence.
//!
//! `healthcheck` probes `GET /rest/api/3/myself` so the Settings
//! "Test connection" button (DAY-83 UI) and the orchestrator's
//! "source healthy?" gate both key on a real authenticated round-trip.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{FixedOffset, Utc};
use connectors_sdk::{ConnCtx, SourceConnector, SyncRequest, SyncResult, SyncStats};
use dayseam_core::{error_codes, DayseamError, ProgressPhase, SourceHealth, SourceId, SourceKind};
use tokio::sync::RwLock;

use crate::config::ConfluenceConfig;
use crate::walk::walk_day;

/// One configured Confluence source. Holds only the per-source
/// configuration that does **not** live on the
/// [`connectors_sdk::BasicAuth`] attached to each [`ConnCtx`].
/// Cloning is cheap — [`ConfluenceConfig`] is one short `String`
/// wrapped in a `Url`.
///
/// `local_tz` is the user's configured timezone, threaded through
/// from [`ConfluenceMux::new`] so the CQL walker can compute the
/// correct UTC window for a local day.
#[derive(Debug, Clone)]
pub struct ConfluenceConnector {
    config: ConfluenceConfig,
    local_tz: FixedOffset,
}

impl ConfluenceConnector {
    /// Construct a connector handle for a single Confluence source.
    /// `local_tz` defaults to UTC when the connector is built outside
    /// a [`ConfluenceMux`]; production paths always go through the
    /// mux and inherit the orchestrator's configured offset.
    #[must_use]
    pub fn new(config: ConfluenceConfig) -> Self {
        Self::with_local_tz(config, FixedOffset::east_opt(0).expect("0 offset"))
    }

    /// Construct a connector handle with an explicit `local_tz`. The
    /// mux uses this variant so every connector in the map shares
    /// whatever timezone the orchestrator was booted with.
    #[must_use]
    pub fn with_local_tz(config: ConfluenceConfig, local_tz: FixedOffset) -> Self {
        Self { config, local_tz }
    }

    /// Borrow the configured workspace URL. Exposed for the Settings
    /// UI (and DAY-80 tests) to render "currently connected to
    /// `<workspace>`" text without having to reach into
    /// `BasicAuth::descriptor`.
    #[must_use]
    pub fn config(&self) -> &ConfluenceConfig {
        &self.config
    }
}

#[async_trait]
impl SourceConnector for ConfluenceConnector {
    fn kind(&self) -> SourceKind {
        SourceKind::Confluence
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        // `GET /rest/api/3/myself` — shared with Jira. Any Atlassian
        // Cloud credential that authenticates against this endpoint
        // also authenticates against the `/wiki/*` Confluence
        // surface, so this probe is the right "can we talk to
        // Confluence?" signal. Rationale matches
        // `connector_jira::connector::healthcheck`: we route through
        // `ctx.auth.authenticate(…)` (rather than calling
        // `validate_auth` a second time) so the probe uses whatever
        // auth strategy the orchestrator hands us.
        let url = self
            .config
            .workspace_url
            .join("rest/api/3/myself")
            .map_err(|e| DayseamError::InvalidConfig {
                code: "confluence.config.bad_workspace_url".to_string(),
                message: format!("cannot join `/rest/api/3/myself` onto workspace URL: {e}"),
            })?;
        let request = ctx
            .http
            .reqwest()
            .get(url)
            .header("Accept", "application/json");
        let request = ctx.auth.authenticate(request).await?;
        match ctx
            .http
            .send(request, &ctx.cancel, Some(&ctx.progress), Some(&ctx.logs))
            .await
        {
            Ok(_) => Ok(SourceHealth {
                ok: true,
                checked_at: Some(Utc::now()),
                last_error: None,
            }),
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
                    message: "confluence connector v0.2 only services SyncRequest::Day; \
                             Range + Since land with v0.3's incremental scheduler"
                        .to_string(),
                });
            }
        };

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Starting {
                message: format!("Fetching Confluence activity for {day}"),
            },
        );

        let outcome = walk_day(
            &ctx.http,
            ctx.auth.clone(),
            &self.config.workspace_url,
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
                    "Confluence fetched {} content row(s), emitted {} event(s)",
                    outcome.fetched_count,
                    outcome.events.len(),
                ),
            },
        );

        let stats = SyncStats {
            fetched_count: outcome.fetched_count,
            filtered_by_identity: outcome.filtered_by_identity,
            filtered_by_date: outcome.filtered_by_date,
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

/// Per-source configuration the [`ConfluenceMux`] needs to hydrate
/// one [`ConfluenceConnector`]. One entry per
/// [`dayseam_core::SourceConfig::Confluence`] row; populated at
/// startup (boot-only hydration, ARC-01) and updated by the
/// Add-Source / Reconnect flow in DAY-82.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfluenceSourceCfg {
    pub source_id: SourceId,
    pub config: ConfluenceConfig,
}

/// Multiplexing [`SourceConnector`] for Confluence.
///
/// Semantically identical to [`connector_jira::JiraMux`]: an
/// `Arc<RwLock<HashMap<SourceId, ConfluenceConnector>>>` the
/// Add-Source / Reconnect flow can upsert into without rebuilding the
/// registry. `local_tz` is shared by every inner connector so a single
/// user timezone applies across all Confluence workspaces.
#[derive(Debug, Clone)]
pub struct ConfluenceMux {
    local_tz: FixedOffset,
    inner: Arc<RwLock<HashMap<SourceId, ConfluenceConnector>>>,
}

impl Default for ConfluenceMux {
    fn default() -> Self {
        Self::new(
            FixedOffset::east_opt(0).expect("0 offset"),
            std::iter::empty(),
        )
    }
}

impl ConfluenceMux {
    /// Build a mux pre-populated with `sources`. Empty iterators are
    /// the common case at boot on a brand-new install.
    #[must_use]
    pub fn new(
        local_tz: FixedOffset,
        sources: impl IntoIterator<Item = ConfluenceSourceCfg>,
    ) -> Self {
        let mut map = HashMap::new();
        for cfg in sources {
            map.insert(
                cfg.source_id,
                ConfluenceConnector::with_local_tz(cfg.config, local_tz),
            );
        }
        Self {
            local_tz,
            inner: Arc::new(RwLock::new(map)),
        }
    }

    /// Add or replace the inner connector for `cfg.source_id`.
    pub async fn upsert(&self, cfg: ConfluenceSourceCfg) {
        let conn = ConfluenceConnector::with_local_tz(cfg.config, self.local_tz);
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

    /// Test-only: whether the mux has any sources registered. Paired
    /// with [`Self::len`] to keep clippy's `len_without_is_empty`
    /// happy.
    #[doc(hidden)]
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

#[async_trait]
impl SourceConnector for ConfluenceMux {
    fn kind(&self) -> SourceKind {
        SourceKind::Confluence
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.healthcheck(ctx).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no Confluence source registered for id {}", ctx.source_id),
            }),
        }
    }

    async fn sync(&self, ctx: &ConnCtx, request: SyncRequest) -> Result<SyncResult, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.sync(ctx, request).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no Confluence source registered for id {}", ctx.source_id),
            }),
        }
    }
}
