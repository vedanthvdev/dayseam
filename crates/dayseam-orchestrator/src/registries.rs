//! Connector and sink registries.
//!
//! Each [`Orchestrator`](crate::Orchestrator) holds exactly one
//! [`ConnectorRegistry`] and one [`SinkRegistry`]. The registries map
//! [`SourceKind`] / [`SinkKind`] to a single trait-object handle that
//! lives for the lifetime of the process, so every run for a given
//! kind goes through the same implementation.
//!
//! The production default registers:
//! * [`LocalGitConnector`] against [`SourceKind::LocalGit`]
//! * [`MarkdownFileSink`] against [`SinkKind::MarkdownFile`]
//!
//! Tests construct registries by hand to substitute
//! [`connectors_sdk::MockConnector`] and/or [`sinks_sdk::MockSink`]
//! without needing a real filesystem or a real git repo.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::FixedOffset;
use connector_gitlab::{GitlabMux, GitlabSourceCfg};
use connector_local_git::LocalGitConnector;
use connectors_sdk::SourceConnector;
use dayseam_core::{SinkKind, SourceKind};
use sink_markdown_file::MarkdownFileSink;
use sinks_sdk::SinkAdapter;

/// Registry of [`SourceConnector`] implementations keyed by
/// [`SourceKind`]. Cheap to clone — all lookups go through the inner
/// [`HashMap`] and every value is an [`Arc`].
#[derive(Debug, Clone, Default)]
pub struct ConnectorRegistry {
    connectors: HashMap<SourceKind, Arc<dyn SourceConnector>>,
}

impl ConnectorRegistry {
    /// Fresh, empty registry. Tests use this directly and then
    /// [`Self::insert`] the mock connectors they need.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the connector registered for `kind`.
    /// Returning the previous handle (rather than silently clobbering
    /// it) means a test that double-registers by accident fails loudly
    /// at the call site.
    pub fn insert(
        &mut self,
        kind: SourceKind,
        connector: Arc<dyn SourceConnector>,
    ) -> Option<Arc<dyn SourceConnector>> {
        self.connectors.insert(kind, connector)
    }

    /// Look up the connector for `kind`. Returns `None` if no
    /// connector has been registered; the orchestrator translates that
    /// into a typed error before returning to the caller.
    #[must_use]
    pub fn get(&self, kind: SourceKind) -> Option<Arc<dyn SourceConnector>> {
        self.connectors.get(&kind).cloned()
    }

    /// Every kind currently registered. Exposed for diagnostics only.
    #[must_use]
    pub fn kinds(&self) -> Vec<SourceKind> {
        self.connectors.keys().copied().collect()
    }
}

/// Registry of [`SinkAdapter`] implementations keyed by [`SinkKind`].
/// Same shape as [`ConnectorRegistry`] — one `Arc<dyn …>` per kind —
/// deliberately kept as a parallel type so a registry-level bug in one
/// direction cannot silently corrupt the other.
///
/// `SinkAdapter` does not require `Debug` (unlike `SourceConnector`)
/// so we hand-roll the `Debug` impl instead of deriving it; the only
/// diagnostics information that matters is the set of kinds.
#[derive(Clone, Default)]
pub struct SinkRegistry {
    sinks: HashMap<SinkKind, Arc<dyn SinkAdapter>>,
}

impl std::fmt::Debug for SinkRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kinds: Vec<SinkKind> = self.sinks.keys().copied().collect();
        f.debug_struct("SinkRegistry")
            .field("kinds", &kinds)
            .finish()
    }
}

impl SinkRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        kind: SinkKind,
        sink: Arc<dyn SinkAdapter>,
    ) -> Option<Arc<dyn SinkAdapter>> {
        self.sinks.insert(kind, sink)
    }

    #[must_use]
    pub fn get(&self, kind: SinkKind) -> Option<Arc<dyn SinkAdapter>> {
        self.sinks.get(&kind).cloned()
    }

    #[must_use]
    pub fn kinds(&self) -> Vec<SinkKind> {
        self.sinks.keys().copied().collect()
    }
}

