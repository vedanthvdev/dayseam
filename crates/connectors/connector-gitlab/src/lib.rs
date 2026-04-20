//! `connector-gitlab` — Dayseam's second [`connectors_sdk::SourceConnector`]
//! implementation.
//!
//! For a requested local-timezone day, the connector:
//!
//! 1. Calls `GET /api/v4/users/:user_id/events?after=&before=` against
//!    the configured GitLab host (self-hosted or `gitlab.com`),
//!    paginating until the last event's `created_at` slips out of the
//!    day-window buffer.
//! 2. Normalises each response row into a
//!    [`dayseam_core::ActivityEvent`] — one per merge-request state
//!    change, issue state change, comment, approval, and, for push
//!    events, one `CommitAuthored` event per pushed commit SHA (capped
//!    per push to keep a 200-commit push from producing 200 bullets;
//!    see [`walk`] for the cap).
//! 3. Filters events whose `author.id` is not in the per-source
//!    [`dayseam_core::SourceIdentity`] list — the v0.1 identity match
//!    is *numeric user-id only*, never username, so a user whose
//!    handle rotates but whose id stays stable is still attributed
//!    correctly.
//! 4. Emits progress / log events the same shape local-git does, plus a
//!    GitLab-specific `gitlab.rate_limited` progress line whenever the
//!    server responds 429 so the user sees why a sync is slow.
//!
//! Every module is intentionally small so the invariants listed in
//! `docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md` Task 1 can be
//! verified file-by-file:
//!
//! * [`auth`] — `validate_pat` helper used by the IPC add-source flow.
//!   The [`connectors_sdk::PatAuth`] attached to `ConnCtx::auth` is
//!   what the connector reaches for during `sync`.
//! * [`events`] — serde-typed wrappers around the Events API response
//!   shape. Forward-compatible via `serde(other)` so an unknown
//!   target type degrades to a typed [`GitlabUpstreamError::ShapeChanged`]
//!   rather than a panic.
//! * [`walk`] — the day-window walker. Owns pagination, 429 backoff,
//!   and per-push commit enrichment.
//! * [`normalise`] — `GitlabEvent` → `ActivityEvent`. One match arm per
//!   [`dayseam_core::ActivityKind`] variant GitLab produces.
//! * [`errors`] — the seven `gitlab.*` registry codes surfaced as
//!   typed constructors on [`DayseamError`].
//! * [`connector`] — wires the modules into the `SourceConnector`
//!   trait, dispatches on `SyncRequest`, emits progress and log
//!   streams.

pub mod auth;
pub mod connector;
pub mod errors;
pub mod events;
pub mod normalise;
pub mod walk;

pub use auth::{validate_pat, GitlabUserInfo};
pub use connector::{GitlabConnector, GitlabMux, GitlabSourceCfg};
pub use errors::GitlabUpstreamError;
