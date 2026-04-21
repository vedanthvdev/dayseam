//! `connector-confluence` — Dayseam's fourth
//! [`connectors_sdk::SourceConnector`] implementation.
//!
//! This crate owns Confluence Cloud credential validation,
//! `SourceKind::Confluence` registration with the orchestrator, and
//! the per-source [`connector::ConfluenceConnector`] handle. The CQL
//! walker that turns one Confluence workspace + one day into a vec of
//! [`dayseam_core::ActivityEvent`] lands in DAY-80; this scaffold
//! registers the kind + ships auth so the Add-Source dialog (DAY-82)
//! can wire a user onto a Confluence source without blocking on the
//! walker.
//!
//! ## Shape mirrors `connector-jira`
//!
//! Jira and Confluence share one Atlassian Cloud credential, one
//! hostname, and one identity row (the `AtlassianAccountId`); the
//! shared plumbing — Basic-auth header shape, `/myself` probe,
//! identity seed, error-code taxonomy — lives one layer down in
//! [`connector_atlassian_common`]. This crate is a thin Confluence
//! facade on top of that common, and is intentionally parallel to
//! [`connector_jira`] so a reviewer reading one knows where to find
//! the sibling in the other.
//!
//! ## Modules
//!
//! * [`auth`] — `validate_auth` + `list_identities`, both wrappers
//!   around [`connector_atlassian_common::discover_cloud`] /
//!   [`connector_atlassian_common::seed_atlassian_identity`]. The two
//!   entry points the IPC layer (DAY-82 Add-Source dialog) calls
//!   when a Confluence source is added.
//! * [`config`] — [`ConfluenceConfig`]. Per-source configuration
//!   carried on the [`dayseam_core::SourceConfig::Confluence`] row.
//!   Holds only the workspace URL — the email lives on the paired
//!   [`dayseam_core::SourceConfig::Jira`] row or on a dedicated
//!   Confluence credential (planned follow-up); it is not duplicated
//!   here so the "one keychain entry serves both products" flow has
//!   a single source of truth.
//! * [`connector`] — the [`SourceConnector`] implementation plus the
//!   [`connector::ConfluenceMux`] that dispatches
//!   [`SourceConnector::sync`] by `ctx.source_id` to the right
//!   [`connector::ConfluenceConnector`] instance, mirroring
//!   [`connector_jira::JiraMux`]. Wired in DAY-80 to route
//!   [`connectors_sdk::SyncRequest::Day`] into [`walk::walk_day`];
//!   `Range` / `Since` remain [`dayseam_core::DayseamError::Unsupported`]
//!   until v0.3's incremental scheduler, matching the Jira shape.
//! * [`walk`] — the per-day CQL walker. Runs
//!   `GET /wiki/rest/api/search?cql=contributor%20%3D%20currentUser()%20…`,
//!   paginates via the shared `_links.next` helper, and hands each row
//!   to [`normalise::normalise_result`].
//! * [`normalise`] — one CQL result → at most one [`ActivityEvent`].
//!   Arms per `content.type` (`"page"` / `"comment"`) matching the
//!   spike §8 taxonomy.
//! * [`rollup`] — rapid-save collapse for `ConfluencePageEdited`
//!   events. Pure-function parallel to
//!   [`connector_jira::rollup::collapse_rapid_transitions`].
//!
//! [`SourceConnector`]: connectors_sdk::SourceConnector
//! [`ActivityEvent`]: dayseam_core::ActivityEvent

pub mod auth;
pub mod config;
pub mod connector;
pub mod normalise;
pub mod rollup;
pub mod walk;

pub use auth::{list_identities, validate_auth};
pub use config::ConfluenceConfig;
pub use connector::{ConfluenceConnector, ConfluenceMux, ConfluenceSourceCfg};
