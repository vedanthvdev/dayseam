//! Rapid-review collapse for GitHub PR review events.
//!
//! GitHub lets a reviewer pile multiple review "submits" onto the
//! same PR in rapid succession — a drive-by "approved", followed by
//! a "commented" on one thread, followed by another "approved" once
//! the push lands. In the EOD narrative those three lines read as
//! one review: rolling them into a single bullet with
//! `review_count = N` and the final `state` avoids the
//! "I reviewed PR #42 three times in ten seconds" noise pattern
//! that Jira's rapid-transition collapse was designed to fix
//! (DAY-77).
//!
//! The collapse is local to this module — the walker calls
//! [`collapse_rapid_reviews`] after it finishes assembling the
//! per-day event list. The report engine (DAY-97) does not need to
//! know about it: it sees a single `GitHubPullRequestReviewed` event
//! with a stable metadata shape.
//!
//! Symmetric with `connector-jira::rollup::collapse_rapid_transitions`.

use dayseam_core::{ActivityEvent, ActivityKind};

/// Window inside which consecutive reviews on the same PR by the
/// same author fold. Defaults to 60s — the same shape as Jira's
/// rapid-transition window (DAY-77). Kept as a `const` so a future
/// tweak is one line + one test.
pub const RAPID_REVIEW_WINDOW_SECONDS: i64 = 60;

