//! Cross-source enrichment pipeline.
//!
//! Two passes, both pure and idempotent:
//!
//! 1. [`extract_ticket_keys`] scans every event's `title` + `body`
//!    for Jira-shaped ticket keys (`/\b[A-Z]{2,10}-\d+\b/`) and
//!    attaches a `jira_issue` [`EntityRef`] as a `target` on the
//!    event. A GitLab MR titled `"CAR-5117: Fix review findings"`
//!    gets a `jira_issue` entity pointing at `CAR-5117` with **zero
//!    Jira API calls**. Downstream passes use this to cross-link MRs
//!    to Jira transitions.
//! 2. [`annotate_transition_with_mr`] walks `JiraIssueTransitioned`
//!    events and, for each one, looks up whether the day also has a
//!    matching `MrOpened` / `MrMerged` with a `jira_issue` target
//!    pointing at the same issue key. When it does, the transition's
//!    `parent_external_id` is set to the MR's `external_id`, so the
//!    verbose-mode render can show `(triggered by !321)` next to a
//!    status change.
//!
//! # Why regex-free
//!
//! The ticket-key pattern is simple enough that a hand-rolled
//! ASCII scanner avoids pulling `regex` (and its 300 kLoC `regex-automata`
//! transitive) into the pure-function report crate. The report engine
//! is a hot path (the UI re-renders on every filter toggle) so a
//! dependency with a 2 MB binary footprint would be a net loss even
//! if we reused it elsewhere — which we don't: no other crate in the
//! workspace uses `regex`.
//!
//! # Noise bail
//!
//! Commit titles like `"Fix GH-123 by bumping LOG4J-2 from 2.17.0 to
//! 2.17.2"` contain tokens that syntactically match the pattern
//! (`LOG4J-2`) but semantically aren't Jira tickets. We can't
//! distinguish the two from the string alone, so
//! [`extract_ticket_keys`] bails when a single event surfaces more
//! than [`MAX_TICKET_KEYS_PER_EVENT`] candidates — the commit
//! probably references many tickets in a non-structured way and
//! we'd rather attach nothing than the wrong thing.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use dayseam_core::{ActivityEvent, ActivityKind, EntityKind, EntityRef};
use uuid::Uuid;

/// Bail threshold for [`extract_ticket_keys`]. See the module docs.
pub(crate) const MAX_TICKET_KEYS_PER_EVENT: usize = 3;

/// Attach a `jira_issue` [`EntityRef`] for every ticket key found in
/// each event's `title` and `body`.
///
/// Idempotent: a second call produces no new entities, because the
/// function checks for an existing `jira_issue` entity with a
/// matching `external_id` before pushing. Events that already carry
/// a `jira_issue` target (e.g. the Jira connector's own emissions)
/// are untouched.
///
/// Events with more than [`MAX_TICKET_KEYS_PER_EVENT`] unique
/// candidates are treated as noise and attached no entity — see the
/// module docs for the rationale.
pub fn extract_ticket_keys(events: &mut [ActivityEvent]) {
    for event in events.iter_mut() {
        let mut keys: Vec<String> = Vec::new();
        scan_ticket_keys(&event.title, &mut keys);
        if let Some(body) = &event.body {
            scan_ticket_keys(body, &mut keys);
        }
        keys.sort();
        keys.dedup();
        if keys.is_empty() || keys.len() > MAX_TICKET_KEYS_PER_EVENT {
            continue;
        }
        for key in keys {
            let already = event
                .entities
                .iter()
                .any(|e| e.kind == EntityKind::JiraIssue && e.external_id == key);
            if !already {
                event.entities.push(EntityRef {
                    kind: EntityKind::JiraIssue,
                    external_id: key,
                    label: None,
                });
            }
        }
    }
}

