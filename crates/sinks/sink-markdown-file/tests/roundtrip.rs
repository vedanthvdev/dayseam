//! Integration tests covering the eight invariants the
//! `sink-markdown-file` crate documents in its plan entry (Phase 2,
//! Task 4):
//!
//! 1. Atomic write — a partial temp file never replaces the target.
//! 2. Marker-block preservation — writing for date D into a file that
//!    already has blocks for other dates appends, leaves prose between
//!    blocks byte-identical.
//! 3. Marker-block replacement — rewriting the same date replaces only
//!    its block; adjacent blocks survive.
//! 4. Marker-block shape — malformed blocks surface
//!    `SINK_MALFORMED_MARKER` without clobbering the file.
//! 5. Two destinations, one consistent result — a failing secondary
//!    destination never rolls back a successful primary.
//! 6. Obsidian-friendly filename — `Dayseam <YYYY-MM-DD>.md`.
//! 7. Concurrent-write refusal — a second writer observes the lock
//!    sentinel and returns `SINK_FS_CONCURRENT_WRITE`.
//! 8. Capability declaration — `SinkCapabilities::LOCAL_ONLY`.
//!
//! Each test is self-contained (tempdir per test, no global state) and
//! runs in a single-threaded runtime; the concurrent-write test uses
//! a hand-rolled thread pair rather than tokio tasks so it exercises
//! the lock sentinel across OS-level handles.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use chrono::{NaiveDate, TimeZone, Utc};
use dayseam_core::{
    error_codes, DayseamError, RenderedBullet, RenderedSection, ReportDraft, SinkCapabilities,
    SinkConfig, SinkKind,
};
use dayseam_events::{RunId, RunStreams};
use sink_markdown_file::MarkdownFileSink;
use sinks_sdk::{SinkAdapter, SinkCtx};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn draft_for(date: NaiveDate, bullets: &[&str]) -> ReportDraft {
    ReportDraft {
        id: Uuid::new_v4(),
        date,
        template_id: "dayseam.dev_eod".into(),
        template_version: "2026-04-18".into(),
        sections: vec![RenderedSection {
            id: "commits".into(),
            title: "Commits".into(),
            bullets: bullets
                .iter()
                .enumerate()
                .map(|(i, t)| RenderedBullet {
                    id: format!("b{i}"),
                    text: (*t).to_string(),
                    // Uniform kind across this test's synthetic
                    // bullets — the sink's grouping behaviour is
                    // already covered by its own unit tests; the
                    // roundtrip's concern is marker-block stability,
                    // and a single kind keeps the assertions focused.
                    source_kind: Some(dayseam_core::SourceKind::LocalGit),
                })
                .collect(),
        }],
        evidence: Vec::new(),
        per_source_state: HashMap::new(),
        verbose_mode: false,
        generated_at: Utc.with_ymd_and_hms(2026, 4, 18, 22, 15, 9).unwrap(),
    }
}

fn cfg_for(dirs: Vec<PathBuf>, frontmatter: bool) -> SinkConfig {
    SinkConfig::MarkdownFile {
        config_version: 1,
        dest_dirs: dirs,
        frontmatter,
    }
}

fn ctx_from(streams: &RunStreams, cancel: CancellationToken) -> SinkCtx {
    SinkCtx::new(
        Some(streams.run_id),
        streams.progress_tx.clone(),
        streams.log_tx.clone(),
        cancel,
    )
}

fn target_in(dir: &Path, date: NaiveDate) -> PathBuf {
    dir.join(format!("Dayseam {date}.md"))
}

// -- Invariant #1: atomic write -----------------------------------------------

