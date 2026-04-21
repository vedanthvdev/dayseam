//! Rapid-save collapse for Confluence page edits.
//!
//! Confluence auto-saves drafts in the background — a user who keeps a
//! page open for ten minutes and types into it can trigger dozens of
//! `version.number` bumps with identical author / same content id /
//! timestamps spaced seconds apart. Emitting one `ConfluencePageEdited`
//! bullet per auto-save would drown the EOD narrative; the rollup
//! collapses consecutive same-author edits on the same page whose
//! `occurred_at` gaps fall inside a five-minute window into one event.
//!
//! In the v0.2 scaffold of the walker, the CQL search returns one row
//! per content id (carrying only `version.when` / `version.number` of
//! the *latest* version), so a normal walk emits at most one
//! `ConfluencePageEdited` per page and this function is a no-op. It
//! still lives here because:
//!
//! * The `walk.rs` integration test
//!   [`walk_day_collapses_rapid_saves_into_one_page_edited`] hands the
//!   walker a hand-assembled multi-version fixture to prove the
//!   invariant end-to-end — the collapse must still be correct the
//!   moment a future version of this crate iterates per-version
//!   history (`/wiki/rest/api/content/{id}/history`).
//! * The pure-function shape parallels
//!   [`connector_jira::rollup::collapse_rapid_transitions`], which
//!   keeps the sibling connectors symmetric for reviewers.
//!
//! ## Invariants
//!
//! 1. Input is pre-filtered to `ActivityKind::ConfluencePageEdited`
//!    events for a single `(content_id, author_account_id)` pair and
//!    pre-sorted ascending by `occurred_at`.
//! 2. Consecutive entries whose gap is `<=
//!    [`RAPID_SAVE_WINDOW_SECONDS`] collapse into one entry that keeps
//!    the *latest* `occurred_at` (the user's subjective "last touch"
//!    moment) and records how many raw saves fused.
//! 3. A single input entry round-trips unchanged with
//!    `save_count == 1`.

use chrono::{DateTime, Utc};

/// Maximum gap (in seconds) between two consecutive same-author edits
/// on the same page for them to be considered part of the same save
/// cascade. Five minutes is the plan's Task 8 cutoff and matches the
/// spike's §8 observation that Confluence auto-saves drafts roughly
/// every minute while a page is open.
pub const RAPID_SAVE_WINDOW_SECONDS: i64 = 5 * 60;

/// One edit candidate, opaquely keyed on `content_id` by the caller.
/// The walker hands a `Vec<PageEditRecord>` to the normaliser / rollup
/// after filtering by self-author and page id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEditRecord {
    pub occurred_at: DateTime<Utc>,
    /// Version number at the time of this save. Carried through so a
    /// downstream `ConfluencePageEdited` event can surface the latest
    /// version number on its metadata payload.
    pub version_number: u32,
}

/// Result of collapsing a same-author run of edits on one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollapsedEdit {
    /// `occurred_at` of the *latest* entry in the cascade — matches the
    /// spike's "user's subjective last-touch" intuition.
    pub occurred_at: DateTime<Utc>,
    /// Latest version number observed in the cascade.
    pub version_number: u32,
    /// How many raw saves fused into this entry. `> 1` implies the
    /// renderer should hint at the collapse (e.g. "rolled up from N
    /// saves"), matching the Jira transition rollup shape.
    pub save_count: u32,
}

/// Collapse consecutive same-author edits on one page whose
/// `occurred_at` gaps are inside [`RAPID_SAVE_WINDOW_SECONDS`].
///
/// The function is pure; the caller is responsible for pre-sorting
/// ascending by `occurred_at` and for filtering to a single
/// `(content_id, author_account_id)` pair. Empty input yields empty
/// output; a singleton yields one `CollapsedEdit` with `save_count == 1`.
pub fn collapse_rapid_edits(edits: &[PageEditRecord]) -> Vec<CollapsedEdit> {
    let mut out: Vec<CollapsedEdit> = Vec::with_capacity(edits.len());
    for e in edits {
        match out.last_mut() {
            Some(last) => {
                let gap = (e.occurred_at - last.occurred_at).num_seconds();
                if (0..=RAPID_SAVE_WINDOW_SECONDS).contains(&gap) {
                    last.occurred_at = e.occurred_at;
                    last.version_number = e.version_number;
                    last.save_count = last.save_count.saturating_add(1);
                } else {
                    out.push(singleton(e));
                }
            }
            None => out.push(singleton(e)),
        }
    }
    out
}

fn singleton(e: &PageEditRecord) -> CollapsedEdit {
    CollapsedEdit {
        occurred_at: e.occurred_at,
        version_number: e.version_number,
        save_count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs_offset: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap() + chrono::Duration::seconds(secs_offset)
    }

    fn e(offset: i64, version: u32) -> PageEditRecord {
        PageEditRecord {
            occurred_at: at(offset),
            version_number: version,
        }
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert!(collapse_rapid_edits(&[]).is_empty());
    }

    #[test]
    fn single_edit_round_trips_with_save_count_one() {
        let out = collapse_rapid_edits(&[e(0, 2)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].save_count, 1);
        assert_eq!(out[0].version_number, 2);
    }

    #[test]
    fn five_autosaves_thirty_seconds_apart_collapse_to_one() {
        // The spike's autosave cadence: 5 saves, 30s apart = 2 minutes
        // total. Every gap is 30s, well inside the 5-minute window, so
        // all five fuse into one edit carrying the latest version.
        let out = collapse_rapid_edits(&[e(0, 2), e(30, 3), e(60, 4), e(90, 5), e(120, 6)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].save_count, 5);
        assert_eq!(out[0].version_number, 6);
        assert_eq!(out[0].occurred_at, at(120));
    }

    #[test]
    fn edits_across_window_boundary_split_into_two_runs() {
        // Gap from t=0 to t=60 is within the window; t=60 to t=600 is
        // 540s = 9 minutes > 5-minute window → split.
        let out = collapse_rapid_edits(&[e(0, 2), e(60, 3), e(600, 4), e(630, 5)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].save_count, 2);
        assert_eq!(out[1].save_count, 2);
    }

    #[test]
    fn exactly_five_minute_gap_is_inside_the_window() {
        // Inclusive upper bound: exactly 300s collapses. The Jira rapid-
        // transition rollup uses the same inclusive policy and the
        // rationale is identical: clock drift across Atlassian edge
        // servers shouldn't split a real autosave run in two.
        let out = collapse_rapid_edits(&[e(0, 2), e(300, 3)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].save_count, 2);
    }

    #[test]
    fn five_minute_one_second_gap_splits() {
        let out = collapse_rapid_edits(&[e(0, 2), e(301, 3)]);
        assert_eq!(out.len(), 2);
    }
}
