//! In-memory registry of in-flight OAuth PKCE login sessions.
//!
//! One registry lives on [`AppState`][crate::state::AppState] for the
//! lifetime of the process. Every
//! [`oauth_begin_login`][crate::ipc::oauth::oauth_begin_login] call
//! inserts a session, the background loopback driver mutates the
//! session's status as the flow progresses, and
//! [`oauth_session_status`][crate::ipc::oauth::oauth_session_status]
//! / `oauth_cancel_login` read or cancel a session by id.
//!
//! The registry is intentionally *not* persisted. A session represents
//! a browser tab the user is currently consenting in; app shutdown
//! drops every in-flight tab, so surviving tokens would be strictly
//! less useful than starting over from a clean slate. The *successful*
//! `TokenPair`s that settle here are pulled out by a separate IPC
//! call (DAY-203 `outlook_sources_add`) which promotes them into a
//! keychain row + `sources` row in one transactional step; the token
//! itself never leaks out of this process over IPC.
//!
//! Concurrency discipline: every public method awaits the inner
//! [`tokio::sync::Mutex`] and holds the guard for the minimum span
//! needed to finish one state transition. Long operations (HTTP
//! round-trip to the token endpoint, awaiting the loopback callback)
//! happen *outside* the lock so a slow IdP cannot stall the UI's
//! `oauth_session_status` polls.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use connectors_sdk::TokenPair;
use dayseam_core::{OAuthSessionId, OAuthSessionStatus, OAuthSessionView};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// One row in the session registry. Internal to the desktop crate —
/// the public projection the UI sees is [`OAuthSessionView`], which
/// deliberately omits the sensitive token and cancellation fields.
#[derive(Debug)]
pub struct OAuthSession {
    pub id: OAuthSessionId,
    pub provider_id: String,
    pub created_at: DateTime<Utc>,
    pub status: OAuthSessionStatus,
    /// Filled in by the background loopback driver once
    /// `connectors_sdk::exchange_code` returns a pair. Kept server-
    /// side on purpose — DAY-203's follow-up IPC pulls it out to
    /// persist to the keychain; no other path in this crate ever
    /// hands it back across the IPC boundary.
    pub token_pair: Option<TokenPair>,
    /// Cancellation handle for the background driver. Triggered by
    /// `oauth_cancel_login` or by process shutdown; the driver
    /// observes it in its `tokio::select!` and aborts without
    /// completing the token exchange.
    pub cancel: CancellationToken,
}

impl OAuthSession {
    /// Project this session into the sensitive-field-free
    /// [`OAuthSessionView`] the frontend sees on the wire.
    #[must_use]
    pub fn to_view(&self) -> OAuthSessionView {
        OAuthSessionView {
            id: self.id,
            provider_id: self.provider_id.clone(),
            created_at: self.created_at,
            status: self.status.clone(),
        }
    }
}

/// Thread-safe registry of every currently-tracked OAuth login
/// session. Cloning the registry is cheap (it's an `Arc` over the
/// same mutex), which is what lets the background driver task own a
/// handle without taking a Tauri `State<'_, AppState>`.
#[derive(Debug, Clone, Default)]
pub struct OAuthSessionRegistry {
    inner: Arc<Mutex<HashMap<OAuthSessionId, OAuthSession>>>,
}

