//! `connector-jira` — Dayseam's third
//! [`connectors_sdk::SourceConnector`] implementation (v0.2 scaffold).
//!
//! This crate is the *thin shell* the add-source dialog talks to when
//! a user registers a Jira Cloud workspace: it owns credential
//! validation, `SourceKind::Jira` registration with the orchestrator,
//! and a per-source `JiraConnector` handle that the JQL walker
//! (DAY-77) will slot into at `sync`-time. The heavy lifting — ADF
//! parsing, cursor pagination, cloud-identity discovery, and the
//! error taxonomy — all lives one layer down in
//! [`connector_atlassian_common`] and is shared with the Confluence
//! connector that lands in DAY-79.
//!
//! ## Today vs. DAY-77
//!
//! `JiraConnector::sync` returns
//! [`dayseam_core::DayseamError::Unsupported`] for **every**
//! [`connectors_sdk::SyncRequest`] variant. That is deliberate: the
//! scaffold PR's job is to register `SourceKind::Jira` end-to-end, let
//! the Add-Source dialog probe credentials, and surface
//! `SourceIdentity` rows at the IPC boundary — *without* also having
//! to review a ~1.5-day JQL walker in the same diff. DAY-77 flips the
//! `SyncRequest::Day` arm from `Unsupported` to a live walk.
//!
//! ## Modules
//!
//! * [`auth`] — `validate_auth` + `list_identities`, both thin
//!   wrappers around [`connector_atlassian_common::discover_cloud`] /
//!   [`connector_atlassian_common::seed_atlassian_identity`]. These
//!   are the two entry points the IPC layer (DAY-82) calls when a
//!   Jira source is added.
//! * [`config`] — [`JiraConfig`]. Per-source configuration carried on
//!   the [`dayseam_core::SourceConfig::Jira`] row: just the workspace
//!   URL and the account email. The API token lives in the keychain
//!   via the source's `secret_ref`; the email sits next to
//!   `workspace_url` so the shared-vs-separate PAT flows (DAY-81) can
//!   address two sources as independent auth contexts even when they
//!   share a single keychain entry.
//! * [`connector`] — the [`SourceConnector`] implementation, the
//!   `JiraConnector` per-source handle, and the `JiraMux` that
//!   dispatches by `ctx.source_id` the way [`connector_gitlab::GitlabMux`]
//!   does for GitLab.
//!
//! [`SourceConnector`]: connectors_sdk::SourceConnector

pub mod auth;
pub mod config;
pub mod connector;

pub use auth::{list_identities, validate_auth};
pub use config::JiraConfig;
pub use connector::{JiraConnector, JiraMux, JiraSourceCfg};
