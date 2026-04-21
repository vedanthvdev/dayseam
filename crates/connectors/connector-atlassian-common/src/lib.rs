//! Shared primitives for `connector-jira` and `connector-confluence`.
//!
//! This crate is the "once-and-only-once" layer the v0.2 plan's
//! [Task 3](`docs/plan/2026-04-20-v0.2-atlassian.md`) extracted from
//! the spike's finding that Jira and Confluence share one accountId,
//! one email + API-token credential, one tenant hostname, and one
//! rich-text format (ADF). Keeping the five shared concerns here —
//!
//! 1. [`adf::adf_to_plain`] — ADF → plain-text walker;
//! 2. [`cloud::discover_cloud`] — `GET /rest/api/3/myself` probe
//!    returning [`cloud::AtlassianAccountInfo`];
//! 3. [`identity::seed_atlassian_identity`] — builds the
//!    [`dayseam_core::SourceIdentity`] row the walker's self-filter
//!    needs before first sync;
//! 4. [`pagination::JqlTokenPaginator`] +
//!    [`pagination::V2CursorPaginator`] — the two cursor shapes
//!    Atlassian exposes;
//! 5. [`errors`] — the nine-code [`errors::AtlassianError`] taxonomy
//!    DAY-73 reserved, including the `atlassian.auth.*` classifier
//!    DAY-74 deferred here per the Phase-3 CORR-01 invariant.
//!
//! — prevents cross-connector drift of the CONS-addendum-04 class
//! (Phase 3 §2.4) and means the Jira walker and the Confluence
//! walker agree on how a 401 is surfaced to the UI, how an `@mention`
//! is rendered in a bullet, and how an `accountId` is validated.
//!
//! ## Layering
//!
//! The crate depends on `connectors-sdk`, `dayseam-core`, and
//! `dayseam-events`. It deliberately does **not** depend on
//! `dayseam-db`: DB writes live in the IPC layer (per the
//! `ensure_gitlab_self_identity` precedent), and the
//! [`identity::seed_atlassian_identity`] helper returns a
//! `SourceIdentity` value for the caller to persist, not a side
//! effect. This keeps the connector crates testable without a SQLite
//! harness.

pub mod adf;
pub mod cloud;
pub mod errors;
pub mod identity;
pub mod pagination;

pub use adf::{adf_to_plain, UNSUPPORTED_MARKER};
pub use cloud::{discover_cloud, AtlassianAccountInfo, AtlassianCloud};
pub use errors::{map_status, validate_account_id, AtlassianError, Product, MAX_ACCOUNT_ID_LEN};
pub use identity::seed_atlassian_identity;
pub use pagination::{CursorPaginator, JqlTokenPaginator, Page, V2CursorPaginator};