impl OAuthSessionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a new session. Returns the previous entry for `id`
    /// if one somehow existed — the caller should log and ignore
    /// that return value because `OAuthSessionId` is a v4 UUID and
    /// collisions are vanishingly rare; surfacing the collision is
    /// still preferable to silently clobbering a live session.
    pub async fn insert(&self, session: OAuthSession) -> Option<OAuthSession> {
        let mut guard = self.inner.lock().await;
        guard.insert(session.id, session)
    }

    /// Return the public view of `id` if a session exists. The UI's
    /// `oauth_session_status` command is the only expected caller.
    pub async fn get_view(&self, id: &OAuthSessionId) -> Option<OAuthSessionView> {
        let guard = self.inner.lock().await;
        guard.get(id).map(OAuthSession::to_view)
    }

    /// Mutate a session's status. Returns the resulting view if the
    /// session existed so the caller can emit a single
    /// `oauth://session-updated` event from the same lock acquisition
    /// — reading the registry a second time to build the event
    /// payload would be racy against a concurrent cancel.
    pub async fn update_status(
        &self,
        id: &OAuthSessionId,
        status: OAuthSessionStatus,
    ) -> Option<OAuthSessionView> {
        let mut guard = self.inner.lock().await;
        let session = guard.get_mut(id)?;
        session.status = status;
        Some(session.to_view())
    }

    /// Store a freshly-exchanged [`TokenPair`] against `id`. Called
    /// by the loopback driver the instant
    /// `connectors_sdk::exchange_code` returns. Does *not* flip the
    /// status — the caller combines this with a subsequent
    /// [`Self::update_status`] so the "tokens are here" and "status
    /// is Completed" invariants commit in a single acquisition.
    pub async fn set_token_pair(&self, id: &OAuthSessionId, pair: TokenPair) {
        let mut guard = self.inner.lock().await;
        if let Some(session) = guard.get_mut(id) {
            session.token_pair = Some(pair);
        }
    }

    /// Clone the stored [`TokenPair`] for `id` without consuming the
    /// session. Used by DAY-203's `outlook_validate_credentials`,
    /// which may run multiple times before the user commits the
    /// source (e.g. the user clicks "Add source", sees a transient
    /// Graph 5xx, cancels, and tries again). The session row and
    /// token bytes stay server-side until `take_token_pair` fires
    /// from `outlook_sources_add`.
    pub async fn peek_token_pair(&self, id: &OAuthSessionId) -> Option<TokenPair> {
        let guard = self.inner.lock().await;
        guard.get(id).and_then(|s| s.token_pair.clone())
    }

    /// Remove a session and return its stored [`TokenPair`], if any.
    /// Used by DAY-203's `outlook_sources_add` IPC that promotes a
    /// Completed session into a keychain row. Once this function
    /// returns the session is gone from the registry, so a second
    /// call with the same id finds nothing and the UI has a single
    /// moment at which the token leaves process memory.
    pub async fn take_token_pair(&self, id: &OAuthSessionId) -> Option<TokenPair> {
        let mut guard = self.inner.lock().await;
        let mut session = guard.remove(id)?;
        session.token_pair.take()
    }

    /// Signal cancellation on the session's background driver and
    /// flip its status to `Cancelled`. Returns the updated view if
    /// the session existed. The registry keeps the row around so a
    /// UI that was mid-poll sees the terminal `Cancelled` state
    /// before falling off on its next request.
    pub async fn cancel(&self, id: &OAuthSessionId) -> Option<OAuthSessionView> {
        let mut guard = self.inner.lock().await;
        let session = guard.get_mut(id)?;
        session.cancel.cancel();
        session.status = OAuthSessionStatus::Cancelled;
        Some(session.to_view())
    }

    /// Best-effort "tear down every in-flight session" hook. Called
    /// from app shutdown so the background drivers exit promptly
    /// instead of sitting on timeouts while the process tries to
    /// quit.
    #[allow(dead_code)] // Hooked in by the shutdown path in a follow-up ticket.
    pub async fn cancel_all(&self) {
        let guard = self.inner.lock().await;
        for session in guard.values() {
            session.cancel.cancel();
        }
    }

    /// Test helper — returns the count of currently-tracked sessions
    /// so integration tests can assert "the registry is empty after
    /// the happy-path completes."
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Paired with [`Self::len`] so clippy's `len_without_is_empty`
    /// lint has a companion to check. Not called in production —
    /// the registry is consulted by key via `get_view` / `cancel` —
    /// but the tests lean on it to assert a clean registry after a
    /// happy-path run.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_session(id: OAuthSessionId) -> OAuthSession {
        OAuthSession {
            id,
            provider_id: "microsoft-outlook".to_string(),
            created_at: Utc::now(),
            status: OAuthSessionStatus::Pending,
            token_pair: None,
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_view_returns_none_for_missing_session() {
        let reg = OAuthSessionRegistry::new();
        let id = OAuthSessionId::new();
        assert!(reg.get_view(&id).await.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn insert_then_get_view_reflects_current_status() {
        let reg = OAuthSessionRegistry::new();
        let id = OAuthSessionId::new();
        reg.insert(stub_session(id)).await;
        let view = reg.get_view(&id).await.expect("view exists");
        assert_eq!(view.status, OAuthSessionStatus::Pending);
        reg.update_status(&id, OAuthSessionStatus::Completed).await;
        let view = reg.get_view(&id).await.expect("view still exists");
        assert_eq!(view.status, OAuthSessionStatus::Completed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_flips_status_and_trips_token() {
        let reg = OAuthSessionRegistry::new();
        let id = OAuthSessionId::new();
        let session = stub_session(id);
        let observer = session.cancel.clone();
        reg.insert(session).await;
        let view = reg.cancel(&id).await.expect("cancel finds session");
        assert_eq!(view.status, OAuthSessionStatus::Cancelled);
        assert!(observer.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_on_missing_session_is_a_noop() {
        let reg = OAuthSessionRegistry::new();
        assert!(reg.cancel(&OAuthSessionId::new()).await.is_none());
    }

    fn stub_pair() -> TokenPair {
        connectors_sdk::TokenPair::new(
            "access".to_string(),
            "refresh".to_string(),
            Utc::now() + chrono::Duration::hours(1),
            vec!["openid".to_string()],
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn peek_token_pair_does_not_consume_session() {
        let reg = OAuthSessionRegistry::new();
        let id = OAuthSessionId::new();
        reg.insert(stub_session(id)).await;
        reg.set_token_pair(&id, stub_pair()).await;

        let first = reg.peek_token_pair(&id).await.expect("tokens present");
        assert_eq!(first.access_token, "access");

        // Session and pair still addressable after peek.
        let second = reg
            .peek_token_pair(&id)
            .await
            .expect("tokens still present");
        assert_eq!(second.refresh_token, "refresh");
        assert!(reg.get_view(&id).await.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn take_token_pair_consumes_session() {
        let reg = OAuthSessionRegistry::new();
        let id = OAuthSessionId::new();
        reg.insert(stub_session(id)).await;
        reg.set_token_pair(&id, stub_pair()).await;

        let taken = reg.take_token_pair(&id).await.expect("tokens present");
        assert_eq!(taken.access_token, "access");

        assert!(reg.peek_token_pair(&id).await.is_none());
        assert!(reg.get_view(&id).await.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn peek_without_token_pair_returns_none() {
        let reg = OAuthSessionRegistry::new();
        let id = OAuthSessionId::new();
        reg.insert(stub_session(id)).await;
        assert!(reg.peek_token_pair(&id).await.is_none());
    }
}
