//! Cross-source `CommitAuthored` deduplication.
//!
//! Phase 3 is the first release with two producers of
//! [`ActivityKind::CommitAuthored`] rows:
//!
//! * [`connector-local-git`] walks the filesystem and emits one event
//!   per local commit SHA.
//! * [`connector-gitlab`] walks the Events API and emits a summary
//!   event anchored on the push tip SHA (Task 1); Task 2's follow-up
//!   enrichment expands a push into one event per commit SHA.
//!
//! When a user's work flows commit → push → MR, both producers emit a
//! `CommitAuthored` keyed on the same `external_id` (the commit SHA).
//! Without this pass the rendered draft shows each commit twice.
//!
//! ## Contract
//!
//! [`dedup_commit_authored`] is a pure function. Given a `Vec<ActivityEvent>`
//! that may contain zero-or-more `CommitAuthored` collisions, it returns
//! a `Vec<ActivityEvent>` where:
//!
//! 1. The set of `(kind, external_id)` pairs is preserved — no SHA
//!    invented, no SHA dropped (_set-like preservation_).
//! 2. For each colliding pair the *richer* row survives: longer `body`
//!    wins; on a tie, lex-smallest `source_id` wins (deterministic).
//! 3. The survivor's `links` and `entities` are the union of the
//!    colliding rows' `links` / `entities` (order-preserved,
//!    first-seen wins on duplicates).
//! 4. Privacy is monotone: if any colliding row is
//!    [`Privacy::RedactedPrivateRepo`], the survivor inherits it.
//! 5. Non-`CommitAuthored` events pass through untouched.
//!
//! The implementation is the only place in the engine that walks
//! cross-source event collisions; the downstream `rollup.rs` still
//! performs its own `(repo_path, date)` merge for *same-source* repo
//! collisions (DAY-52 regression guard).

use std::collections::BTreeMap;

use dayseam_core::{ActivityEvent, ActivityKind, EntityRef, Link, Privacy, SourceId};

/// Collapse cross-source `CommitAuthored` events that share a commit
/// SHA into a single canonical row.
///
/// See the module docs for the full contract. The function is `O(n)`
/// in the number of events plus `O(k log k)` in the number of unique
/// colliding SHAs `k` (the `BTreeMap` insertion cost). It never
/// allocates per-event scratch space beyond the output `Vec`.
///
/// The output preserves the *first-seen* position of each kept event
/// relative to the input. This matters because the downstream
/// [`crate::rollup`] treats the event vec as an ordered stream when
/// synthesising `CommitSet` artifacts; a set-semantic shuffle would
/// churn synthetic artifact ids between runs.
#[must_use]
pub fn dedup_commit_authored(events: Vec<ActivityEvent>) -> Vec<ActivityEvent> {
    // `by_sha` indexes into `merged` by SHA for the two-pass merge.
    // Using a `BTreeMap` over `HashMap` buys determinism without a
    // custom hasher and the input is already small (a day of one
    // person's commits rarely exceeds the low hundreds).
    let mut merged: Vec<Option<ActivityEvent>> = Vec::with_capacity(events.len());
    let mut by_sha: BTreeMap<String, usize> = BTreeMap::new();

    for event in events {
        if event.kind != ActivityKind::CommitAuthored {
            merged.push(Some(event));
            continue;
        }

        let sha = event.external_id.clone();
        match by_sha.get(&sha).copied() {
            None => {
                by_sha.insert(sha, merged.len());
                merged.push(Some(event));
            }
            Some(idx) => {
                // Safe: `idx` came from a prior iteration of this
                // loop that pushed `Some`, and no other write path
                // can flip that slot to `None`.
                let existing = merged[idx]
                    .take()
                    .expect("by_sha index always points to Some");
                let winner = merge_two(existing, event);
                merged[idx] = Some(winner);
            }
        }
    }

    merged.into_iter().flatten().collect()
}

