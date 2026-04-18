//! App-wide broadcast bus for infrequent, small signals.
//!
//! Today this is the [`ToastEvent`] channel; in later versions it also
//! carries settings-changed and update-available notifications. The
//! shape is intentionally extensible — adding a new broadcast kind is a
//! field on [`AppBus`] plus paired `publish_*` / `subscribe_*`
//! methods, with no changes to existing call sites.
//!
//! Uses [`tokio::sync::broadcast`] so:
//!
//! * publishers never block (they always send to the in-memory ring
//!   buffer),
//! * multiple subscribers each receive every event (fanout), and
//! * a slow subscriber that falls behind by more than `capacity` gets
//!   a `RecvError::Lagged(n)` and continues from the newest event
//!   rather than stalling the publisher.
//!
//! Dropping a stale "update available" ping is always preferable to
//! blocking a sync.

use dayseam_core::ToastEvent;
use tokio::sync::broadcast;

/// Default ring-buffer capacity per broadcast channel. 64 is generous
/// enough for every realistic UI scenario (a user sees toasts in ones,
/// not hundreds) while keeping memory negligible.
const DEFAULT_CAPACITY: usize = 64;

/// App-wide broadcast hub. Cheap to clone; internal `Sender` handles
/// are reference-counted.
#[derive(Debug, Clone)]
pub struct AppBus {
    toasts: broadcast::Sender<ToastEvent>,
}

impl AppBus {
    /// Create a bus with the default capacity ([`DEFAULT_CAPACITY`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a bus with an explicit per-channel capacity. Mostly
    /// useful in tests that want to exercise lag behaviour with a
    /// tiny buffer.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let (toasts, _) = broadcast::channel(capacity);
        Self { toasts }
    }

    /// Publish a toast to every current subscriber. Returns the number
    /// of subscribers that received it (0 when there are none — that
    /// is not an error; a toast published while no windows are open
    /// is simply dropped).
    pub fn publish_toast(&self, event: ToastEvent) -> usize {
        self.toasts.send(event).unwrap_or(0)
    }

    /// Subscribe to future toasts. Each subscriber gets its own
    /// independent queue.
    #[must_use]
    pub fn subscribe_toasts(&self) -> broadcast::Receiver<ToastEvent> {
        self.toasts.subscribe()
    }

    /// Number of live subscribers across all broadcast channels. Used
    /// by observability hooks and by tests.
    #[must_use]
    pub fn toast_subscriber_count(&self) -> usize {
        self.toasts.receiver_count()
    }
}

impl Default for AppBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors a subscriber can observe while waiting for broadcasts.
/// Exposed by name so downstream consumers don't have to take a direct
/// dependency on `tokio::sync::broadcast` just to match on its errors.
pub type ToastSubscribeError = broadcast::error::RecvError;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use dayseam_core::ToastSeverity;
    use uuid::Uuid;

    fn sample_toast() -> ToastEvent {
        ToastEvent {
            id: Uuid::new_v4(),
            severity: ToastSeverity::Info,
            title: "hello".into(),
            body: None,
            emitted_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn toast_fanout_reaches_every_subscriber() {
        let bus = AppBus::new();
        let mut a = bus.subscribe_toasts();
        let mut b = bus.subscribe_toasts();

        let toast = sample_toast();
        assert_eq!(bus.publish_toast(toast.clone()), 2);

        let got_a = a.recv().await.expect("a receives");
        let got_b = b.recv().await.expect("b receives");
        assert_eq!(got_a.id, toast.id);
        assert_eq!(got_b.id, toast.id);
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_not_an_error() {
        let bus = AppBus::new();
        assert_eq!(bus.publish_toast(sample_toast()), 0);
    }

    #[tokio::test]
    async fn dropped_subscriber_does_not_affect_others() {
        let bus = AppBus::new();
        let mut a = bus.subscribe_toasts();
        {
            let _b = bus.subscribe_toasts();
            assert_eq!(bus.toast_subscriber_count(), 2);
        }
        assert_eq!(bus.toast_subscriber_count(), 1);

        let toast = sample_toast();
        assert_eq!(bus.publish_toast(toast.clone()), 1);
        assert_eq!(a.recv().await.expect("a still alive").id, toast.id);
    }
}
