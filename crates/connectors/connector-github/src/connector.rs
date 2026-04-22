//! [`SourceConnector`] implementation + per-source multiplexer.
//!
//! The shape mirrors [`connector_jira::JiraMux`] and
//! [`connector_gitlab::GitlabMux`] one-for-one:
//!
//! 1. The orchestrator registry is keyed by [`SourceKind`] and stores
//!    a single trait-object handle per kind. GitHub, like GitLab and
//!    Jira, needs one inner handle per configured source (each
//!    carries its own `api_base_url`), so the registered value is a
//!    [`GithubMux`] that dispatches [`SourceConnector::sync`] by
//!    `ctx.source_id` to the right [`GithubConnector`] instance.
//!
//! 2. `GithubConnector::sync` returns
//!    [`DayseamError::Unsupported`] for every [`SyncRequest`] variant
//!    in this scaffold PR. DAY-96 flips the `SyncRequest::Day` arm
//!    onto the events-endpoint + search-driven walker; keeping the
//!    unsupported-today wiring in this diff lets the scaffold and
//!    the walker land as two independently-reviewable PRs — the
//!    precedent DAY-76 / DAY-77 set for Jira and DAY-79 / DAY-80 set
//!    for Confluence.
//!
//! 3. `healthcheck` issues `GET {api_base_url}/user` against whatever
//!    [`AuthStrategy`] the orchestrator attached to the context. A
//!    green probe proves the stored PAT still authenticates and the
//!    API base URL still resolves — exactly what the "Test
//!    connection" button in Settings (DAY-99 UI) will want. We
//!    deliberately do **not** call back into [`crate::auth::validate_auth`]
//!    here: that helper is specialised to the Add-Source flow (it
//!    consumes a freshly-built `&PatAuth`), while `healthcheck` has
//!    to operate on the generic `Arc<dyn AuthStrategy>` the
//!    orchestrator hands us — identical to how Jira's and GitLab's
//!    `healthcheck` work.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{FixedOffset, Utc};
use connectors_sdk::{ConnCtx, SourceConnector, SyncRequest, SyncResult, SyncStats};
use dayseam_core::{error_codes, DayseamError, ProgressPhase, SourceHealth, SourceId, SourceKind};
use reqwest::StatusCode;
use tokio::sync::RwLock;

use crate::config::GithubConfig;
use crate::errors::map_status;
use crate::walk::walk_day;

/// One configured GitHub source. Holds only the per-source
/// configuration that does **not** live in the
/// [`connectors_sdk::PatAuth`] attached to each [`ConnCtx`]. Cloning
/// is cheap — `GithubConfig` is a `Clone` of a single parsed URL.
///
/// `local_tz` is the user's configured timezone, threaded through from
/// [`GithubMux::new`] so the walker can compute the correct UTC window
/// for a local day.
#[derive(Debug, Clone)]
pub struct GithubConnector {
    config: GithubConfig,
    local_tz: FixedOffset,
}

impl GithubConnector {
    /// Construct a connector handle for a single GitHub source.
    /// `local_tz` defaults to UTC when the connector is built outside
    /// a [`GithubMux`]; production paths always go through the mux
    /// and inherit the orchestrator's configured offset.
    #[must_use]
    pub fn new(config: GithubConfig) -> Self {
        Self::with_local_tz(config, FixedOffset::east_opt(0).expect("0 offset"))
    }

    /// Construct a connector handle with an explicit `local_tz`. The
    /// mux uses this variant so every connector in the map shares
    /// whatever timezone the orchestrator was booted with.
    #[must_use]
    pub fn with_local_tz(config: GithubConfig, local_tz: FixedOffset) -> Self {
        Self { config, local_tz }
    }

    /// Borrow the configured API base URL. Exposed for the Settings
    /// UI (and DAY-96 tests) to render "currently connected to
    /// `<api_base_url>`" text without having to reach into
    /// `PatAuth::descriptor`.
    #[must_use]
    pub fn config(&self) -> &GithubConfig {
        &self.config
    }
}

