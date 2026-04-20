//! `SourceConnector` trait implementation.
//!
//! [`GitlabConnector`] wires the modules together:
//!
//! 1. `sync` takes a [`SyncRequest::Day`] (everything else returns
//!    `Unsupported`, matching the v0.1 local-git shape).
//! 2. Delegates to [`crate::walk::walk_day`] for pagination, identity
//!    filtering, and normalisation.
//! 3. Emits a `Starting` progress event up-front and a `Completed`
//!    event at the end; in-flight retry progress is emitted by the
//!    shared [`connectors_sdk::HttpClient`].
//! 4. Returns a [`SyncResult`] with a deterministic event list and
//!    no artefacts — the GitLab-side artefact model (MR threads,
//!    issue discussions) lands in Task 2.
//!
//! The connector's entire `sync` implementation lives below one
//! screen so the invariants in the Phase 3 plan can be verified at a
//! glance.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{FixedOffset, Utc};
use connectors_sdk::{ConnCtx, SourceConnector, SyncRequest, SyncResult, SyncStats};
use dayseam_core::{error_codes, DayseamError, ProgressPhase, SourceHealth, SourceId, SourceKind};
use tokio::sync::RwLock;

use crate::walk::walk_day;

/// Configured GitLab source connector instance. One per configured
/// `SourceConfig::GitLab` row.
#[derive(Debug, Clone)]
pub struct GitlabConnector {
    /// The GitLab host, e.g. `"https://gitlab.com"` or
    /// `"https://git.company.io"`. Stored without a trailing slash;
    /// the walker appends `/api/v4/...`.
    base_url: String,
    /// The numeric user id to walk events for. Captured from the
    /// `/user` probe when the source was added, never the stale
    /// value from a renamed username.
    user_id: i64,
    /// User's local timezone at sync time. The orchestrator captures
    /// this once per run so a daylight-saving transition in the
    /// middle of a sync does not re-bucket half the events.
    local_tz: FixedOffset,
}

impl GitlabConnector {
    pub fn new(base_url: impl Into<String>, user_id: i64, local_tz: FixedOffset) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url,
            user_id,
            local_tz,
        }
    }
}

#[async_trait]
impl SourceConnector for GitlabConnector {
    fn kind(&self) -> SourceKind {
        SourceKind::GitLab
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        // GET /user via the configured auth strategy. Surfaces token
        // validity, DNS reachability, and TLS correctness in a
        // single call — exactly what the Settings "Test connection"
        // button needs.
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/api/v4/user");
        let request = ctx.http.reqwest().get(&url);
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
                    message:
                        "gitlab connector v0.1 only services SyncRequest::Day; Range lands in v0.2"
                            .to_string(),
                });
            }
        };

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Starting {
                message: format!("Fetching GitLab events for {day}"),
            },
        );

        let outcome = walk_day(
            &ctx.http,
            ctx.auth.clone(),
            &self.base_url,
            self.user_id,
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
                    "GitLab fetched {} event(s), {} filtered by identity, {} dropped by shape",
                    outcome.events.len(),
                    outcome.filtered_by_identity,
                    outcome.dropped_by_shape,
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

/// Per-source configuration the [`GitlabMux`] needs to route a sync
/// call. Populated from each [`dayseam_core::SourceConfig::GitLab`]
/// row at orchestrator startup + whenever a GitLab source is added,
/// updated, or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitlabSourceCfg {
    pub source_id: SourceId,
    pub base_url: String,
    pub user_id: i64,
}

/// Multiplexing [`SourceConnector`] implementation.
///
/// The orchestrator registry is keyed by [`SourceKind`] and therefore
/// stores a single connector handle per kind. GitLab needs one handle
/// per configured source (each has its own `base_url` + `user_id`),
/// so the registered entry is a `GitlabMux` that dispatches
/// [`SourceConnector::sync`] by `ctx.source_id` to an inner
/// [`GitlabConnector`] instance.
///
/// The inner map is behind an `RwLock` so Task 3's add-source /
/// Reconnect flow can add / remove / rotate sources without rebuilding
/// the registry.
#[derive(Debug, Clone)]
pub struct GitlabMux {
    local_tz: FixedOffset,
    inner: Arc<RwLock<HashMap<SourceId, GitlabConnector>>>,
}

impl GitlabMux {
    pub fn new(local_tz: FixedOffset, sources: impl IntoIterator<Item = GitlabSourceCfg>) -> Self {
        let mut map = HashMap::new();
        for cfg in sources {
            map.insert(
                cfg.source_id,
                GitlabConnector::new(cfg.base_url, cfg.user_id, local_tz),
            );
        }
        Self {
            local_tz,
            inner: Arc::new(RwLock::new(map)),
        }
    }

    /// Add or replace the inner connector for `cfg.source_id`.
    pub async fn upsert(&self, cfg: GitlabSourceCfg) {
        let conn = GitlabConnector::new(cfg.base_url, cfg.user_id, self.local_tz);
        self.inner.write().await.insert(cfg.source_id, conn);
    }

    /// Remove the inner connector for `source_id`, if any.
    pub async fn remove(&self, source_id: SourceId) {
        self.inner.write().await.remove(&source_id);
    }
}

#[async_trait]
impl SourceConnector for GitlabMux {
    fn kind(&self) -> SourceKind {
        SourceKind::GitLab
    }

    async fn healthcheck(&self, ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.healthcheck(ctx).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no GitLab source registered for id {}", ctx.source_id),
            }),
        }
    }

    async fn sync(&self, ctx: &ConnCtx, request: SyncRequest) -> Result<SyncResult, DayseamError> {
        match self.inner.read().await.get(&ctx.source_id).cloned() {
            Some(c) => c.sync(ctx, request).await,
            None => Err(DayseamError::InvalidConfig {
                code: error_codes::IPC_SOURCE_NOT_FOUND.to_string(),
                message: format!("no GitLab source registered for id {}", ctx.source_id),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitlab_mux_can_be_wrapped_as_arc_dyn_source_connector() {
        // Sanity: the orchestrator stores `Arc<dyn SourceConnector>`
        // in its registry, so the mux must be object-safe through that
        // bound. A compile-time check that regressions here will fail
        // loudly.
        let mux = GitlabMux::new(FixedOffset::east_opt(0).unwrap(), std::iter::empty());
        let _as_dyn: std::sync::Arc<dyn SourceConnector> = std::sync::Arc::new(mux);
    }

    #[tokio::test]
    async fn gitlab_mux_upsert_and_remove_round_trip() {
        let mux = GitlabMux::new(FixedOffset::east_opt(0).unwrap(), std::iter::empty());
        let sid = uuid::Uuid::new_v4();
        mux.upsert(GitlabSourceCfg {
            source_id: sid,
            base_url: "https://gitlab.example".into(),
            user_id: 17,
        })
        .await;
        assert!(mux.inner.read().await.contains_key(&sid));
        mux.remove(sid).await;
        assert!(!mux.inner.read().await.contains_key(&sid));
    }
}
