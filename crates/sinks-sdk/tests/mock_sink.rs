//! Integration tests for [`MockSink`] — the hermetic reference sink
//! downstream crates will depend on in their own test suites.
//!
//! These tests lock in the behaviour contract: recorded writes match the
//! config, progress events fire in order, cancellation is honoured, and
//! the armed `fail_next_with` path surfaces the requested error without
//! breaking subsequent calls.

use std::path::PathBuf;

use chrono::Utc;
use dayseam_core::{ProgressPhase, ReportDraft, SinkConfig, SinkKind};
use dayseam_events::{RunId, RunStreams};
use sinks_sdk::{MockSink, SinkAdapter, SinkCapabilities, SinkCtx};
use tokio_util::sync::CancellationToken;

fn draft() -> ReportDraft {
    ReportDraft {
        id: uuid::Uuid::new_v4(),
        date: chrono::NaiveDate::from_ymd_opt(2026, 4, 17).unwrap(),
        template_id: "dev-eod".into(),
        template_version: "1".into(),
        sections: vec![],
        evidence: vec![],
        per_source_state: Default::default(),
        verbose_mode: false,
        generated_at: Utc::now(),
    }
}

fn markdown_cfg(path: &str) -> SinkConfig {
    SinkConfig::MarkdownFile {
        config_version: 1,
        dest_dirs: vec![PathBuf::from(path)],
        frontmatter: true,
    }
}

/// Build a `SinkCtx` that shares the run id and senders with `streams`
/// without taking ownership — the test keeps `streams` alive so it can
/// drain the receivers at the end.
fn ctx_from_streams(streams: &RunStreams, cancel: CancellationToken) -> SinkCtx {
    SinkCtx::new(
        Some(streams.run_id),
        streams.progress_tx.clone(),
        streams.log_tx.clone(),
        cancel,
    )
}

#[tokio::test]
async fn mock_sink_records_writes_and_advertises_canonical_local_capabilities() {
    let sink = MockSink::new();
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from_streams(&streams, CancellationToken::new());

    assert_eq!(sink.kind(), SinkKind::MarkdownFile);
    let caps = sink.capabilities();
    assert_eq!(caps, SinkCapabilities::LOCAL_ONLY);
    caps.validate().expect("canonical local caps validate");

    let cfg = markdown_cfg("/tmp/dayseam-mock-a");
    sink.validate(&ctx, &cfg).await.expect("validate ok");

    let d = draft();
    let receipt = sink.write(&ctx, &cfg, &d).await.expect("write ok");

    assert_eq!(receipt.sink_kind, SinkKind::MarkdownFile);
    assert_eq!(
        receipt.destinations_written,
        vec![PathBuf::from("/tmp/dayseam-mock-a")]
    );
    assert_eq!(receipt.run_id, Some(streams.run_id));

    let writes = sink.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].draft_id, d.id);
    assert_eq!(writes[0].cfg, cfg);
}

#[tokio::test]
async fn mock_sink_emits_progress_events_in_order() {
    let sink = MockSink::new();
    let streams = RunStreams::new(RunId::new());
    let run_id = streams.run_id;
    // Keep only the receiver we need; by using `split()` the log_rx is
    // dropped immediately, and we explicitly drop the senders below
    // before draining so `recv()` can terminate on channel close.
    let ((progress_tx, log_tx), (mut progress_rx, _log_rx)) = streams.split();

    let ctx = SinkCtx::new(
        Some(run_id),
        progress_tx.clone(),
        log_tx.clone(),
        CancellationToken::new(),
    );

    sink.write(&ctx, &markdown_cfg("/tmp/x"), &draft())
        .await
        .expect("write ok");

    // Close every sender clone so the receiver drain terminates on
    // `None` once the three recorded events have been observed.
    drop(ctx);
    drop(progress_tx);
    drop(log_tx);

    let mut phases = Vec::new();
    while let Some(event) = progress_rx.recv().await {
        assert_eq!(event.run_id, run_id);
        phases.push(event.phase);
    }

    assert_eq!(phases.len(), 3);
    assert!(matches!(phases[0], ProgressPhase::Starting { .. }));
    assert!(matches!(
        phases[1],
        ProgressPhase::InProgress {
            completed: 1,
            total: Some(1),
            ..
        }
    ));
    assert!(matches!(phases[2], ProgressPhase::Completed { .. }));
}

#[tokio::test]
async fn mock_sink_bails_when_cancel_token_is_set_before_write() {
    let sink = MockSink::new();
    let streams = RunStreams::new(RunId::new());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let ctx = ctx_from_streams(&streams, cancel);

    let err = sink
        .write(&ctx, &markdown_cfg("/tmp/x"), &draft())
        .await
        .expect_err("write should abort on cancelled run");

    assert_eq!(
        err.code(),
        dayseam_core::error_codes::RUN_CANCELLED_BY_USER,
        "cancellation must use the stable error code"
    );
    assert!(
        sink.writes().is_empty(),
        "no write should have been recorded"
    );
}

#[tokio::test]
async fn mock_sink_fail_next_is_one_shot() {
    let sink = MockSink::new();
    let streams = RunStreams::new(RunId::new());
    let ctx = ctx_from_streams(&streams, CancellationToken::new());

    sink.fail_next_with(dayseam_core::DayseamError::Io {
        code: dayseam_core::error_codes::SINK_FS_NOT_WRITABLE.into(),
        path: Some(PathBuf::from("/read-only")),
        message: "permission denied".into(),
    });

    let err = sink
        .write(&ctx, &markdown_cfg("/read-only"), &draft())
        .await
        .expect_err("armed failure should surface");
    assert_eq!(err.code(), dayseam_core::error_codes::SINK_FS_NOT_WRITABLE);

    // Second write succeeds — the injection is strictly one-shot.
    sink.write(&ctx, &markdown_cfg("/tmp/writable"), &draft())
        .await
        .expect("subsequent write should succeed");
}

#[tokio::test]
async fn sink_ctx_bail_if_cancelled_returns_dayseam_cancelled_error() {
    let streams = RunStreams::new(RunId::new());
    let cancel = CancellationToken::new();
    let ctx = ctx_from_streams(&streams, cancel.clone());
    assert!(ctx.bail_if_cancelled().is_ok());

    cancel.cancel();
    let err = ctx
        .bail_if_cancelled()
        .expect_err("must error once cancelled");
    assert_eq!(err.code(), dayseam_core::error_codes::RUN_CANCELLED_BY_USER);
}
