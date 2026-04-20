//! Plan Task 2 golden test for `annotate_rolled_into_mr`.
//!
//! Exercises the helper end-to-end: two pushes land on one MR and a
//! third push is unmerged, the helper stamps the MR iid on the two
//! rolled-up commits and leaves the third at `None`.

mod common;

use common::commit_event;
use dayseam_core::Privacy;
use dayseam_report::{annotate_rolled_into_mr, MergeRequestArtifact};

#[test]
fn two_pushes_on_one_mr_and_one_outside() {
    let src = common::source_id(7);
    let mut events = vec![
        commit_event(
            src,
            "sha_in_mr_a",
            "/work/dayseam",
            "self@example.com",
            9,
            "On the MR: add DSPy tag",
            Privacy::Normal,
        ),
        commit_event(
            src,
            "sha_in_mr_b",
            "/work/dayseam",
            "self@example.com",
            10,
            "On the MR: refactor loader",
            Privacy::Normal,
        ),
        commit_event(
            src,
            "sha_outside",
            "/work/dayseam",
            "self@example.com",
            11,
            "Unrelated commit on main",
            Privacy::Normal,
        ),
    ];

    let mrs = vec![MergeRequestArtifact {
        external_id: "!42".into(),
        commit_shas: vec!["sha_in_mr_a".into(), "sha_in_mr_b".into()],
    }];

    annotate_rolled_into_mr(&mut events, &mrs);

    assert_eq!(events[0].parent_external_id.as_deref(), Some("!42"));
    assert_eq!(events[1].parent_external_id.as_deref(), Some("!42"));
    assert_eq!(
        events[2].parent_external_id, None,
        "a push not on any MR keeps parent_external_id = None"
    );
}

/// The helper must be safe to run after the dedup pass: if dedup
/// collapsed two sources' SHA into one, the survivor's
/// `parent_external_id` is still blank (because dedup chose a
/// non-parent row to keep) and the MR rollup fills it in.
#[test]
fn annotation_follows_dedup_canonical_survivor() {
    use dayseam_report::dedup_commit_authored;

    let src_a = common::source_id(1);
    let src_b = common::source_id(2);
    let e_a = commit_event(
        src_a,
        "sha1",
        "/work/dayseam",
        "self@example.com",
        9,
        "local-git",
        Privacy::Normal,
    );
    let mut e_b = commit_event(
        src_b,
        "sha1",
        "/work/dayseam",
        "self@example.com",
        9,
        "gitlab with a far richer body wins",
        Privacy::Normal,
    );
    // The GitLab-side row carries a longer body so dedup picks it.
    e_b.body = Some("long upstream body".into());

    let mut deduped = dedup_commit_authored(vec![e_a, e_b]);
    assert_eq!(deduped.len(), 1);

    annotate_rolled_into_mr(
        &mut deduped,
        &[MergeRequestArtifact {
            external_id: "!42".into(),
            commit_shas: vec!["sha1".into()],
        }],
    );

    assert_eq!(deduped[0].parent_external_id.as_deref(), Some("!42"));
    assert_eq!(deduped[0].source_id, src_b, "richer body wins");
}
