//! The Dev EOD template.
//!
//! v0.1 ships one template. The full v0.1 design (`docs/design/2026-04-17-v0.1-design.md`
//! §7.1.2) shows sections for merge requests, issues, and reviews —
//! those only come online once the GitLab connector (Phase 3) starts
//! emitting those artifact kinds. For Phase 2 the template is
//! deliberately scoped to the `CommitSet` artifacts
//! `connector-local-git` produces.
//!
//! The template is two Handlebars partials — the two logical hinges
//! future templates will reuse:
//!
//! * `section_commits` — renders the markdown fragment that becomes a
//!   bullet's [`dayseam_core::RenderedBullet::text`]. Handles the
//!   normal case and the verbose-mode expansion.
//! * `evidence_link` — renders the inline evidence suffix
//!   ("_1 commit_"). Kept in its own partial so templates that
//!   aggregate differently (e.g. a weekly rollup) can replace it
//!   without forking the whole section.
//!
//! Partial sources are embedded as string constants so `cargo test`
//! works on any machine without a working-directory assumption.

use handlebars::Handlebars;

use crate::error::ReportError;

/// `section_commits` partial source. Input is a [`crate::render::BulletCtx`]
/// (see `render.rs`).
const SECTION_COMMITS: &str = "{{headline}} — {{> evidence_link evidence=evidence}}\
{{#if verbose_mode}}\
{{#each verbose_lines}}
  - {{{this}}}\
{{/each}}\
{{/if}}";

/// `evidence_link` partial source. Expects a `{ "evidence": "1 commit" }`
/// context; rendered both inline from `section_commits` and directly
/// by tests that want to snapshot just the evidence suffix.
const EVIDENCE_LINK: &str = "_{{evidence}}_";

pub(crate) fn register(reg: &mut Handlebars<'_>) -> Result<(), ReportError> {
    reg.register_partial("section_commits", SECTION_COMMITS)
        .map_err(|source| ReportError::Register {
            template_id: format!("{}::section_commits", super::DEV_EOD_TEMPLATE_ID),
            source,
        })?;
    reg.register_partial("evidence_link", EVIDENCE_LINK)
        .map_err(|source| ReportError::Register {
            template_id: format!("{}::evidence_link", super::DEV_EOD_TEMPLATE_ID),
            source,
        })?;
    Ok(())
}

/// Render the free-standing evidence suffix (`_1 commit_`) without
/// going through the Handlebars registry. Kept next to the partial
/// it mirrors so the two forms never drift.
pub(crate) fn render_evidence_suffix(reason: &str) -> String {
    format!("_{reason}_")
}
