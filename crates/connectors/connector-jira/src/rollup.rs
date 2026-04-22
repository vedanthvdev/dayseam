//! Rapid-transition rollup for a single issue's changelog.
//!
//! The spike's motivating anecdote is `CAR-5117`, which cascaded from
//! `Work In Progress` → `In Review` → `In Test` → `In Test Regression`
//! → `Regression Passed` → `Production Pending` → `Production
//! Verification` in under 60 seconds as a workflow automation rollup
//! kicked in. Emitting six `JiraIssueTransitioned` bullets for that
//! cascade would drown the EOD narrative; the rollup collapses
//! consecutive same-author status transitions that occur within a
//! short time window into a single event that carries the *earliest*
//! `from` and the *latest* `to`, plus a `transition_count` on the
//! metadata so the renderer can mention the cascade.
//!
//! The rollup is a pure function over an already-ordered slice of
//! [`StatusTransition`] records. The caller (the normaliser) is
//! responsible for filtering `field == "status"` changelog items
//! authored by the self-user and sorting them by `created_at`
//! ascending before passing them in.

use chrono::{DateTime, Utc};

/// The maximum gap (in seconds) between two consecutive same-author
/// status transitions for them to be considered part of one cascade.
/// 60 seconds is the spike's empirical cutoff — longer than any
/// bot-triggered transition chain we observed, shorter than any
/// human-driven "I touched this ticket twice in one session" pattern.
pub const RAPID_TRANSITION_WINDOW_SECONDS: i64 = 60;

/// One status-change entry extracted from a Jira changelog history.
/// Pre-filtered by the normaliser (self-author only, `field == "status"`
/// only) before reaching the rollup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusTransition {
    pub created_at: DateTime<Utc>,
    pub from_status: String,
    pub to_status: String,
    /// The category key (`new`, `indeterminate`, `done`) the target
    /// status belongs to. Surfaced on the rolled-up event so the
    /// renderer can choose verbiage based on "did this ticket reach
    /// done today?" without a second lookup.
    pub status_category: String,
}

/// The result of collapsing a cascade. `transition_count == 1` means
/// the item passed through the rollup untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollapsedTransition {
    /// `created_at` of the *latest* entry in the cascade — the bullet
    /// sorts by the time the last transition happened, which is what
    /// a user would notice in the activity feed.
    pub created_at: DateTime<Utc>,
    /// `from_status` of the *earliest* entry in the cascade.
    pub from_status: String,
    /// `to_status` of the *latest* entry in the cascade.
    pub to_status: String,
    /// `status_category` of the *latest* entry.
    pub status_category: String,
    /// Number of raw transitions that collapsed into this event.
    /// Renderer will note `(rolled up from N transitions)` when this
    /// is greater than 1.
    pub transition_count: u32,
    /// Intermediate statuses the cascade passed through, in
    /// chronological order, exclusive of `from_status` (earliest) and
    /// `to_status` (latest). Always has `transition_count.saturating_sub(1)`
    /// entries (empty when `transition_count <= 1`).
    ///
    /// Added in DAY-88 (CORR-v0.2-04): pre-fix, a cascade
    /// `Todo → InProgress → Review → Done` rendered as `Todo → Done`
    /// and every intermediate state was silently discarded, which broke
    /// audit use cases (e.g. "did this ticket ever enter Review
    /// today?"). The field is serialised into `metadata.via` on the
    /// emitted `ActivityEvent`; renderers can ignore it, and the
    /// rollup test suite pins its chronological invariant.
    pub via: Vec<String>,
}

/// Collapse consecutive transitions whose `created_at` gaps are within
/// [`RAPID_TRANSITION_WINDOW_SECONDS`] into one [`CollapsedTransition`]
/// per cascade. Input must be pre-sorted ascending by `created_at`;
/// the rollup is a single pass over the slice.
///
/// If `transitions` is empty the result is empty; if it has one
/// element the rollup is a no-op returning a singleton `CollapsedTransition`
/// with `transition_count == 1`.
pub fn collapse_rapid_transitions(transitions: &[StatusTransition]) -> Vec<CollapsedTransition> {
    let mut out: Vec<CollapsedTransition> = Vec::with_capacity(transitions.len());
    for t in transitions {
        match out.last_mut() {
            Some(last) => {
                let gap = (t.created_at - last.created_at).num_seconds();
                if (0..=RAPID_TRANSITION_WINDOW_SECONDS).contains(&gap) {
                    // Extend the cascade: keep the earliest `from`, push
                    // the previous `to_status` (which is about to be
                    // overwritten) onto `via`, move `to` /
                    // `status_category` / `created_at` forward, bump
                    // the count.
                    last.via.push(std::mem::take(&mut last.to_status));
                    last.to_status = t.to_status.clone();
                    last.status_category = t.status_category.clone();
                    last.created_at = t.created_at;
                    last.transition_count = last.transition_count.saturating_add(1);
                } else {
                    out.push(singleton(t));
                }
            }
            None => out.push(singleton(t)),
        }
    }
    out
}

