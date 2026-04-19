//! Structured [`ReportDraft`] → markdown-body text.
//!
//! `dayseam-report` produces the structural draft (sections + bullets +
//! evidence) and deliberately leaves final markdown assembly to the sink.
//! Keeping the assembly local means each sink gets to pick its own dialect
//! (bullet glyph, heading level, blank-line convention) without coordinating
//! with the renderer, and the rendered bytes never leak out of the sink that
//! owns them.
//!
//! The dialect used by this sink is intentionally plain:
//!
//! ```text
//! ## Section title
//!
//! - bullet one
//! - bullet two
//! ```
//!
//! One blank line between the heading and its bullets; one blank line
//! between adjacent sections; a single trailing newline on the whole
//! fragment so the marker-block end delimiter lands on its own line.

use dayseam_core::ReportDraft;

/// Render the sections + bullets of `draft` into the body text that
/// lives between the begin and end markers. Does *not* include the
/// marker lines themselves — that is the caller's responsibility.
pub(crate) fn render_body(draft: &ReportDraft) -> String {
    let mut out = String::new();
    for (idx, section) in draft.sections.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str("## ");
        out.push_str(&section.title);
        out.push_str("\n\n");
        for bullet in &section.bullets {
            out.push_str("- ");
            out.push_str(&bullet.text);
            out.push('\n');
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};
    use dayseam_core::{RenderedBullet, RenderedSection};
    use std::collections::HashMap;
    use uuid::Uuid;

    fn draft_with_sections(sections: Vec<RenderedSection>) -> ReportDraft {
        ReportDraft {
            id: Uuid::nil(),
            date: NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(),
            template_id: "dayseam.dev_eod".to_string(),
            template_version: "2026-04-18".to_string(),
            sections,
            evidence: Vec::new(),
            per_source_state: HashMap::new(),
            verbose_mode: false,
            generated_at: Utc::now(),
        }
    }

    #[test]
    fn single_section_renders_heading_then_bullets() {
        let section = RenderedSection {
            id: "commits".to_string(),
            title: "Commits".to_string(),
            bullets: vec![
                RenderedBullet {
                    id: "b1".to_string(),
                    text: "first bullet".to_string(),
                },
                RenderedBullet {
                    id: "b2".to_string(),
                    text: "second bullet".to_string(),
                },
            ],
        };
        let out = render_body(&draft_with_sections(vec![section]));
        assert_eq!(out, "## Commits\n\n- first bullet\n- second bullet\n");
    }

    #[test]
    fn multiple_sections_are_separated_by_blank_line() {
        let a = RenderedSection {
            id: "a".to_string(),
            title: "Alpha".to_string(),
            bullets: vec![RenderedBullet {
                id: "ba".to_string(),
                text: "one".to_string(),
            }],
        };
        let b = RenderedSection {
            id: "b".to_string(),
            title: "Beta".to_string(),
            bullets: vec![RenderedBullet {
                id: "bb".to_string(),
                text: "two".to_string(),
            }],
        };
        let out = render_body(&draft_with_sections(vec![a, b]));
        assert_eq!(out, "## Alpha\n\n- one\n\n## Beta\n\n- two\n");
    }

    #[test]
    fn empty_section_still_renders_heading() {
        let section = RenderedSection {
            id: "x".to_string(),
            title: "Nothing yet".to_string(),
            bullets: Vec::new(),
        };
        let out = render_body(&draft_with_sections(vec![section]));
        assert_eq!(out, "## Nothing yet\n\n");
    }
}
