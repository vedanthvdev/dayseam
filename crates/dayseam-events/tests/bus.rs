//! Integration tests for `dayseam-events`.
//!
//! The unit tests inside each module cover happy-path basics. These
//! integration tests cover the behaviours that matter across module
//! boundaries: cloned senders all feed the same receiver, receivers
//! terminate cleanly when senders are dropped, broadcast lag is
//! recoverable, and log + progress streams stay independent.

use std::time::Duration;

use chrono::Utc;
use dayseam_core::{LogLevel, ProgressPhase, RunId, ToastEvent, ToastSeverity};
use dayseam_events::{AppBus, RunStreams, ToastSubscribeError};
use tokio::time::timeout;
use uuid::Uuid;

fn toast(title: &str) -> ToastEvent {
    ToastEvent {
        id: Uuid::new_v4(),
        severity: ToastSeverity::Info,
        title: title.into(),
        body: None,
        emitted_at: Utc::now(),
    }
}

#[tokio::test]
async fn cloned_senders_all_deliver_to_the_same_receiver() {
    let mut streams = RunStreams::new(RunId::new());
    let tx_a = streams.progress_tx.clone();
    let tx_b = streams.progress_tx.clone();

    tx_a.send(
        None,
        ProgressPhase::Starting {
            message: "from a".into(),
        },
    );
    tx_b.send(
        None,
        ProgressPhase::Starting {
            message: "from b".into(),
        },
    );
    drop(tx_a);
    drop(tx_b);
    drop(streams.progress_tx);

    let mut messages = Vec::new();
    while let Some(evt) = streams.progress_rx.recv().await {
        if let ProgressPhase::Starting { message } = evt.phase {
            messages.push(message);
        }
    }
    messages.sort();
    assert_eq!(messages, vec!["from a", "from b"]);
}

#[tokio::test]
async fn progress_receiver_yields_none_once_every_sender_is_dropped() {
    let streams = RunStreams::new(RunId::new());
    let (senders, (mut progress_rx, _log_rx)) = streams.split();
    drop(senders);

    let result = timeout(Duration::from_millis(200), progress_rx.recv())
        .await
        .expect("receiver observes end-of-stream without hanging");
    assert!(result.is_none());
}

#[tokio::test]
async fn progress_and_log_streams_are_independent() {
    let mut streams = RunStreams::new(RunId::new());
    streams.progress_tx.send(
        None,
        ProgressPhase::Starting {
            message: "p".into(),
        },
    );
    streams
        .log_tx
        .send(LogLevel::Info, None, "l", serde_json::Value::Null);
    drop(streams.progress_tx);
    drop(streams.log_tx);

    let p = streams.progress_rx.recv().await.expect("one progress");
    let l = streams.log_rx.recv().await.expect("one log");
    assert!(matches!(p.phase, ProgressPhase::Starting { .. }));
    assert_eq!(l.message, "l");
    assert!(streams.progress_rx.recv().await.is_none());
    assert!(streams.log_rx.recv().await.is_none());
}

#[tokio::test]
async fn broadcast_lagged_subscriber_recovers_without_blocking_publisher() {
    // Capacity 2 plus 10 publishes means the slow subscriber overflows
    // and the broadcast channel skips ahead. The contract we're
    // asserting is: (a) the publisher never blocks, (b) the subscriber
    // observes Lagged explicitly rather than silently losing events,
    // and (c) once the subscriber drains the remaining ring contents
    // it can continue reading new publishes.
    let bus = AppBus::with_capacity(2);
    let mut slow = bus.subscribe_toasts();

    for i in 0..10 {
        bus.publish_toast(toast(&format!("t{i}")));
    }

    let first = slow.recv().await;
    assert!(
        matches!(first, Err(ToastSubscribeError::Lagged(_))),
        "slow subscriber must observe Lagged, got {first:?}",
    );

    // Drain whatever's still in the ring from the pre-lag batch so the
    // subscriber is caught up to the live edge, then publish a fresh
    // event and assert it arrives.
    while let Ok(Ok(_)) = timeout(Duration::from_millis(20), slow.recv()).await {}

    bus.publish_toast(toast("recovered"));
    let next = timeout(Duration::from_millis(200), slow.recv())
        .await
        .expect("subscriber receives post-recovery publish within timeout")
        .expect("channel still live");
    assert_eq!(next.title, "recovered");
}

#[tokio::test]
async fn broadcast_multi_publisher_multi_subscriber() {
    let bus = AppBus::new();
    let mut subs = [
        bus.subscribe_toasts(),
        bus.subscribe_toasts(),
        bus.subscribe_toasts(),
    ];

    let handles: Vec<_> = (0..5)
        .map(|i| {
            let bus = bus.clone();
            tokio::spawn(async move {
                bus.publish_toast(toast(&format!("p{i}")));
            })
        })
        .collect();
    for h in handles {
        h.await.expect("publisher task");
    }

    for sub in subs.iter_mut() {
        let mut received = 0;
        while let Ok(Ok(_)) = timeout(Duration::from_millis(100), sub.recv()).await {
            received += 1;
        }
        assert_eq!(received, 5, "every subscriber sees every publisher");
    }
}