/// Merge two `CommitAuthored` events that share a SHA.
///
/// The survivor is picked by:
/// 1. Body length (longest wins — proxy for "which connector enriched
///    this commit the most").
/// 2. On tie, lex-smallest `source_id` (deterministic across runs —
///    if both sides carry the same body the choice has to be stable
///    so rerunning the same day does not churn the canonical id).
///
/// After the survivor is picked, `links` and `entities` are unioned
/// across both inputs (order-preserved, first-seen wins). Privacy
/// monotonically upgrades to `RedactedPrivateRepo` if either side
/// carries it — the design §10.3 contract is "the louder side wins",
/// where "louder" means "more restrictive".
fn merge_two(a: ActivityEvent, b: ActivityEvent) -> ActivityEvent {
    let (mut winner, loser) = if body_rank(&a) >= body_rank(&b) && pick_a_on_tie(&a, &b) {
        (a, b)
    } else {
        (b, a)
    };

    winner.links = union_links(winner.links, loser.links);
    winner.entities = union_entities(winner.entities, loser.entities);
    winner.privacy = louder_privacy(winner.privacy, loser.privacy);

    winner
}

fn body_rank(e: &ActivityEvent) -> usize {
    e.body.as_deref().map_or(0, str::len)
}

/// Returns `true` iff `a` should win a body-length tie. The choice is
/// "lex-smallest `source_id`", so the canonical survivor of any
/// colliding pair is deterministic regardless of input ordering.
fn pick_a_on_tie(a: &ActivityEvent, b: &ActivityEvent) -> bool {
    if body_rank(a) != body_rank(b) {
        return true;
    }
    source_key(a.source_id) <= source_key(b.source_id)
}

fn source_key(s: SourceId) -> [u8; 16] {
    *s.as_bytes()
}

fn union_links(mut a: Vec<Link>, b: Vec<Link>) -> Vec<Link> {
    for link in b {
        if !a.iter().any(|existing| existing == &link) {
            a.push(link);
        }
    }
    a
}

fn union_entities(mut a: Vec<EntityRef>, b: Vec<EntityRef>) -> Vec<EntityRef> {
    for ent in b {
        if !a.iter().any(|existing| existing == &ent) {
            a.push(ent);
        }
    }
    a
}