#[tokio::test]
async fn crash_midwrite_leaves_target_intact() {
    // We simulate "crash mid-write" by creating an orphan `.dayseam.tmp`
    // sibling by hand and asserting that the sink's next init sweep
    // removes it, while the target (which we write first) stays
    // byte-identical. Real SIGKILLs in-process aren't available to
    // Rust's cooperative runtime; the sweep semantics are what the
    // invariant actually protects.
    let dir = tempfile::tempdir().unwrap();
    let target = target_in(dir.path(), NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());
    fs::write(&target, b"pre-crash user content\n").unwrap();

    let orphan = dir
        .path()
        .join(".Dayseam 2026-04-18.md.12345678.dayseam.tmp");
    fs::write(&orphan, b"half-written temp").unwrap();
    // Back-date the orphan far enough that the sweep (5 min threshold)
    // will delete it on init.
    fs::OpenOptions::new()
        .write(true)
        .open(&orphan)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();

    let _sink = MarkdownFileSink::new(&[dir.path().to_path_buf()]);

    assert!(
        !orphan.exists(),
        "orphan temp must be swept on sink init (Invariant #1)"
    );
    assert_eq!(
        fs::read(&target).unwrap(),
        b"pre-crash user content\n",
        "target must stay byte-identical; the sweep must not mutate it"
    );
}

// -- Invariant #2: marker-block preservation ---------------------------------

#[tokio::test]
async fn marker_block_preserves_surrounding_user_prose() {
    let dir = tempfile::tempdir().unwrap();
    let target = target_in(dir.path(), NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());

    let existing = concat!(
        "# My journal\n",
        "\n",
        "-- personal note before dayseam blocks --\n",
        "\n",
        "<!-- dayseam:begin date=\"2026-04-15\" run_id=\"old-r3\" template=\"dayseam.dev_eod\" version=\"2026-04-18\" -->\n",
        "- D-3 stuff\n",
        "<!-- dayseam:end -->\n",
        "\n",
        "-- prose between blocks --\n",
        "\n",
        "<!-- dayseam:begin date=\"2026-04-17\" run_id=\"old-r1\" template=\"dayseam.dev_eod\" version=\"2026-04-18\" -->\n",
        "- D-1 stuff\n",
        "<!-- dayseam:end -->\n",
        "\n",
        "-- trailing prose --\n",
    );
    fs::write(&target, existing).unwrap();

    let sink = MarkdownFileSink::new(&[dir.path().to_path_buf()]);
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from(&streams, CancellationToken::new());
    let cfg = cfg_for(vec![dir.path().to_path_buf()], false);
    let draft = draft_for(
        NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(),
        &["new D commit bullet"],
    );

    sink.write(&ctx, &cfg, &draft)
        .await
        .expect("write succeeds");

    let out = fs::read_to_string(&target).unwrap();
    assert!(
        out.contains("# My journal"),
        "top-of-file prose must survive"
    );
    assert!(
        out.contains("-- personal note before dayseam blocks --"),
        "pre-block prose must survive byte-for-byte"
    );
    assert!(
        out.contains("-- prose between blocks --"),
        "inter-block prose must survive byte-for-byte"
    );
    assert!(
        out.contains("-- trailing prose --"),
        "trailing prose must survive byte-for-byte"
    );
    assert!(out.contains("- D-3 stuff"), "D-3 block must be untouched");
    assert!(out.contains("- D-1 stuff"), "D-1 block must be untouched");
    assert!(
        out.contains("- new D commit bullet"),
        "new D block must be appended"
    );
}

// -- Invariant #3: marker-block replacement ----------------------------------

#[tokio::test]
async fn rewriting_same_date_replaces_only_its_block() {
    // Seed the per-date file directly with two adjacent blocks (what
    // the user would see if they had renamed an older Dayseam file
    // to collect several days, or manually concatenated two days of
    // output). The sink must replace only the D block and leave the
    // D-1 block byte-identical.
    let dir = tempfile::tempdir().unwrap();
    let target = target_in(dir.path(), NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());
    let seeded = concat!(
        "<!-- dayseam:begin date=\"2026-04-17\" run_id=\"r-yesterday\" template=\"dayseam.dev_eod\" version=\"2026-04-18\" -->\n",
        "## Commits\n\n",
        "- yesterday bullet\n",
        "<!-- dayseam:end -->\n",
        "\n",
        "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r-today-v1\" template=\"dayseam.dev_eod\" version=\"2026-04-18\" -->\n",
        "## Commits\n\n",
        "- old today bullet\n",
        "<!-- dayseam:end -->\n",
    );
    fs::write(&target, seeded).unwrap();

    let sink = MarkdownFileSink::new(&[dir.path().to_path_buf()]);
    let cfg = cfg_for(vec![dir.path().to_path_buf()], false);
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from(&streams, CancellationToken::new());

    sink.write(
        &ctx,
        &cfg,
        &draft_for(
            NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(),
            &["new today bullet"],
        ),
    )
    .await
    .unwrap();

    let after = fs::read_to_string(&target).unwrap();
    assert!(
        after.contains("- yesterday bullet"),
        "adjacent day's block must be preserved: {after}"
    );
    assert!(
        !after.contains("- old today bullet"),
        "old today block must be replaced: {after}"
    );
    assert!(
        after.contains("- new today bullet"),
        "new today block must be spliced in: {after}"
    );
    // The adjacent block's run_id must be preserved — we only touched
    // the 2026-04-18 block, so `r-yesterday` must survive verbatim.
    assert!(
        after.contains("run_id=\"r-yesterday\""),
        "adjacent block attributes must be byte-identical: {after}"
    );
}

