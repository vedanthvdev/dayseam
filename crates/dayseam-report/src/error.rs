//! Typed errors produced by the report engine.
//!
//! Kept distinct from [`dayseam_core::DayseamError`] so the engine's
//! *programmer-error* surface (unknown template id, template source
//! fails to render) never leaks into runtime error plumbing that's
//! better off matching on connector/sink errors only.

use thiserror::Error;

/// Errors returned by [`crate::render`].
#[derive(Debug, Error)]
pub enum ReportError {
    /// The caller asked for a `template_id` the engine doesn't know
    /// about. Carries the unknown id so the orchestrator can include
    /// it verbatim in the user-visible toast.
    #[error("unknown template id: {0}")]
    UnknownTemplate(String),

    /// Handlebars rejected a bundled template source. This is a
    /// programmer error (only bundled, version-locked templates
    /// reach this path in v0.1) and is surfaced as a typed error
    /// rather than a panic so an orchestrator run fails loudly rather
    /// than crashing the entire app.
    #[error("failed to render template `{template_id}`: {source}")]
    Render {
        /// The template id whose source rejected the render call.
        template_id: String,
        /// The wrapped Handlebars error.
        #[source]
        source: handlebars::RenderError,
    },

    /// The bundled template source failed to register at engine
    /// construction. Same reasoning as [`Self::Render`] — only
    /// reachable from a code edit that breaks a version-locked
    /// template, so it stays typed rather than panicking.
    #[error("failed to register template `{template_id}`: {source}")]
    Register {
        /// The template id whose source failed to register.
        template_id: String,
        /// The wrapped Handlebars registration error.
        #[source]
        source: handlebars::TemplateError,
    },
}