/// Collapse consecutive `GitHubPullRequestReviewed` events on the
/// same PR by the same author within
/// [`RAPID_REVIEW_WINDOW_SECONDS`] into one. The collapsed event
/// keeps the **last** row's `state` (so "requested changes then
/// approved" reads as "approved") and sums `review_count` into
/// `metadata.review_count`.
///
/// Input may be in any order; the function sorts by
/// `(parent_external_id, occurred_at)` before grouping and
/// restores the original input's relative ordering afterwards for
/// determinism.
///
/// Non-review events pass through untouched.
#[must_use]
pub fn collapse_rapid_reviews(events: Vec<ActivityEvent>) -> Vec<ActivityEvent> {
    // Partition into reviews / non-reviews. Reviews go through the
    // sort-and-collapse pass; non-reviews are re-emitted in input
    // order at the tail.
    let mut reviews: Vec<ActivityEvent> = Vec::new();
    let mut others: Vec<ActivityEvent> = Vec::new();
    for ev in events {
        if ev.kind == ActivityKind::GitHubPullRequestReviewed {
            reviews.push(ev);
        } else {
            others.push(ev);
        }
    }

    if reviews.is_empty() {
        return others;
    }

    // Group by (parent_external_id, actor.external_id).
    reviews.sort_by(|a, b| {
        let pa = a.parent_external_id.clone().unwrap_or_default();
        let pb = b.parent_external_id.clone().unwrap_or_default();
        let aa = a.actor.external_id.clone().unwrap_or_default();
        let ab = b.actor.external_id.clone().unwrap_or_default();
        pa.cmp(&pb)
            .then(aa.cmp(&ab))
            .then(a.occurred_at.cmp(&b.occurred_at))
            .then(a.id.cmp(&b.id))
    });

    let mut collapsed: Vec<ActivityEvent> = Vec::new();
    for ev in reviews {
        match collapsed.last_mut() {
            Some(prev)
                if same_thread(prev, &ev)
                    && within_rapid_window(prev.occurred_at, ev.occurred_at) =>
            {
                // Fold the current review into the previous: bump
                // `review_count`, overwrite `state` with the newer
                // one, advance `occurred_at` to the newer
                // timestamp, and keep the newer event's `id`
                // deterministic — re-derive from the latest
                // external_id + kind so the row stays stable across
                // re-syncs.
                let prev_count = prev
                    .metadata
                    .get("review_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1);
                let new_count = prev_count
                    + ev.metadata
                        .get("review_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(1);
                if let Some(state) = ev.metadata.get("review_state").cloned() {
                    prev.metadata["review_state"] = state;
                }
                prev.metadata["review_count"] = serde_json::Value::from(new_count);
                prev.occurred_at = ev.occurred_at;
                // Title overwritten so the final render keys on the
                // latest review's PR title (in case the PR was
                // renamed between reviews).
                prev.title = ev.title;
            }
            _ => collapsed.push(ev),
        }
    }

    // Stable return order: collapsed reviews first (ordered by
    // occurred_at for the caller's convenience; the walker does the
    // final sort), then the non-review events.
    collapsed.sort_by(|a, b| {
        a.occurred_at
            .cmp(&b.occurred_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    collapsed.extend(others);
    collapsed
}

fn same_thread(a: &ActivityEvent, b: &ActivityEvent) -> bool {
    let same_pr = a.parent_external_id == b.parent_external_id && a.parent_external_id.is_some();
    let same_actor = a.actor.external_id == b.actor.external_id;
    same_pr && same_actor
}

fn within_rapid_window(
    earlier: chrono::DateTime<chrono::Utc>,
    later: chrono::DateTime<chrono::Utc>,
) -> bool {
    let delta = later.signed_duration_since(earlier);
    delta.num_seconds().abs() <= RAPID_REVIEW_WINDOW_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use dayseam_core::{Actor, EntityRef, Link, Privacy, RawRef};
    use uuid::Uuid;

    fn review_event(id_suffix: u64, offset_seconds: i64, state: &str) -> ActivityEvent {
        let base = Utc.with_ymd_and_hms(2026, 4, 20, 12, 0, 0).unwrap();
        ActivityEvent {
            id: Uuid::from_u128(0xabcd_0000_0000_0000_0000_0000_0000_0000 + id_suffix as u128),
            source_id: Uuid::new_v4(),
            external_id: "modulr/foo#42".into(),
            kind: ActivityKind::GitHubPullRequestReviewed,
            occurred_at: base + ChronoDuration::seconds(offset_seconds),
            actor: Actor {
                display_name: "vedanth".into(),
                email: None,
                external_id: Some("17".into()),
            },
            title: format!("Reviewed PR: Add payments ({state})"),
            body: None,
            links: vec![Link {
                url: "https://github.com/modulr/foo/pull/42".into(),
                label: Some("#42".into()),
            }],
            entities: vec![EntityRef {
                kind: dayseam_core::EntityKind::GitHubPullRequest,
                external_id: "modulr/foo#42".into(),
                label: Some("#42".into()),
            }],
            parent_external_id: Some("modulr/foo#42".into()),
            metadata: serde_json::json!({
                "github_event_id": format!("evt-{id_suffix}"),
                "review_state": state,
                "review_count": 1,
            }),
            raw_ref: RawRef {
                storage_key: format!("github:event:evt-{id_suffix}"),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    #[test]
    fn three_rapid_reviews_collapse_to_one_with_final_state() {
        let events = vec![
            review_event(1, 0, "commented"),
            review_event(2, 10, "changes_requested"),
            review_event(3, 30, "approved"),
        ];
        let out = collapse_rapid_reviews(events);
        assert_eq!(out.len(), 1);
        let ev = &out[0];
        assert_eq!(
            ev.metadata.get("review_count").and_then(|v| v.as_i64()),
            Some(3)
        );
        assert_eq!(
            ev.metadata.get("review_state").and_then(|v| v.as_str()),
            Some("approved"),
            "final state wins"
        );
    }

    #[test]
    fn reviews_separated_by_more_than_window_do_not_collapse() {
        let events = vec![
            review_event(1, 0, "approved"),
            // 90s later — outside the 60s window.
            review_event(2, 90, "commented"),
        ];
        let out = collapse_rapid_reviews(events);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn reviews_on_different_prs_do_not_collapse() {
        let a = review_event(1, 0, "approved");
        let mut b = review_event(2, 10, "approved");
        b.parent_external_id = Some("modulr/foo#43".into());
        b.external_id = "modulr/foo#43".into();
        let out = collapse_rapid_reviews(vec![a.clone(), b.clone()]);
        assert_eq!(out.len(), 2);
        // Deterministic order by occurred_at: a first (0s), b next (10s).
        assert_eq!(out[0].parent_external_id, a.parent_external_id);
        assert_eq!(out[1].parent_external_id, b.parent_external_id);
    }

    #[test]
    fn non_review_events_pass_through() {
        let non_review = ActivityEvent {
            kind: ActivityKind::GitHubPullRequestOpened,
            ..review_event(9, 0, "approved")
        };
        let out = collapse_rapid_reviews(vec![non_review.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, ActivityKind::GitHubPullRequestOpened);
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(collapse_rapid_reviews(vec![]).is_empty());
    }

    #[test]
    fn out_of_order_input_is_normalised_by_occurred_at() {
        let events = vec![
            review_event(3, 30, "approved"),
            review_event(1, 0, "commented"),
            review_event(2, 10, "changes_requested"),
        ];
        let out = collapse_rapid_reviews(events);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].metadata.get("review_count").and_then(|v| v.as_i64()),
            Some(3)
        );
    }
}
