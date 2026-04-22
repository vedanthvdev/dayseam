//! Connector DTO conventions — persisted vs wire-format types.
//!
//! This module is **doc-only**: it declares no types. It exists so
//! `cargo doc` surfaces the convention in the same navigation tree as
//! [`AuthStrategy`](crate::AuthStrategy), [`HttpClient`](crate::HttpClient),
//! and the rest of the SDK, where a connector author new to the crate
//! is most likely to look first.
//!
//! ## Two flavours of type
//!
//! A source connector handles two very different flavours of
//! `serde`-derived type. The distinction matters for forward
//! compatibility, and conflating them has caused real bugs in this
//! codebase — notably the v0.3 capstone's
//! [CONS-v0.3-01](https://github.com/vedanthvdev/dayseam/blob/master/docs/review/v0.3-review.md)
//! finding, which flagged inconsistent `#[serde(default)]` discipline
//! across connectors and prompted the
//! [`SerdeDefaultAudit`](dayseam_macros::SerdeDefaultAudit) derive.
//!
//! ### Persisted state
//!
//! Any `serde`-derived type whose bytes end up inside the SQLite
//! `state.db` (the `sources.config_json` blob, the
//! `activity_events.entities` JSON column, the `artifacts.payload_json`
//! column, `sync_runs.per_source_state`, or an IPC round-trip through
//! `@dayseam/ipc-types`). These types have two independent evolution
//! axes:
//!
//! 1. A **Dayseam binary upgrade** — a user running v0.3.0 upgrades to
//!    v0.4.0 with a `state.db` written by the older binary. The newer
//!    binary has to deserialise rows the older binary wrote.
//! 2. A **Dayseam binary downgrade** (rare but real — a user rolls a
//!    bad `v0.4.x` release back to `v0.3.y`). The older binary has to
//!    deserialise rows the newer binary wrote.
//!
//! Because the evolution axes are internal to Dayseam, every
//! `#[serde(default)]` field on a persisted type is a **mutation the
//! Dayseam author owns** — which means the author also owns the
//! repair story for a row that was written by an older binary that
//! did not emit the field. That is exactly what
//! [`SerdeDefaultAudit`](dayseam_macros::SerdeDefaultAudit) enforces
//! at compile time: every `#[serde(default)]` must carry either a
//! paired `#[serde_default_audit(repair = "…")]` annotation (the
//! repair function to call for older rows) or a
//! `#[serde_default_audit(no_repair = "…")]` waiver with a rationale
//! string.
//!
//! **Rule of thumb for persisted types:** apply
//! `#[derive(dayseam_macros::SerdeDefaultAudit)]` alongside the serde
//! derives. The derive is a no-op on types that have no
//! `#[serde(default)]` fields today, and it locks in the discipline
//! for the day someone adds one. See
//! [`accepts_enum_variant_fields.rs`](https://github.com/vedanthvdev/dayseam/blob/master/crates/dayseam-macros/tests/trybuild/pass/accepts_enum_variant_fields.rs)
//! for the `SourceConfig`-style shape every connector config follows.
//!
//! ### HTTP DTOs
//!
//! Any `serde`-derived type whose bytes only ever cross the wire
//! between a connector and its upstream API (GitLab `GET /events`
//! response bodies, Jira `/search` JQL results, Confluence REST
//! payloads, GitHub `GET /user/events` bodies). These types have a
//! **different** evolution axis: the upstream API evolves
//! independently of the Dayseam binary. Fields appear that the
//! connector doesn't care about; optional fields disappear; an enum
//! variant the connector doesn't handle lands in a new API version.
//!
//! The defensive posture here is **liberal-in, conservative-out**:
//!
//! - Use `#[serde(default)]` freely on optional fields the upstream
//!   API may or may not emit. The connector's job is to tolerate
//!   missing fields, not to round-trip them byte-stable.
//! - Use `#[serde(deny_unknown_fields = false)]` (the default) — an
//!   unexpected new field is not an error.
//! - Use `#[serde(other)]` on catch-all enum variants so an unknown
//!   upstream event type surfaces as `EventKind::Unknown(String)`
//!   rather than failing the deserialiser.
//!
//! **Rule of thumb for HTTP DTOs:** **do not** apply `SerdeDefaultAudit`.
//! The audit's whole purpose is to force the author of a persisted
//! type to own the field-evolution story; for an HTTP DTO the
//! upstream API owns that story and the audit would flag every
//! reasonable default as a compile-time bug. The SDK's
//! [`AuthStrategy`](crate::AuthStrategy) and
//! [`HttpClient`](crate::HttpClient) types deliberately carry no
//! `serde` derives at all — they are in-memory shapes, not
//! persisted ones.
//!
//! ## Where each connector's DTOs live
//!
//! - GitLab: `crates/connectors/connector-gitlab/src/dtos.rs`.
//! - Jira / Confluence: `crates/connectors/connector-atlassian-common/src/dtos/`.
//! - GitHub (v0.4): `crates/connectors/connector-github/src/dtos.rs` —
//!   lands in DAY-95.
//!
//! ## Frequently asked edge cases
//!
//! - **A type round-trips through both.** The normalised
//!   [`dayseam_core::ActivityEvent`] is the canonical example: it is
//!   persisted in `activity_events` *and* it flows out through the
//!   IPC layer. Treat it as persisted — apply `SerdeDefaultAudit`.
//!   If the UI layer has its own view-model on top, that view-model
//!   lives in `@dayseam/ipc-types` and the Rust side's
//!   `#[ts(export)]` generates a separate TypeScript type; the
//!   view-model can relax defaults on its own side without affecting
//!   the persisted shape.
//! - **A config field has a build-time default.** Example: a GitHub
//!   API base URL that defaults to `https://api.github.com` for
//!   github.com users. Apply the default at the **construction**
//!   boundary (the IPC layer calls
//!   `SourceConfig::GitHub { api_base_url: api_base_url.unwrap_or_else(default_github_api_base_url) }`)
//!   rather than via `#[serde(default)]` on the persisted type. A
//!   row persisted with an explicit `api_base_url` is an independent
//!   piece of state that a future version of the binary may want to
//!   read verbatim (e.g. for a GitHub Enterprise user whose hostname
//!   should not silently re-default). Pushing the default to the
//!   construction boundary keeps the persisted shape honest about
//!   what was actually written.
//!
//! *(Authored DAY-94 — see
//! [`docs/review/v0.3-review.md`](https://github.com/vedanthvdev/dayseam/blob/master/docs/review/v0.3-review.md)
//! CONS-v0.3-01 for the finding that motivated this module.)*
