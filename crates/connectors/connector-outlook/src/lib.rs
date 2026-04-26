//! `connector-outlook` — Dayseam's fifth
//! [`connectors_sdk::SourceConnector`] implementation, landing in
//! v0.9 as the first OAuth-backed source.
//!
//! This crate owns Outlook (Microsoft Graph) credential validation,
//! `SourceKind::Outlook` registration with the orchestrator, the
//! per-source [`OutlookConnector`] handle, and the calendar-day
//! walker that turns a local date into a vec of
//! [`dayseam_core::ActivityEvent`] of kind
//! [`dayseam_core::ActivityKind::OutlookMeetingAttended`].
//!
//! Microsoft Graph is sufficiently different from the other
//! connectors' REST APIs — OAuth 2.0 access-token rotation instead of
//! long-lived PATs, `@odata.nextLink` pagination instead of
//! `Link`-header pagination, a `Prefer: outlook.timezone="UTC"`
//! header that materially changes the server's timestamp emission —
//! that the shared helpers live in this crate rather than in
//! [`connector_atlassian_common`].
//!
//! ## Modules
//!
//! * [`auth`] — [`auth::validate_auth`] +
//!   [`auth::list_identities`]. The two entry points the IPC layer
//!   (DAY-203's `outlook_validate_credentials`) calls when an
//!   Outlook source is added. `validate_auth` probes `GET /me`;
//!   `list_identities` turns the echoed [`auth::OutlookUserInfo`]
//!   into the two [`dayseam_core::SourceIdentity`] rows the walker
//!   filters by — the Graph object id (stable forever) and the UPN
//!   (stable across renames within a tenant; used as the actor
//!   email in rendered evidence rows).
//! * [`config`] — [`OutlookConfig`]. Per-source configuration
//!   carried on the [`dayseam_core::SourceConfig::Outlook`] row:
//!   tenant id, UPN, and the Graph API base URL (fixed at
//!   [`config::GRAPH_API_BASE_URL`] but exposed as a field so
//!   sovereign-cloud tenants — `graph.microsoft.us`,
//!   `microsoftgraph.chinacloudapi.cn` — can override it in a later
//!   ticket without a schema change).
//! * [`connector`] — the [`connectors_sdk::SourceConnector`]
//!   implementation, the [`OutlookConnector`] per-source handle, and
//!   the [`OutlookMux`] that dispatches by `ctx.source_id` the way
//!   [`connector_github::GithubMux`] does. `sync` routes
//!   [`connectors_sdk::SyncRequest::Day`] through
//!   [`walk::walk_day`]; everything else returns `Unsupported`
//!   until v0.10's incremental scheduler lands.
//! * [`errors`] — [`errors::OutlookUpstreamError`] +
//!   [`errors::map_status`]. Classifies 4xx / 5xx into the
//!   registry-defined `outlook.*` error codes the UI keys its
//!   Reconnect-card copy off. Transport failures come back from the
//!   SDK's [`connectors_sdk::HttpClient::send`] as `http.transport.*`
//!   sub-codes rather than through a per-connector mapper that
//!   string-matches on the request URL.
//! * [`events`] — wire shapes for the trimmed Graph calendar event
//!   object. Keeps only the fields the walker / normaliser use.
//! * [`normalise`] — `GraphEvent` → `ActivityEvent` mapping.
//!   Single kind output; private-sensitivity redaction + UTC
//!   timestamp parsing live here.
//! * [`walk`] — per-day `calendarView` walker. Filters cancelled,
//!   all-day, free-show-as, declined, and unattended rows before
//!   normalising.
//!
//! [`SourceConnector`]: connectors_sdk::SourceConnector

pub mod auth;
pub mod config;
pub mod connector;
pub mod errors;
pub mod events;
pub mod normalise;
pub mod walk;

pub use auth::{list_identities, validate_auth, OutlookUserInfo};
pub use config::{OutlookConfig, GRAPH_API_BASE_URL};
pub use connector::{OutlookConnector, OutlookMux, OutlookSourceCfg};
pub use errors::{map_status as map_outlook_status, OutlookUpstreamError};
pub use events::{
    GraphAttendee, GraphAttendeeRef, GraphAttendeeStatus, GraphDateTime, GraphEmailAddress,
    GraphEvent, GraphEventsPage,
};
pub use normalise::{normalise_event, NormaliseError};
pub use walk::{walk_day, WalkOutcome};
