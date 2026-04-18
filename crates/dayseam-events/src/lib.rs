//! Run-scoped streams and app-wide broadcast bus for Dayseam.
//!
//! Two distinct transports for two distinct jobs, matching the contract
//! documented in `ARCHITECTURE.md` §11.3:
//!
//! * **Per-run streams** ([`RunStreams`]) — ordered, unbounded
//!   [`tokio::sync::mpsc`] channels scoped to a single [`RunId`]. One
//!   pair of streams is opened per sync run. Producers get cheap
//!   cloneable sender handles ([`ProgressSender`], [`LogSender`]) that
//!   never block and never drop events. Receivers are held by the
//!   Tauri layer and forwarded verbatim to the frontend through a
//!   typed `Channel<T>` command argument.
//! * **App-wide broadcast** ([`AppBus`]) — [`tokio::sync::broadcast`]
//!   fanout for infrequent, small signals such as
//!   [`ToastEvent`]. Publishers never block; slow subscribers get
//!   `RecvError::Lagged` and recover by resubscribing. The Tauri layer
//!   forwards broadcasts to every window via `Manager::emit`.
//!
//! The event types themselves live in `dayseam-core::types::events`
//! alongside every other IPC type so `ts-rs` can generate their
//! TypeScript equivalents in a single pass.

pub mod app_bus;
pub mod run_streams;

pub use app_bus::{AppBus, ToastSubscribeError};
pub use run_streams::{LogReceiver, LogSender, ProgressReceiver, ProgressSender, RunStreams};

pub use dayseam_core::{LogEvent, ProgressEvent, ProgressPhase, RunId, ToastEvent, ToastSeverity};