const fn louder_privacy(a: Privacy, b: Privacy) -> Privacy {
    match (a, b) {
        (Privacy::RedactedPrivateRepo, _) | (_, Privacy::RedactedPrivateRepo) => {
            Privacy::RedactedPrivateRepo
        }
        _ => Privacy::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{Actor, RawRef};
    use proptest::prelude::*;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn src(n: u128) -> SourceId {
        Uuid::from_u128(n)
    }

    fn event(
        source: SourceId,
        sha: &str,
        kind: ActivityKind,
        body: Option<&str>,
        privacy: Privacy,
    ) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                format!("{source}::{sha}::{kind:?}").as_bytes(),
            ),
            source_id: source,
            external_id: sha.into(),
            kind,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Test".into(),
                email: Some("test@example.com".into()),
                external_id: None,
            },
            title: format!("commit {sha}"),
            body: body.map(str::to_string),
            links: Vec::new(),
            entities: Vec::new(),
            parent_external_id: None,
            metadata: serde_json::Value::Null,
            raw_ref: RawRef {
                storage_key: "k".into(),
                content_type: "application/x-git".into(),
            },
            privacy,
        }
    }

    fn commit(source: SourceId, sha: &str) -> ActivityEvent {
        event(
            source,
            sha,
            ActivityKind::CommitAuthored,
            None,
            Privacy::Normal,
        )
    }

    /// Non-`CommitAuthored` events pass through untouched and in
    /// order — dedup is scoped to the one kind that has two producers.
    #[test]
    fn non_commit_authored_events_pass_through() {
        let s = src(1);
        let mr = event(
            s,
            "!11",
            ActivityKind::MrOpened,
            Some("open"),
            Privacy::Normal,
        );
        let issue = event(
            s,
            "#22",
            ActivityKind::IssueOpened,
            Some("open"),
            Privacy::Normal,
        );
        let out = dedup_commit_authored(vec![mr.clone(), issue.clone()]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].external_id, "!11");
        assert_eq!(out[1].external_id, "#22");
    }

    /// Two sources emitting the same SHA collapse to one event.
    #[test]
    fn two_sources_same_sha_collapses_to_one() {
        let a = src(1);
        let b = src(2);
        let out = dedup_commit_authored(vec![commit(a, "sha1"), commit(b, "sha1")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].external_id, "sha1");
    }

    /// The richer body wins — the plan §10.3 rule of "keep the row
    /// whose body is longer" drives the survivor choice.
    #[test]
    fn dedup_picks_richer_body() {
        let a = src(1);
        let b = src(2);
        let short = event(
            a,
            "sha1",
            ActivityKind::CommitAuthored,
            Some("short"),
            Privacy::Normal,
        );
        let rich = event(
            b,
            "sha1",
            ActivityKind::CommitAuthored,
            Some("a far more detailed commit message"),
            Privacy::Normal,
        );
        let out = dedup_commit_authored(vec![short, rich.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].body.as_deref(), rich.body.as_deref());
        assert_eq!(out[0].source_id, b);
    }

    /// Body-length ties are broken by lex-smallest `source_id`, so
    /// two runs of the same input on two different orderings pick
    /// the same winner.
    #[test]
    fn dedup_picks_richer_body_with_lex_tiebreak() {
        let a = src(0x0000_0000_0000_0000_0000_0000_0000_00AA);
        let b = src(0x0000_0000_0000_0000_0000_0000_0000_00BB);
        let e_a = event(
            a,
            "sha1",
            ActivityKind::CommitAuthored,
            Some("same"),
            Privacy::Normal,
        );
        let e_b = event(
            b,
            "sha1",
            ActivityKind::CommitAuthored,
            Some("same"),
            Privacy::Normal,
        );

        let ab = dedup_commit_authored(vec![e_a.clone(), e_b.clone()]);
        let ba = dedup_commit_authored(vec![e_b, e_a]);
        assert_eq!(ab.len(), 1);
        assert_eq!(ba.len(), 1);
        assert_eq!(ab[0].source_id, a, "lex-smallest wins");
        assert_eq!(ba[0].source_id, a, "order-independent tiebreak");
    }

    /// Links and entities union across colliding rows.
    #[test]
    fn dedup_unions_links_and_entities() {
        let a = src(1);
        let b = src(2);
        let mut e_a = commit(a, "sha1");
        e_a.links = vec![Link {
            url: "https://git/a".into(),
            label: Some("local".into()),
        }];
        e_a.entities = vec![EntityRef {
            kind: "repo".into(),
            external_id: "/work/foo".into(),
            label: None,
        }];
        let mut e_b = commit(b, "sha1");
        e_b.body = Some("richer body".into());
        e_b.links = vec![Link {
            url: "https://gitlab/commit/sha1".into(),
            label: Some("gitlab".into()),
        }];
        e_b.entities = vec![EntityRef {
            kind: "project".into(),
            external_id: "42".into(),
            label: Some("payments".into()),
        }];

        let out = dedup_commit_authored(vec![e_a.clone(), e_b.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_id, b, "richer body wins");
        assert_eq!(out[0].links.len(), 2, "links unioned");
        assert!(out[0].links.iter().any(|l| l.url == "https://git/a"));
        assert!(out[0]
            .links
            .iter()
            .any(|l| l.url == "https://gitlab/commit/sha1"));
        assert_eq!(out[0].entities.len(), 2, "entities unioned");
    }

    /// Duplicated links/entities across inputs are de-duplicated —
    /// union, not concat.
    #[test]
    fn union_is_not_concat_for_duplicate_links() {
        let a = src(1);
        let b = src(2);
        let link = Link {
            url: "https://example/commit/sha1".into(),
            label: None,
        };
        let mut e_a = commit(a, "sha1");
        e_a.links = vec![link.clone()];
        let mut e_b = commit(b, "sha1");
        e_b.links = vec![link.clone()];

        let out = dedup_commit_authored(vec![e_a, e_b]);
        assert_eq!(out[0].links.len(), 1);
    }

    /// Plan invariant 5: if either side carries
    /// `RedactedPrivateRepo`, the survivor inherits it.
    #[test]
    fn dedup_respects_redacted_private_repo() {
        let a = src(1);
        let b = src(2);
        let local_private = event(
            a,
            "sha1",
            ActivityKind::CommitAuthored,
            Some("a"),
            Privacy::RedactedPrivateRepo,
        );
        let gitlab_normal = event(
            b,
            "sha1",
            ActivityKind::CommitAuthored,
            Some("a longer body"),
            Privacy::Normal,
        );

        let out = dedup_commit_authored(vec![local_private, gitlab_normal]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].privacy, Privacy::RedactedPrivateRepo);
    }

    /// Idempotent: `dedup(dedup(x)) == dedup(x)` for any input.
    #[test]
    fn dedup_is_idempotent() {
        let a = src(1);
        let b = src(2);
        let input = vec![commit(a, "sha1"), commit(b, "sha1"), commit(a, "sha2")];
        let once = dedup_commit_authored(input.clone());
        let twice = dedup_commit_authored(once.clone());
        assert_eq!(once, twice);
    }

    proptest! {
        /// Plan invariant 2: dedup never invents or loses a SHA.
        #[test]
        fn dedup_preserves_the_sha_set(shas in proptest::collection::vec("[a-z0-9]{6}", 0..20), which in proptest::collection::vec(any::<bool>(), 1..4)) {
            let a = src(1);
            let b = src(2);
            let events: Vec<ActivityEvent> = shas
                .iter()
                .zip(which.iter().cycle())
                .map(|(sha, b_side)| commit(if *b_side { b } else { a }, sha))
                .collect();
            let expected: BTreeSet<String> = shas.iter().cloned().collect();

            let out = dedup_commit_authored(events);
            let got: BTreeSet<String> = out
                .iter()
                .filter(|e| e.kind == ActivityKind::CommitAuthored)
                .map(|e| e.external_id.clone())
                .collect();
            prop_assert_eq!(got, expected);
        }

        /// Plan invariant 1: dedup is set-like — any permutation of the
        /// input yields the same canonical survivor set.
        #[test]
        fn dedup_is_order_independent_on_the_kept_set(
            shas in proptest::collection::vec("[a-z0-9]{6}", 0..12),
        ) {
            let a = src(0x000000000000000000000000000000AA);
            let b = src(0x000000000000000000000000000000BB);
            let forward: Vec<ActivityEvent> = shas
                .iter()
                .enumerate()
                .flat_map(|(i, sha)| {
                    // Emit both sides for even indices so half the
                    // input has collisions to collapse.
                    if i % 2 == 0 {
                        vec![commit(a, sha), commit(b, sha)]
                    } else {
                        vec![commit(a, sha)]
                    }
                })
                .collect();
            let mut reverse = forward.clone();
            reverse.reverse();

            let out_f = dedup_commit_authored(forward);
            let out_r = dedup_commit_authored(reverse);
            // Canonical survivors are keyed by SHA; compare the
            // (sha, source_id) pair sets.
            let as_set = |v: Vec<ActivityEvent>| -> BTreeSet<(String, [u8; 16])> {
                v.into_iter()
                    .filter(|e| e.kind == ActivityKind::CommitAuthored)
                    .map(|e| (e.external_id, *e.source_id.as_bytes()))
                    .collect()
            };
            prop_assert_eq!(as_set(out_f), as_set(out_r));
        }
    }
}