fn singleton(t: &StatusTransition) -> CollapsedTransition {
    CollapsedTransition {
        created_at: t.created_at,
        from_status: t.from_status.clone(),
        to_status: t.to_status.clone(),
        status_category: t.status_category.clone(),
        transition_count: 1,
        via: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs_offset: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap() + chrono::Duration::seconds(secs_offset)
    }

    fn t(offset: i64, from: &str, to: &str) -> StatusTransition {
        StatusTransition {
            created_at: at(offset),
            from_status: from.into(),
            to_status: to.into(),
            status_category: "indeterminate".into(),
        }
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert!(collapse_rapid_transitions(&[]).is_empty());
    }

    #[test]
    fn single_transition_round_trips_with_count_one() {
        let out = collapse_rapid_transitions(&[t(0, "To Do", "In Progress")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transition_count, 1);
        assert_eq!(out[0].from_status, "To Do");
        assert_eq!(out[0].to_status, "In Progress");
    }

    #[test]
    fn six_transition_cascade_within_window_collapses_to_one() {
        // Mirrors the spike's CAR-5117 anecdote: 6 transitions over
        // ~30 seconds. All consecutive gaps are ≤ 60s, so they fuse.
        let items = [
            t(0, "Work In Progress", "In Review"),
            t(5, "In Review", "In Test"),
            t(10, "In Test", "In Test Regression"),
            t(15, "In Test Regression", "Regression Passed"),
            t(20, "Regression Passed", "Production Pending"),
            t(25, "Production Pending", "Production Verification"),
        ];
        let out = collapse_rapid_transitions(&items);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transition_count, 6);
        assert_eq!(out[0].from_status, "Work In Progress");
        assert_eq!(out[0].to_status, "Production Verification");
        assert_eq!(out[0].created_at, at(25));
    }

    #[test]
    fn transitions_across_window_boundary_split_into_two_cascades() {
        let items = [
            t(0, "A", "B"),
            t(30, "B", "C"),
            // Gap from 30 → 120 is 90 seconds → new cascade.
            t(120, "C", "D"),
            t(150, "D", "E"),
        ];
        let out = collapse_rapid_transitions(&items);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].from_status, "A");
        assert_eq!(out[0].to_status, "C");
        assert_eq!(out[0].transition_count, 2);
        assert_eq!(out[1].from_status, "C");
        assert_eq!(out[1].to_status, "E");
        assert_eq!(out[1].transition_count, 2);
    }

    #[test]
    fn exactly_60_second_gap_is_inside_the_window() {
        // The inclusive boundary is intentional: 60 == 60 collapses.
        // The outer cutoff is documented at 60 so clock-drift across
        // Atlassian server instances doesn't bleed a real cascade into
        // two bullets.
        let out = collapse_rapid_transitions(&[t(0, "A", "B"), t(60, "B", "C")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transition_count, 2);
    }

    #[test]
    fn sixty_one_second_gap_splits() {
        let out = collapse_rapid_transitions(&[t(0, "A", "B"), t(61, "B", "C")]);
        assert_eq!(out.len(), 2);
    }

    /// DAY-88 / CORR-v0.2-04. Pre-fix, a `Todo → InProgress → Review →
    /// Done` cascade rendered as `Todo → Done` and every intermediate
    /// state was lost. The test pins that `via` now preserves the
    /// intermediate hops in chronological order, exclusive of the
    /// earliest `from_status` and the latest `to_status`.
    #[test]
    fn collapse_preserves_intermediate_transitions_in_via() {
        let out = collapse_rapid_transitions(&[
            t(0, "Todo", "InProgress"),
            t(5, "InProgress", "Review"),
            t(10, "Review", "Done"),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].from_status, "Todo");
        assert_eq!(out[0].to_status, "Done");
        assert_eq!(
            out[0].via,
            vec!["InProgress".to_string(), "Review".to_string()],
            "via must list every intermediate hop in chronological order"
        );
        assert_eq!(out[0].transition_count, 3);
    }

    /// Edge cases for `via` at the boundaries.
    ///
    /// * A singleton (one transition) has `transition_count == 1` and
    ///   no intermediate hops.
    /// * A two-step cascade `[A→B, B→C]` covers three states `A, B, C`;
    ///   `from = A`, `to = C`, and the one intermediate state `B`
    ///   moves into `via`.
    #[test]
    fn collapse_via_has_n_minus_one_entries_for_n_transitions() {
        let singleton = collapse_rapid_transitions(&[t(0, "A", "B")]);
        assert_eq!(singleton[0].transition_count, 1);
        assert_eq!(singleton[0].via, Vec::<String>::new());

        let two_step = collapse_rapid_transitions(&[t(0, "A", "B"), t(5, "B", "C")]);
        assert_eq!(two_step[0].from_status, "A");
        assert_eq!(two_step[0].to_status, "C");
        assert_eq!(two_step[0].via, vec!["B".to_string()]);
        assert_eq!(two_step[0].transition_count, 2);
    }
}