/// Annotate `JiraIssueTransitioned` events with the GitLab MR that
/// (probably) triggered them.
///
/// Uses the `jira_issue` [`EntityRef`] that [`extract_ticket_keys`]
/// attaches to MRs: an `MrOpened` / `MrMerged` whose title carried
/// the ticket key `CAR-5117` exposes a `jira_issue` entity with
/// `external_id = "CAR-5117"`. A `JiraIssueTransitioned` event for
/// `CAR-5117` then finds that MR via the index built here and stamps
/// `parent_external_id = Some(<mr_external_id>)`.
///
/// Earliest-MR-wins by `occurred_at`, with a stable tie-break on
/// `ActivityEvent::id` (UUIDv5, deterministic from
/// `(source_id, external_id, kind)` — content-addressable, so the
/// tie-break is reproducible across runs regardless of walker
/// insertion order).
///
/// Pre-DAY-88 this picked "first in input order" using
/// `HashMap::entry(_).or_insert(_)`. That was walker-insertion
/// dependent: a fan-out that merged GitLab events before Jira ones
/// (or vice-versa) changed which MR claimed a shared Jira issue.
/// CORR-v0.2-05 makes the choice temporal: the MR that happened
/// first chronologically wins — that's what the user would expect
/// to see called out in the EOD narrative as "the MR that triggered
/// the status change".
///
/// Overwrites any existing `parent_external_id` on the transition.
/// DAY-77's Jira connector populates `parent_external_id` with the
/// issue key for routing purposes, but issue key is also in the
/// event's `entities` list — the field is free to repurpose here.
///
/// No-op on events that aren't `JiraIssueTransitioned`.
/// No-op on transitions whose issue key has no matching MR.
pub fn annotate_transition_with_mr(events: &mut [ActivityEvent]) {
    let issue_to_mr = build_issue_to_mr_index(events);
    if issue_to_mr.is_empty() {
        return;
    }
    for event in events.iter_mut() {
        if event.kind != ActivityKind::JiraIssueTransitioned {
            continue;
        }
        let Some(issue_key) = event
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::JiraIssue)
            .map(|e| e.external_id.clone())
        else {
            continue;
        };
        if let Some(mr_id) = issue_to_mr.get(issue_key.as_str()) {
            event.parent_external_id = Some(mr_id.clone());
        }
    }
}

/// Build an issue-key → winning-MR-`external_id` index.
///
/// Owned strings so the caller can freely `iter_mut()` the event vec
/// after we return. Using `&str` references ties the index lifetime
/// to the borrow of `events`, which conflicts with the subsequent
/// mutable walk.
///
/// For each Jira issue referenced by any MR, picks the MR with the
/// earliest `occurred_at`. Ties break on the MR's deterministic
/// `ActivityEvent::id`, so two MRs with identical timestamps always
/// resolve the same way across runs. See
/// [`annotate_transition_with_mr`] for the rationale on switching
/// from walker-insertion order to temporal order.
fn build_issue_to_mr_index(events: &[ActivityEvent]) -> HashMap<String, String> {
    // Candidate winning MR per issue_key: (occurred_at, id, external_id).
    // The `(occurred_at, id)` tuple is `Ord` under the natural
    // lexicographic rule — earlier time wins, same-time ties break
    // on the UUID.
    let mut best: HashMap<String, (DateTime<Utc>, Uuid, String)> = HashMap::new();
    for event in events {
        if !matches!(event.kind, ActivityKind::MrOpened | ActivityKind::MrMerged) {
            continue;
        }
        for ent in &event.entities {
            if ent.kind != EntityKind::JiraIssue {
                continue;
            }
            let incoming = (event.occurred_at, event.id, event.external_id.clone());
            best.entry(ent.external_id.clone())
                .and_modify(|current| {
                    if (incoming.0, incoming.1) < (current.0, current.1) {
                        *current = incoming.clone();
                    }
                })
                .or_insert(incoming);
        }
    }
    best.into_iter()
        .map(|(issue_key, (_, _, mr_external_id))| (issue_key, mr_external_id))
        .collect()
}

/// Scan `text` for `[A-Z]{2,10}-\d+` tokens and push matches onto
/// `out` (with dedup at the caller, not here, because the caller
/// concatenates title + body before dedup).
///
/// Respects word boundaries: a leading alphanumeric or trailing
/// alphanumeric disqualifies the match, so `LOG4J-2` (the trailing
/// letter kills the word-boundary) and `FOO-42a` (trailing letter on
/// the digits) are rejected, while `[CAR-5117]` and
/// `"Merged CAR-5117:"` match.
fn scan_ticket_keys(text: &str, out: &mut Vec<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find the start of a potential key: an uppercase ASCII
        // letter with no alphanumeric char immediately before it.
        if !is_ascii_upper(bytes[i]) || (i > 0 && is_ascii_alnum(bytes[i - 1])) {
            i += 1;
            continue;
        }
        // Collect the uppercase prefix (letters only).
        let prefix_start = i;
        while i < bytes.len() && is_ascii_upper(bytes[i]) {
            i += 1;
        }
        let prefix_len = i - prefix_start;
        if !(2..=10).contains(&prefix_len) || i >= bytes.len() || bytes[i] != b'-' {
            continue;
        }
        // Skip the hyphen, collect digits.
        let hyphen = i;
        i += 1;
        let digits_start = i;
        while i < bytes.len() && is_ascii_digit(bytes[i]) {
            i += 1;
        }
        let digits_len = i - digits_start;
        if digits_len == 0 {
            // Rewind to just after the hyphen so the next scan can
            // re-evaluate from here.
            i = hyphen + 1;
            continue;
        }
        // Trailing-alnum boundary: if the byte after the digits is
        // alphanumeric, reject (e.g. `CAR-5117a`).
        if i < bytes.len() && is_ascii_alnum(bytes[i]) {
            continue;
        }
        if let Ok(token) = std::str::from_utf8(&bytes[prefix_start..i]) {
            out.push(token.to_string());
        }
    }
}