// -- Invariant #4: marker-block shape (malformed) ----------------------------

#[tokio::test]
async fn malformed_marker_is_rejected_without_file_change() {
    let dir = tempfile::tempdir().unwrap();
    let target = target_in(dir.path(), NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());

    // Dangling end — a broken block the sink must refuse to touch.
    let broken = "prose\n<!-- dayseam:end -->\nmore prose\n";
    fs::write(&target, broken).unwrap();

    let sink = MarkdownFileSink::new(&[dir.path().to_path_buf()]);
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from(&streams, CancellationToken::new());
    let cfg = cfg_for(vec![dir.path().to_path_buf()], false);
    let draft = draft_for(NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(), &["new"]);

    let err = sink
        .write(&ctx, &cfg, &draft)
        .await
        .expect_err("malformed marker must be rejected");
    assert!(
        matches!(&err, DayseamError::Internal { code, .. } if code == error_codes::SINK_MALFORMED_MARKER),
        "unexpected error variant: {err:?}"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        broken,
        "target file must be byte-identical when marker is malformed"
    );
}

// -- Invariant #5: partial-success when one destination fails ----------------

#[tokio::test]
async fn second_destination_failure_does_not_rollback_first() {
    let ok_dir = tempfile::tempdir().unwrap();
    // Second "destination" is a file, not a directory — the validate
    // step would reject it, but `write()` is also defensive: the
    // atomic-write step fails (can't open temp in a non-directory)
    // and the sink must not roll back the already-persisted first
    // destination.
    let bogus = ok_dir.path().join("not-a-directory");
    fs::write(&bogus, b"i am a file").unwrap();

    let sink = MarkdownFileSink::new(&[ok_dir.path().to_path_buf()]);
    let cfg = cfg_for(vec![ok_dir.path().to_path_buf(), bogus.clone()], false);
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from(&streams, CancellationToken::new());
    let draft = draft_for(NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(), &["commit"]);

    let receipt = sink
        .write(&ctx, &cfg, &draft)
        .await
        .expect("first destination succeeds so the adapter returns Ok");

    assert_eq!(
        receipt.destinations_written.len(),
        1,
        "receipt must reflect only the successful destination"
    );
    let ok_path = target_in(ok_dir.path(), NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());
    assert!(ok_path.exists(), "first destination must stay committed");
    assert!(
        fs::read_to_string(&ok_path).unwrap().contains("- commit"),
        "first destination content must be the newly written draft"
    );
    assert_eq!(
        fs::read(&bogus).unwrap(),
        b"i am a file",
        "bogus destination's bytes must not be overwritten"
    );
}

// -- Invariant #6: Obsidian-friendly default filename ------------------------

#[tokio::test]
async fn default_filename_matches_obsidian_convention() {
    let dir = tempfile::tempdir().unwrap();
    let sink = MarkdownFileSink::new(&[dir.path().to_path_buf()]);
    let cfg = cfg_for(vec![dir.path().to_path_buf()], false);
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from(&streams, CancellationToken::new());
    let draft = draft_for(NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(), &["x"]);

    sink.write(&ctx, &cfg, &draft).await.unwrap();

    let expected = dir.path().join("Dayseam 2026-04-18.md");
    assert!(
        expected.exists(),
        "expected `Dayseam <YYYY-MM-DD>.md`; got entries: {:?}",
        fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect::<Vec<_>>()
    );
}