/// Production defaults: one shipping connector, one shipping sink.
///
/// Built as a free function (rather than [`Default`] on each registry)
/// because the defaults need arguments the registry itself does not
/// want to own (scan roots, private-repo list, local timezone, sink
/// destination directories). The Tauri layer calls this once during
/// `setup`.
#[derive(Debug, Clone)]
pub struct DefaultRegistryConfig {
    /// Scan roots the local-git connector walks. Comes from
    /// `sources.config.LocalGit.scan_roots` at startup.
    pub local_git_scan_roots: Vec<PathBuf>,
    /// Repos the user has explicitly marked private. Events from these
    /// repos keep their actor / timestamp but the body / title / raw
    /// payload are redacted. See
    /// [`connector_local_git::privacy`](connector_local_git::privacy).
    pub local_git_private_roots: Vec<PathBuf>,
    /// User's local timezone. Used by the connector to pin
    /// `SyncRequest::Day(NaiveDate)` to a specific UTC window.
    pub local_tz: FixedOffset,
    /// Destination directories the markdown-file sink writes to. Every
    /// `save_report` run writes identical bytes to every directory in
    /// this list; the sink exposes per-destination receipts on partial
    /// failure.
    pub markdown_dest_dirs: Vec<PathBuf>,
    /// Configured GitLab sources (one entry per `SourceConfig::GitLab`
    /// row). The [`GitlabMux`] registered for
    /// [`SourceKind::GitLab`] dispatches per `source_id` to the right
    /// `(base_url, user_id)` at sync time. Empty in the local-git-only
    /// deployment; the Task 3 add-source flow populates it.
    pub gitlab_sources: Vec<GitlabSourceCfg>,
}

/// Build the pair of registries used in production. Tests that need
/// mock connectors / sinks should construct empty registries and
/// populate them manually.
#[must_use]
pub fn default_registries(cfg: DefaultRegistryConfig) -> (ConnectorRegistry, SinkRegistry) {
    let mut connectors = ConnectorRegistry::new();
    connectors.insert(
        SourceKind::LocalGit,
        Arc::new(LocalGitConnector::new(
            cfg.local_git_scan_roots,
            cfg.local_git_private_roots.into_iter().collect(),
            cfg.local_tz,
        )),
    );
    connectors.insert(
        SourceKind::GitLab,
        Arc::new(GitlabMux::new(cfg.local_tz, cfg.gitlab_sources)),
    );

    let mut sinks = SinkRegistry::new();
    sinks.insert(
        SinkKind::MarkdownFile,
        Arc::new(MarkdownFileSink::new(&cfg.markdown_dest_dirs)),
    );

    (connectors, sinks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use connectors_sdk::MockConnector;

    #[test]
    fn connector_registry_round_trips_insert_and_get() {
        let mut reg = ConnectorRegistry::new();
        let conn = Arc::new(MockConnector::new(SourceKind::GitLab, vec![]));
        assert!(reg.insert(SourceKind::GitLab, conn.clone()).is_none());
        assert!(reg.get(SourceKind::GitLab).is_some());
        assert_eq!(reg.kinds(), vec![SourceKind::GitLab]);
        // Replacing returns the previous handle so a duplicate
        // registration during test setup can assert it.
        assert!(reg.insert(SourceKind::GitLab, conn).is_some());
    }

    #[test]
    fn sink_registry_get_on_empty_returns_none() {
        let reg = SinkRegistry::new();
        assert!(reg.get(SinkKind::MarkdownFile).is_none());
        assert!(reg.kinds().is_empty());
    }

    #[test]
    fn default_registries_populate_shipping_kinds() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = DefaultRegistryConfig {
            local_git_scan_roots: vec![tmp.path().to_path_buf()],
            local_git_private_roots: vec![],
            local_tz: FixedOffset::east_opt(0).expect("UTC offset"),
            markdown_dest_dirs: vec![tmp.path().to_path_buf()],
            gitlab_sources: Vec::new(),
        };
        let (connectors, sinks) = default_registries(cfg);
        assert!(connectors.get(SourceKind::LocalGit).is_some());
        // The GitLab connector is always registered as a mux — even
        // a brand-new user with zero GitLab sources gets a live
        // handle so Task 3's add-source flow can `upsert` into it
        // without re-registering.
        assert!(connectors.get(SourceKind::GitLab).is_some());
        assert!(sinks.get(SinkKind::MarkdownFile).is_some());
    }
}