#[async_trait]
impl SourceConnector for GithubConnector {
    fn kind(&self) -> SourceKind {
        SourceKind::GitHub
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        let url =
            self.config
                .api_base_url
                .join("user")
                .map_err(|e| DayseamError::InvalidConfig {
                    code: "github.config.bad_api_base_url".to_string(),
                    message: format!("cannot join `/user` onto API base URL: {e}"),
                })?;
        let request = ctx
            .http
            .reqwest()
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
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
                // Non-retryable non-success (the SDK already collapsed
                // 429 + 5xx into its own variants on our behalf);
                // classify the raw status through the connector's
                // own taxonomy so the health card carries a
                // `github.*` code, not a generic `http.*` string.
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
                    message: "github connector v0.4 only services SyncRequest::Day; \
                             Range + Since land with v0.5's incremental scheduler"
                        .to_string(),
                });
            }
        };

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Starting {
                message: format!("Fetching GitHub activity for {day}"),
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
                    "GitHub fetched {} row(s), emitted {} event(s)",
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

/// Per-source configuration the [`GithubMux`] needs to hydrate one
/// [`GithubConnector`]. One entry per [`dayseam_core::SourceConfig::GitHub`]
/// row; populated at startup (boot-only hydration, ARC-01) and updated
/// by the Add-Source / Reconnect flow in DAY-99.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubSourceCfg {
    pub source_id: SourceId,
    pub config: GithubConfig,
}

/// Multiplexing [`SourceConnector`] for GitHub.
///
/// Semantically identical to [`connector_jira::JiraMux`] and
/// [`connector_gitlab::GitlabMux`]: an
/// `Arc<RwLock<HashMap<SourceId, GithubConnector>>>` the Add-Source /
/// Reconnect flow can upsert into without rebuilding the registry.
/// `local_tz` is shared by every inner connector so a single user
/// timezone applies across all GitHub accounts.
#[derive(Debug, Clone)]
pub struct GithubMux {
    local_tz: FixedOffset,
    inner: Arc<RwLock<HashMap<SourceId, GithubConnector>>>,
}

impl Default for GithubMux {
    fn default() -> Self {
        Self::new(
            FixedOffset::east_opt(0).expect("0 offset"),
            std::iter::empty(),
        )
    }
}

impl GithubMux {
    /// Build a mux pre-populated with `sources`. Empty iterators are
    /// the common case at boot on a brand-new install.
    #[must_use]
    pub fn new(local_tz: FixedOffset, sources: impl IntoIterator<Item = GithubSourceCfg>) -> Self {
        let mut map = HashMap::new();
        for cfg in sources {
            map.insert(
                cfg.source_id,
                GithubConnector::with_local_tz(cfg.config, local_tz),
            );
        }
        Self {
            local_tz,
            inner: Arc::new(RwLock::new(map)),
        }
    }

    /// Add or replace the inner connector for `cfg.source_id`.
    pub async fn upsert(&self, cfg: GithubSourceCfg) {
        let conn = GithubConnector::with_local_tz(cfg.config, self.local_tz);
        self.inner.write().await.insert(cfg.source_id, conn);
    }

    /// Remove the inner connector for `source_id`, if any.
    pub async fn remove(&self, source_id: SourceId) {
        self.inner.write().await.remove(&source_id);
    }

    /// Test-only: how many sources are currently registered. The
    /// shipping code uses `get(&ctx.source_id)` instead.
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
impl SourceConnector for GithubMux {
    fn kind(&self) -> SourceKind {
        SourceKind::GitHub
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.healthcheck(ctx).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no GitHub source registered for id {}", ctx.source_id),
            }),
        }
    }

    async fn sync(&self, ctx: &ConnCtx, request: SyncRequest) -> Result<SyncResult, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.sync(ctx, request).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no GitHub source registered for id {}", ctx.source_id),
            }),
        }
    }
}
