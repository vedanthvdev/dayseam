//! `connector-local-git` — the first real [`connectors_sdk::SourceConnector`].
//!
//! The connector walks every discoverable git repository under the
//! configured scan roots and, for a requested day, emits one
//! [`dayseam_core::ActivityEvent`] per matching commit plus one
//! [`dayseam_core::Artifact`] (`CommitSet`) per `(repo, day)` bucket
//! that produced at least one event.
//!
//! Every module is intentionally small and single-purpose so the
//! invariants listed in `docs/plan/2026-04-18-v0.1-phase-2-local-git.md`
//! Task 2 can be verified one file at a time:
//!
//! * [`discovery`] — recursive, bounded, deterministic scan-root walk.
//! * [`walk`] — per-repo commit walker (day window + identity filter).
//! * [`privacy`] — the one-function redaction rule for private repos.
//! * [`connector`] — wires the modules into the `SourceConnector`
//!   trait, emits progress + log events, dispatches on `SyncRequest`.
//!
//! The `SourceConnector` impl is the only public surface a caller
//! needs; the helper modules are `pub` only so integration tests can
//! exercise them in isolation. Everything else is `pub(crate)`.

pub mod connector;
pub mod discovery;
pub mod privacy;
pub mod walk;

pub use connector::LocalGitConnector;
pub use discovery::{DiscoveredRepo, DiscoveryConfig, DiscoveryOutcome};
