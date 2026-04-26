//! `connectors-sdk` — the contract every Dayseam source connector
//! speaks.
//!
//! Public surface:
//!
//! * [`SourceConnector`] — the trait connector crates implement.
//! * [`SyncRequest`] / [`SyncResult`] / [`SyncStats`] / [`Checkpoint`] —
//!   the shapes of a `sync` call.
//! * [`AuthStrategy`] / [`PatAuth`] / [`BasicAuth`] / [`OAuthAuth`] /
//!   [`NoneAuth`] / [`AuthDescriptor`] — durable per-source auth.
//! * [`oauth`] module — PKCE, authorization-code exchange,
//!   single-flighted refresh, and the [`oauth::TokenPersister`]
//!   callback the orchestrator implements to write new tokens back to
//!   the keychain without `connectors-sdk` importing `dayseam-secrets`.
//! * [`ConnCtx`] — the per-run context threaded into every connector
//!   call, carrying the run id, identity, progress/log senders, raw
//!   store, clock, HTTP client, and cancellation token.
//! * [`HttpClient`] / [`RetryPolicy`] — the retry-aware HTTP wrapper
//!   every HTTP-using connector goes through.
//! * [`Clock`] / [`SystemClock`] — injectable wall clock.
//! * [`RawStore`] / [`NoopRawStore`] — pluggable raw payload
//!   persistence.
//!
//! [`MockConnector`] is always compiled but lives behind a distinct
//! module so release builds can tree-shake it — nothing in the
//! production code path references it.

pub mod auth;
pub mod clock;
pub mod connector;
pub mod ctx;
pub mod dtos;
pub mod http;
pub mod mock;
pub mod oauth;
pub mod raw_store;
pub mod sync;

pub use auth::{AuthDescriptor, AuthStrategy, BasicAuth, NoneAuth, OAuthAuth, PatAuth};
pub use clock::{Clock, SystemClock};
pub use connector::SourceConnector;
pub use ctx::ConnCtx;
pub use http::{HttpClient, RetryPolicy};
pub use mock::MockConnector;
pub use oauth::{
    exchange_code, generate_pkce_pair, is_scope_downgrade, scope_downgrade_error, CodeChallenge,
    CodeVerifier, NoopTokenPersister, SharedPersister, TokenPair, TokenPersister,
};
pub use raw_store::{NoopRawStore, RawStore};
pub use sync::{Checkpoint, SyncRequest, SyncResult, SyncStats};