// -- Invariant #7: concurrent-write refusal ----------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_write_on_same_file_rejects_second_caller() {
    // Strategy: acquire the adapter's lock by hand (via filesystem
    // sentinel creation), then call `write()` and observe that it
    // returns `SINK_FS_CONCURRENT_WRITE`. Doing it this way avoids a
    // brittle timing dependency between two concurrent `write()`
    // futures while still exercising the exact lock path the adapter
    // uses.
    let dir = tempfile::tempdir().unwrap();
    let target = target_in(dir.path(), NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());

    let barrier = Arc::new(Barrier::new(2));
    let lock_path = {
        let mut s = target.as_os_str().to_owned();
        s.push(".dayseam.lock");
        PathBuf::from(s)
    };

    // Hold the sentinel on a blocking thread for the duration of the
    // second caller's attempt.
    let holder_lock = lock_path.clone();
    let holder_barrier = barrier.clone();
    let holder = thread::spawn(move || {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&holder_lock)
            .expect("hand-held sentinel creation must succeed");
        holder_barrier.wait();
        // Wait for the main thread to finish its write attempt before
        // releasing the lock.
        thread::sleep(std::time::Duration::from_millis(100));
        fs::remove_file(&holder_lock).ok();
    });

    barrier.wait();

    let sink = MarkdownFileSink::new(&[dir.path().to_path_buf()]);
    let cfg = cfg_for(vec![dir.path().to_path_buf()], false);
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from(&streams, CancellationToken::new());
    let draft = draft_for(NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(), &["x"]);

    let err = sink
        .write(&ctx, &cfg, &draft)
        .await
        .expect_err("second caller must refuse to write while sentinel is held");
    assert!(
        matches!(&err, DayseamError::Io { code, .. } if code == error_codes::SINK_FS_CONCURRENT_WRITE),
        "unexpected error variant: {err:?}"
    );
    assert!(
        !target.exists(),
        "target must not be written while the sentinel is held"
    );

    holder.join().unwrap();
}

// -- Invariant #8: capability declaration ------------------------------------

#[tokio::test]
async fn capabilities_are_local_only_and_unattended_safe() {
    let sink = MarkdownFileSink::default();
    assert_eq!(sink.kind(), SinkKind::MarkdownFile);
    let caps = sink.capabilities();
    assert_eq!(caps, SinkCapabilities::LOCAL_ONLY);
    assert!(caps.local_only);
    assert!(!caps.remote_write);
    assert!(!caps.interactive_only);
    assert!(caps.safe_for_unattended);
    caps.validate().expect("local-only caps must validate");
}

// -- Frontmatter merge: generated_at is the only field the sink rewrites -----

#[tokio::test]
async fn frontmatter_merge_updates_generated_at_only() {
    let dir = tempfile::tempdir().unwrap();
    let target = target_in(dir.path(), NaiveDate::from_ymd_opt(2026, 4, 18).unwrap());
    let existing = concat!(
        "---\n",
        "date: 2026-04-18\n",
        "template: dayseam.dev_eod\n",
        "template_version: 2026-04-18\n",
        "generated_at: 2020-01-01T00:00:00Z\n",
        "tags: [daily, work]\n",
        "aliases:\n",
        "  - eod\n",
        "---\n",
        "# Hand-written header preserved\n",
    );
    fs::write(&target, existing).unwrap();

    let sink = MarkdownFileSink::new(&[dir.path().to_path_buf()]);
    let cfg = cfg_for(vec![dir.path().to_path_buf()], true);
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from(&streams, CancellationToken::new());
    let draft = draft_for(NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(), &["commit"]);

    sink.write(&ctx, &cfg, &draft).await.unwrap();

    let after = fs::read_to_string(&target).unwrap();
    assert!(
        !after.contains("2020-01-01"),
        "generated_at must be refreshed on every save"
    );
    assert!(
        after.contains("generated_at: 2026-04-18T22:15:09Z"),
        "generated_at must reflect the draft's timestamp, got: {after}"
    );
    assert!(
        after.contains("tags: [daily, work]"),
        "hand-authored tag list must survive"
    );
    assert!(
        after.contains("aliases:") && after.contains("  - eod"),
        "hand-authored alias list (a multi-line scalar) must survive"
    );
    assert!(
        after.contains("# Hand-written header preserved"),
        "body prose above the marker block must survive"
    );
}
