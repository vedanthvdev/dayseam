//! The Dayseam report engine.
//!
//! The engine is a **pure function** of its input. No IO, no clocks, no
//! randomness: every field that looks like it would need a side-effect
//! (`ReportDraft::id`, `generated_at`, the evidence timestamps) comes
//! in on [`ReportInput`] so the same input always yields a
//! byte-identical output.
//!
//! ```text
//! events + artifacts + template_id + template_version + person +
//! source_identities + per_source_state + verbose_mode + id + date +
//! generated_at  →  ReportDraft
//! ```
//!
//! See `ARCHITECTURE.md` §7A (canonical artifact layer) and §10
//! (report engine) for the larger picture this crate implements.

#![deny(missing_docs)]

mod dedup;
mod error;
mod input;
mod render;
mod rollup;
mod rollup_mr;
mod templates;

pub use dayseam_core::{Evidence, RenderedBullet, RenderedSection, ReportDraft};
pub use dedup::dedup_commit_authored;
pub use error::ReportError;
pub use input::ReportInput;
pub use rollup_mr::{annotate_rolled_into_mr, MergeRequestArtifact};
pub use templates::{DEV_EOD_TEMPLATE_ID, DEV_EOD_TEMPLATE_VERSION};

/// Render a [`ReportInput`] into a [`ReportDraft`].
///
/// This is the only public entry point of the engine. Callers (the
/// orchestrator, tests, the future CLI) construct a [`ReportInput`]
/// and receive either a rendered draft or a typed [`ReportError`] —
/// never a partial result.
///
/// # Errors
///
/// Returns [`ReportError::UnknownTemplate`] if `input.template_id` is
/// not registered and [`ReportError::Render`] if Handlebars rejects a
/// registered template (only possible from a programmer error in the
/// bundled template sources; kept as a typed error rather than a
/// panic so the orchestrator can surface it).
pub fn render(input: ReportInput) -> Result<ReportDraft, ReportError> {
    render::render(input)
}
