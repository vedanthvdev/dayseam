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
use connector_confluence::{ConfluenceMux, ConfluenceSourceCfg};
use connector_gitlab::{GitlabMux, GitlabSourceCfg};
use connector_jira::{JiraMux, JiraSourceCfg};
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
    /// Configured Jira sources (one entry per `SourceConfig::Jira`
    /// row). The [`JiraMux`] registered for [`SourceKind::Jira`]
    /// dispatches per `source_id` to the right workspace + email at
    /// sync time. Empty in every deployment today (the DAY-76
    /// scaffold registers the kind but does not yet service a real
    /// walk); DAY-82 wires the Add-Source dialog to populate this.
    pub jira_sources: Vec<JiraSourceCfg>,
    /// Configured Confluence sources (one entry per
    /// `SourceConfig::Confluence` row). The [`ConfluenceMux`]
    /// registered for [`SourceKind::Confluence`] dispatches per
    /// `source_id` to the right workspace at sync time. Empty in
    /// every deployment today (the DAY-79 scaffold registers the
    /// kind but does not yet service a real walk); DAY-80 adds the
    /// CQL walker, and DAY-82 wires the Add-Source dialog to
    /// populate this.
    pub confluence_sources: Vec<ConfluenceSourceCfg>,
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
    // Always register the Jira kind, even on an install with zero
    // Jira sources, so the DAY-82 Add-Source flow can `upsert` into
    // a live mux without rebuilding the registry — mirroring the
    // GitLab path above.
    connectors.insert(
        SourceKind::Jira,
        Arc::new(JiraMux::new(cfg.local_tz, cfg.jira_sources)),
    );
    // DAY-79 / DAY-80: same "register-empty, upsert-later" contract
    // for the Confluence kind. DAY-80 wired `SyncRequest::Day` onto
    // the CQL walker; `local_tz` threads through the mux exactly the
    // way it does for GitLab and Jira so the walker can derive the
    // correct UTC window from a local day.
    connectors.insert(
        SourceKind::Confluence,
        Arc::new(ConfluenceMux::new(cfg.local_tz, cfg.confluence_sources)),
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
            jira_sources: Vec::new(),
            confluence_sources: Vec::new(),
        };
        let (connectors, sinks) = default_registries(cfg);
        // DAY-83 Task 11.1 — hydration smoke: the shipping connector
        // set is **exactly** {LocalGit, GitLab, Jira, Confluence}.
        // Asserting the full kind set (rather than individual
        // `.get(kind).is_some()` probes) catches both directions of
        // regression: a kind that silently drops out (→ orchestrator
        // fan-out skips it), and a spurious extra kind that gets
        // wired in without a matching `DefaultRegistryConfig` field
        // (→ a connector mux running with a default config that
        // ignores the user's sources). Using `HashSet` keeps the
        // assertion insensitive to the HashMap iteration order the
        // `.kinds()` method returns.
        use std::collections::HashSet;
        let connector_kinds: HashSet<SourceKind> = connectors.kinds().into_iter().collect();
        assert_eq!(
            connector_kinds,
            HashSet::from([
                SourceKind::LocalGit,
                SourceKind::GitLab,
                SourceKind::Jira,
                SourceKind::Confluence,
            ]),
            "default_registries must hydrate exactly the four shipping connector kinds",
        );
        let sink_kinds: HashSet<SinkKind> = sinks.kinds().into_iter().collect();
        assert_eq!(
            sink_kinds,
            HashSet::from([SinkKind::MarkdownFile]),
            "default_registries must hydrate exactly the shipping sink kinds",
        );
        assert!(connectors.get(SourceKind::LocalGit).is_some());
        // The GitLab connector is always registered as a mux — even
        // a brand-new user with zero GitLab sources gets a live
        // handle so Task 3's add-source flow can `upsert` into it
        // without re-registering.
        assert!(connectors.get(SourceKind::GitLab).is_some());
        // Same contract for Jira: the DAY-76 scaffold registers the
        // kind with an empty mux so the DAY-82 Add-Source flow can
        // slot a fresh source in without re-registering. Also
        // double-check the registered handle self-reports the right
        // kind — a wrong kind there would silently route Jira
        // fan-out to whatever mux we accidentally registered,
        // mirroring the Phase 3 GitLab invariant check.
        let jira = connectors
            .get(SourceKind::Jira)
            .expect("Jira kind registered");
        assert_eq!(jira.kind(), SourceKind::Jira);
        // DAY-79: parallel invariant for the Confluence mux. The
        // scaffold registers the kind with an empty mux so the
        // DAY-82 Add-Source flow can slot in a fresh Confluence
        // source without re-registering; double-checking the
        // registered handle self-reports the right kind guards
        // against a copy-paste regression that would silently
        // route Confluence fan-out to the Jira mux (both happen to
        // be typed `Arc<dyn SourceConnector>` so the compiler can't
        // catch that mix-up on its own).
        let confluence = connectors
            .get(SourceKind::Confluence)
            .expect("Confluence kind registered");
        assert_eq!(confluence.kind(), SourceKind::Confluence);
        assert!(sinks.get(SinkKind::MarkdownFile).is_some());
    }

    /// DAY-90 TST-v0.2-03. The `SourceKind` → connector-implementation
    /// mapping has to hold two invariants simultaneously and neither is
    /// guarded by the type system on its own:
    ///
    /// 1. **Coverage.** Every `SourceKind` variant that ships in the
    ///    binary must have an entry in `default_registries`. A new
    ///    variant added without a matching registration compiles fine
    ///    — the orchestrator just silently skips fan-out for that kind
    ///    at runtime (the DOG-v0.2-04-class silent-failure we spent
    ///    DAY-88 sweeping).
    /// 2. **Self-consistency.** The connector registered under key `K`
    ///    must report `K` from its own `.kind()` method. A copy-paste
    ///    bug that wires a `JiraMux` under `SourceKind::Confluence`
    ///    type-checks cleanly (both are `Arc<dyn SourceConnector>`)
    ///    and fails only in production the first time a Confluence
    ///    source runs.
    ///
    /// The exhaustiveness match below turns invariant (1) from a
    /// runtime test into a compile-time failure: adding `SourceKind::
    /// Bitbucket` without extending `ALL_KINDS` *and* adding a
    /// `default_registries` arm for it fails to compile here. The
    /// per-kind `assert_eq!(conn.kind(), kind)` loop below pins
    /// invariant (2).
    #[test]
    fn registry_kind_round_trips_for_every_registered_connector() {
        const ALL_KINDS: &[SourceKind] = &[
            SourceKind::LocalGit,
            SourceKind::GitLab,
            SourceKind::Jira,
            SourceKind::Confluence,
        ];
        // Compiler-checked exhaustiveness: if `SourceKind` grows a
        // variant, this match fails to compile and forces the author
        // to extend `ALL_KINDS` above — which in turn fails the
        // coverage assertion below until `default_registries` is
        // extended too.
        fn _exhaustive_source_kind_check(k: SourceKind) {
            match k {
                SourceKind::LocalGit
                | SourceKind::GitLab
                | SourceKind::Jira
                | SourceKind::Confluence => {}
            }
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = DefaultRegistryConfig {
            local_git_scan_roots: vec![tmp.path().to_path_buf()],
            local_git_private_roots: vec![],
            local_tz: FixedOffset::east_opt(0).expect("UTC offset"),
            markdown_dest_dirs: vec![tmp.path().to_path_buf()],
            gitlab_sources: Vec::new(),
            jira_sources: Vec::new(),
            confluence_sources: Vec::new(),
        };
        let (connectors, _) = default_registries(cfg);

        for &kind in ALL_KINDS {
            let connector = connectors.get(kind).unwrap_or_else(|| {
                panic!(
                    "default_registries must register a connector for every \
                     SourceKind variant; missing {kind:?}",
                )
            });
            assert_eq!(
                connector.kind(),
                kind,
                "connector registered under {kind:?} self-reports as \
                 {:?}; a copy-paste bug has wired the wrong mux under this \
                 key and Dayseam will fan out to the wrong connector at \
                 runtime",
                connector.kind(),
            );
        }

        // Also assert the registry's own inventory exactly matches
        // ALL_KINDS — guards against a spurious extra registration
        // (→ a kind firing without a `DefaultRegistryConfig` field to
        // plumb its sources in).
        use std::collections::HashSet;
        let registered: HashSet<SourceKind> = connectors.kinds().into_iter().collect();
        let expected: HashSet<SourceKind> = ALL_KINDS.iter().copied().collect();
        assert_eq!(
            registered, expected,
            "default_registries' live inventory must equal the ALL_KINDS \
             exhaustiveness list exactly — mismatch means a variant was \
             added in one place but not the other",
        );
    }
}
