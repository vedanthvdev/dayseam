//! `connector-github` — Dayseam's fourth
//! [`connectors_sdk::SourceConnector`] implementation.
//!
//! This crate owns GitHub credential validation, `SourceKind::GitHub`
//! registration with the orchestrator, the per-source
//! [`GithubConnector`] handle, and the building blocks the DAY-96
//! walker (events endpoint + search-driven PR/issue fetch) will
//! assemble into a vec of [`dayseam_core::ActivityEvent`].
//!
//! GitHub's REST API is sufficiently different from Atlassian's
//! (`Link`-header pagination instead of cursor tokens, bearer PATs
//! instead of Basic-auth email+token, no per-product sub-client)
//! that the shared helpers live in this crate rather than in
//! [`connector_atlassian_common`]. If a second GitHub-shaped
//! connector ever shows up (Gitea, Bitbucket Cloud's REST v2) those
//! helpers graduate into an `connector-gh-common` crate in the same
//! way Atlassian's did; until then a dedicated crate is the
//! lighter-weight shape.
//!
//! ## Modules
//!
//! * [`auth`] — [`auth::validate_auth`] +
//!   [`auth::list_identities`]. The two entry points the IPC layer
//!   (DAY-99) calls when a GitHub source is added. `validate_auth`
//!   probes `GET /user`; `list_identities` turns the echoed
//!   `GithubUserInfo` into the [`dayseam_core::SourceIdentity`] row
//!   the activity walker filters by (keyed off the numeric user
//!   id — stable across renames, per the GitHub API reference).
//! * [`config`] — [`GithubConfig`]. Per-source configuration carried
//!   on the [`dayseam_core::SourceConfig::GitHub`] row: just the API
//!   base URL (so Enterprise Server tenants can point at their own
//!   host). The PAT itself lives in the keychain via the source's
//!   `secret_ref`.
//! * [`connector`] — the [`connectors_sdk::SourceConnector`]
//!   implementation, the [`GithubConnector`] per-source handle, and
//!   the [`GithubMux`] that dispatches by `ctx.source_id` the way
//!   [`connector_jira::JiraMux`] does for Jira. `sync` returns
//!   `Unsupported` for every [`connectors_sdk::SyncRequest`] variant
//!   until DAY-96 lands the walker.
//! * [`errors`] — [`errors::GithubUpstreamError`] +
//!   [`errors::map_status`] + [`errors::map_transport_error`].
//!   Classifies 4xx / 5xx / transport failures into the
//!   registry-defined `github.*` error codes the UI keys its
//!   Reconnect-card copy off.
//! * [`pagination`] — [`pagination::next_link`]. Parses the
//!   `Link` header GitHub returns on every paginated endpoint and
//!   hands the walker the next URL, tolerating malformed headers
//!   rather than aborting the sync.
//!
//! [`SourceConnector`]: connectors_sdk::SourceConnector

pub mod auth;
pub mod config;
pub mod connector;
pub mod errors;
pub mod events;
pub mod normalise;
pub mod pagination;
pub mod rollup;
pub mod walk;

pub use auth::{list_identities, validate_auth, GithubUserInfo};
pub use config::{GithubConfig, GITHUB_COM_API_BASE_URL};
pub use connector::{GithubConnector, GithubMux, GithubSourceCfg};
pub use errors::{
    map_status as map_github_status, map_transport_error as map_github_transport_error,
    GithubUpstreamError,
};
pub use events::{
    GithubActor, GithubComment, GithubEvent, GithubEventPayload, GithubIssue, GithubPullRequest,
    GithubRepo, GithubReview, GithubSearchIssue, GithubSearchPage, GithubUserRef,
};
pub use normalise::normalise_event;
pub use pagination::{next_link, parse_next_from_link_header};
pub use rollup::{collapse_rapid_reviews, RAPID_REVIEW_WINDOW_SECONDS};
pub use walk::{walk_day, WalkOutcome};
