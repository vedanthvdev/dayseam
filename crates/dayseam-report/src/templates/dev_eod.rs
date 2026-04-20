//! The Dev EOD template.
//!
//! v0.1 ships one template. The full v0.1 design (`docs/design/2026-04-17-v0.1-design.md`
//! §7.1.2) shows sections for merge requests, issues, and reviews —
//! those only come online once the GitLab connector (Phase 3) starts
//! emitting those artifact kinds. For Phase 2 the template is
//! deliberately scoped to the `CommitSet` artifacts
//! `connector-local-git` produces.
//!
//! `section_commits` is the only partial the template registers. It
//! renders one bullet per commit (the Phase 2 rule — see
//! `render.rs` for the rationale) with an optional verbose-mode SHA
//! suffix. The older `evidence_link` partial (rendering
//! "_N commits_") was removed in DAY-52 along with the
//! per-CommitSet bullet shape it supported; reintroduce it alongside
//! whichever aggregated artifact kind needs it.
//!
//! Partial sources are embedded as string constants so `cargo test`
//! works on any machine without a working-directory assumption.

use handlebars::Handlebars;

use crate::error::ReportError;

/// `section_commits` partial source. Input is a
/// [`crate::render::CommitBulletCtx`] (see `render.rs`).
///
/// * Non-verbose: `{{headline}}` — a single markdown bullet.
/// * Verbose: `{{headline}} · `{{short_sha}}` (rolled into !42)` —
///   the short-SHA and the optional `(rolled into !N)` suffix both
///   render only when `verbose_mode` is true. The plain text is a
///   strict prefix of the verbose text, which
///   `tests/invariants.rs::verbose_mode_only_adds_bullets`
///   depends on. The `(rolled into !N)` suffix lands here per
///   Phase 3 Task 2 when the orchestrator's
///   `annotate_rolled_into_mr` pass stamps the event with an MR iid.
const SECTION_COMMITS: &str = concat!(
    "{{headline}}",
    "{{#if verbose_mode}}",
    "{{#if short_sha}} · `{{short_sha}}`{{/if}}",
    "{{#if rolled_into_mr}} (rolled into {{rolled_into_mr}}){{/if}}",
    "{{/if}}",
);

pub(crate) fn register(reg: &mut Handlebars<'_>) -> Result<(), ReportError> {
    reg.register_partial("section_commits", SECTION_COMMITS)
        .map_err(|source| ReportError::Register {
            template_id: format!("{}::section_commits", super::DEV_EOD_TEMPLATE_ID),
            source,
        })?;
    Ok(())
}
