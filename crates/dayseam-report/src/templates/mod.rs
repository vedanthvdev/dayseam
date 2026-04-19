//! Template registry for the report engine.
//!
//! Templates are identified by a stable `template_id` (e.g.
//! `dayseam.dev_eod`). The registry is built once per render call via
//! [`build_registry`] so concurrent renders cannot step on each
//! other's partials. Phase 2 ships exactly one template — the Dev EOD
//! template in [`dev_eod`].

pub(crate) mod dev_eod;

use handlebars::Handlebars;

use crate::error::ReportError;

/// The Phase 2 default template id. Kept as a public constant because
/// the orchestrator needs to pass it on [`crate::ReportInput::template_id`]
/// and the CLI default will eventually read it too.
pub const DEV_EOD_TEMPLATE_ID: &str = "dayseam.dev_eod";

/// Template version string stamped onto the resulting
/// [`crate::ReportDraft::template_version`]. Bumped whenever the
/// rendered output would change for the same input; pure cosmetic
/// changes in comments stay on the same version.
pub const DEV_EOD_TEMPLATE_VERSION: &str = "2026-04-18";

/// Build a fresh [`Handlebars`] registry with every bundled template
/// + partial registered.
///
/// A new registry per render keeps the engine pure and side-effect
/// free.
pub(crate) fn build_registry<'reg>() -> Result<Handlebars<'reg>, ReportError> {
    let mut reg = Handlebars::new();
    // Strict mode turns missing variables into typed errors. That's
    // exactly the "loud failure" discipline the engine wants — a
    // typo in a template context never silently renders empty.
    reg.set_strict_mode(true);

    dev_eod::register(&mut reg)?;

    Ok(reg)
}