const fn is_ascii_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}

const fn is_ascii_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

const fn is_ascii_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{Actor, EntityKind, EntityRef, Privacy, RawRef, SourceId};
    use uuid::Uuid;

    fn src() -> SourceId {
        Uuid::from_u128(0x1111)
    }

    fn event(kind: ActivityKind, external_id: &str, title: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::new_v5(&Uuid::NAMESPACE_OID, external_id.as_bytes()),
            source_id: src(),
            external_id: external_id.into(),
            kind,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Self".into(),
                email: Some("self@example.com".into()),
                external_id: None,
            },
            title: title.into(),
            body: None,
            links: Vec::new(),
            entities: Vec::new(),
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: format!("k:{external_id}"),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    fn jira_transition(issue_key: &str) -> ActivityEvent {
        let mut e = event(
            ActivityKind::JiraIssueTransitioned,
            &format!("{issue_key}::transition"),
            &format!("{issue_key}: In Progress → Done"),
        );
        e.entities.push(EntityRef {
            kind: EntityKind::JiraIssue,
            external_id: issue_key.into(),
            label: None,
        });
        e
    }

    #[test]
    fn scan_simple_key() {
        let mut out = Vec::new();
        scan_ticket_keys("CAR-5117: Fix things", &mut out);
        assert_eq!(out, vec!["CAR-5117"]);
    }

    #[test]
    fn scan_key_inside_punctuation() {
        let mut out = Vec::new();
        scan_ticket_keys("Merged [CAR-5117] into main", &mut out);
        assert_eq!(out, vec!["CAR-5117"]);
    }

    #[test]
    fn scan_rejects_trailing_letter() {
        let mut out = Vec::new();
        // `LOG4J-2a` — the `a` after the digits kills the match.
        scan_ticket_keys("Bumping LOG4J-2a from 2.17", &mut out);
        assert!(out.is_empty(), "trailing alphanumeric rejects match");
    }

    #[test]
    fn scan_rejects_leading_letter() {
        let mut out = Vec::new();
        // `xCAR-1` — leading letter kills the match.
        scan_ticket_keys("xCAR-1", &mut out);
        assert!(out.is_empty(), "leading alphanumeric rejects match");
    }

    #[test]
    fn scan_rejects_too_short_or_long_prefix() {
        let mut out = Vec::new();
        // `A-1` — prefix below 2 chars.
        scan_ticket_keys("A-1 short", &mut out);
        assert!(out.is_empty());
        // 11-char prefix — above 10.
        scan_ticket_keys("ABCDEFGHIJK-1 longer than allowed", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn commit_titled_with_ticket_gains_jira_target_entity() {
        // Plan invariant 4.
        let mut events = vec![event(
            ActivityKind::CommitAuthored,
            "sha1",
            "CAR-5117: Fix review findings",
        )];
        extract_ticket_keys(&mut events);
        let targets: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::JiraIssue)
            .collect();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].external_id, "CAR-5117");
    }

    #[test]
    fn extract_ticket_keys_is_idempotent() {
        // Plan invariant 5.
        let mut events = vec![event(
            ActivityKind::CommitAuthored,
            "sha1",
            "CAR-5117: Fix review findings",
        )];
        extract_ticket_keys(&mut events);
        let first = events[0].entities.clone();
        extract_ticket_keys(&mut events);
        assert_eq!(events[0].entities, first, "second call must be a no-op");
    }

    #[test]
    fn extract_ticket_keys_bails_on_noisy_titles() {
        // Plan invariant 6. The title references four keys — we bail.
        let mut events = vec![event(
            ActivityKind::CommitAuthored,
            "sha1",
            "Fix GH-123 and FOO-4 and BAR-9 and BAZ-11 by bumping deps",
        )];
        extract_ticket_keys(&mut events);
        let targets: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::JiraIssue)
            .collect();
        assert!(
            targets.is_empty(),
            "event referencing >3 candidates attaches none"
        );
    }

    #[test]
    fn extract_ticket_keys_preserves_existing_jira_issue_targets() {
        let mut e = event(
            ActivityKind::CommitAuthored,
            "sha1",
            "CAR-5117: Fix review findings",
        );
        e.entities.push(EntityRef {
            kind: EntityKind::JiraIssue,
            external_id: "CAR-5117".into(),
            label: Some("Pre-existing".into()),
        });
        let mut events = vec![e];
        extract_ticket_keys(&mut events);
        let targets: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::JiraIssue)
            .collect();
        assert_eq!(targets.len(), 1, "existing jira_issue target wins");
        assert_eq!(targets[0].label.as_deref(), Some("Pre-existing"));
    }

    #[test]
    fn extract_scans_body_in_addition_to_title() {
        let mut e = event(ActivityKind::CommitAuthored, "sha1", "chore: bump deps");
        e.body = Some("Closes CAR-5117 per the release plan.".into());
        let mut events = vec![e];
        extract_ticket_keys(&mut events);
        assert!(events[0]
            .entities
            .iter()
            .any(|ent| ent.kind == EntityKind::JiraIssue && ent.external_id == "CAR-5117"));
    }

    #[test]
    fn jira_transition_annotated_with_mr_that_triggered_it() {
        // Plan invariant 7.
        let mr = {
            let mut e = event(ActivityKind::MrOpened, "!321", "CAR-5117: Rename commands");
            // Simulate what `extract_ticket_keys` already attached.
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mut events = vec![mr, jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events);
        let transition = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(transition.parent_external_id.as_deref(), Some("!321"));
    }

    #[test]
    fn annotate_transition_is_idempotent() {
        // Plan invariant 8.
        let mr = {
            let mut e = event(ActivityKind::MrOpened, "!321", "CAR-5117: Rename commands");
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mut events = vec![mr, jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events);
        let first = events.clone();
        annotate_transition_with_mr(&mut events);
        assert_eq!(events, first, "second call produces identical events");
    }

    #[test]
    fn annotate_no_op_when_mr_missing() {
        let mut events = vec![jira_transition("CAR-9999")];
        annotate_transition_with_mr(&mut events);
        assert_eq!(
            events[0].parent_external_id, None,
            "transition with no matching MR keeps its pre-existing parent (None)"
        );
    }

    /// DAY-88 / CORR-v0.2-05. Pre-fix, the winner was "first in the
    /// vec", which was walker-insertion dependent. Now it is
    /// "earliest `occurred_at`". This test vets the new rule by
    /// placing the earlier-in-time MR *second* in the vec — so
    /// any code that still relies on vec order would pick the wrong
    /// MR and fail the assertion.
    #[test]
    fn annotate_prefers_earliest_mr_by_occurred_at() {
        let later_in_time_but_first_in_vec = {
            let mut e = event(ActivityKind::MrOpened, "!100", "CAR-5117: later-in-time");
            e.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 14, 0, 0).unwrap();
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let earlier_in_time_but_second_in_vec = {
            let mut e = event(ActivityKind::MrMerged, "!200", "CAR-5117: earlier-in-time");
            e.occurred_at = Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap();
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mut events = vec![
            later_in_time_but_first_in_vec,
            earlier_in_time_but_second_in_vec,
            jira_transition("CAR-5117"),
        ];
        annotate_transition_with_mr(&mut events);
        let transition = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(
            transition.parent_external_id.as_deref(),
            Some("!200"),
            "the MR that occurred earlier in time must win even when it appears later in the input vec"
        );
    }

    /// DAY-88 / CORR-v0.2-05. When two MRs share an `occurred_at`,
    /// pairing falls through to `ActivityEvent::id` — which is a
    /// UUIDv5 from `(source_id, external_id, kind)`. Because that's
    /// content-addressable, the tie-break is reproducible across
    /// runs and across walker orderings.
    #[test]
    fn annotate_tie_breaks_mrs_with_same_occurred_at_by_deterministic_id() {
        let shared_time = Utc.with_ymd_and_hms(2026, 4, 20, 9, 0, 0).unwrap();
        let mr_a = {
            let mut e = event(ActivityKind::MrOpened, "!100", "CAR-5117: a");
            e.occurred_at = shared_time;
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mr_b = {
            let mut e = event(ActivityKind::MrOpened, "!200", "CAR-5117: b");
            e.occurred_at = shared_time;
            e.entities.push(EntityRef {
                kind: EntityKind::JiraIssue,
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        // The expected winner is whichever of !100 / !200 has the
        // smaller UUIDv5. Recompute deterministically rather than
        // hard-code, so a seed change in `event()`'s test helper
        // doesn't flip the assertion silently.
        let winning_id = if mr_a.id < mr_b.id { "!100" } else { "!200" };

        // Run once with one vec ordering ...
        let mut events_ab = vec![mr_a.clone(), mr_b.clone(), jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events_ab);
        let transition_ab = events_ab
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();

        // ... and again with the MRs swapped. Both must yield the
        // same winner because the tie-break is deterministic.
        let mut events_ba = vec![mr_b, mr_a, jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events_ba);
        let transition_ba = events_ba
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();

        assert_eq!(
            transition_ab.parent_external_id.as_deref(),
            Some(winning_id)
        );
        assert_eq!(
            transition_ba.parent_external_id.as_deref(),
            Some(winning_id)
        );
    }
}
