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

use dayseam_core::{ActivityEvent, ActivityKind, EntityRef};

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
                .any(|e| e.kind == "jira_issue" && e.external_id == key);
            if !already {
                event.entities.push(EntityRef {
                    kind: "jira_issue".into(),
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
/// `CAR-5117` then finds that MR via the `HashMap<issue_key,
/// mr_external_id>` built here and stamps
/// `parent_external_id = Some(<mr_external_id>)`.
///
/// First-MR-wins: when two MRs both reference the same Jira issue,
/// the first one in the input order claims the transition. Input
/// order is already deterministic at the point this helper runs
/// (dedup + stable connector emission), so "first" is reproducible.
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
            .find(|e| e.kind == "jira_issue")
            .map(|e| e.external_id.clone())
        else {
            continue;
        };
        if let Some(mr_id) = issue_to_mr.get(issue_key.as_str()) {
            event.parent_external_id = Some(mr_id.clone());
        }
    }
}

/// Owned-strings index so the caller can freely `iter_mut()` the
/// event vec after we return. Using `&str` references ties the
/// index lifetime to the borrow of `events`, which conflicts with
/// the subsequent mutable walk.
fn build_issue_to_mr_index(events: &[ActivityEvent]) -> HashMap<String, String> {
    let mut index: HashMap<String, String> = HashMap::new();
    for event in events {
        if !matches!(event.kind, ActivityKind::MrOpened | ActivityKind::MrMerged) {
            continue;
        }
        for ent in &event.entities {
            if ent.kind != "jira_issue" {
                continue;
            }
            // `entry().or_insert` preserves first-MR-wins semantics.
            index
                .entry(ent.external_id.clone())
                .or_insert_with(|| event.external_id.clone());
        }
    }
    index
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
    use dayseam_core::{Actor, EntityRef, Privacy, RawRef, SourceId};
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
            kind: "jira_issue".into(),
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
            .filter(|e| e.kind == "jira_issue")
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
            .filter(|e| e.kind == "jira_issue")
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
            kind: "jira_issue".into(),
            external_id: "CAR-5117".into(),
            label: Some("Pre-existing".into()),
        });
        let mut events = vec![e];
        extract_ticket_keys(&mut events);
        let targets: Vec<&EntityRef> = events[0]
            .entities
            .iter()
            .filter(|e| e.kind == "jira_issue")
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
            .any(|ent| ent.kind == "jira_issue" && ent.external_id == "CAR-5117"));
    }

    #[test]
    fn jira_transition_annotated_with_mr_that_triggered_it() {
        // Plan invariant 7.
        let mr = {
            let mut e = event(ActivityKind::MrOpened, "!321", "CAR-5117: Rename commands");
            // Simulate what `extract_ticket_keys` already attached.
            e.entities.push(EntityRef {
                kind: "jira_issue".into(),
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
                kind: "jira_issue".into(),
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

    #[test]
    fn annotate_respects_first_mr_wins() {
        let mr_a = {
            let mut e = event(ActivityKind::MrOpened, "!100", "CAR-5117: first");
            e.entities.push(EntityRef {
                kind: "jira_issue".into(),
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mr_b = {
            let mut e = event(ActivityKind::MrMerged, "!200", "CAR-5117: second");
            e.entities.push(EntityRef {
                kind: "jira_issue".into(),
                external_id: "CAR-5117".into(),
                label: None,
            });
            e
        };
        let mut events = vec![mr_a, mr_b, jira_transition("CAR-5117")];
        annotate_transition_with_mr(&mut events);
        let transition = events
            .iter()
            .find(|e| e.kind == ActivityKind::JiraIssueTransitioned)
            .unwrap();
        assert_eq!(
            transition.parent_external_id.as_deref(),
            Some("!100"),
            "first MR in input order wins"
        );
    }
}
