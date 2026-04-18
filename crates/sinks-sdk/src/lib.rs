//! `sinks-sdk` — the interface every Dayseam sink implements.
//!
//! This crate intentionally owns **behaviour**, not data. The data types
//! that describe a sink (`Sink`, `SinkKind`, `SinkConfig`,
//! `SinkCapabilities`, `WriteReceipt`) live in `dayseam-core` so the
//! persistence layer, the orchestrator, and the UI can reference them
//! without pulling in async machinery. The trait surface — the
//! [`SinkAdapter`] contract itself, [`SinkCtx`], and the hermetic
//! [`MockSink`] — lives here.
//!
//! The split mirrors `connectors-sdk` and matches the layering diagram
//! in `ARCHITECTURE.md` §3.1: `dayseam-core` is allowed to depend on
//! nothing inside the workspace, the SDK crates sit one layer up, and
//! concrete sink implementations (coming in Phase 2) sit one layer above
//! the SDK.
//!
//! ## Writing a new sink
//!
//! 1. Add a [`SinkKind`] variant in `dayseam-core` and a matching
//!    [`SinkConfig`] variant.
//! 2. Create `crates/sinks/sink-<name>/` with a [`SinkAdapter`] impl.
//! 3. Declare the sink's [`SinkCapabilities`] — call
//!    [`SinkCapabilities::validate`] in a unit test so an illegal combo
//!    is caught at build time, not runtime.
//! 4. Write `write()` against the temp-file + atomic-rename rule in
//!    `ARCHITECTURE.md` §9.1. Partial writes on cancel/crash are a
//!    correctness bug, not a "best-effort" outcome.
//! 5. Register the adapter in the orchestrator's sink dispatcher and
//!    add a UI config panel driven by the new [`SinkConfig`] variant.
//!
//! ## What this crate deliberately does *not* own
//!
//! - Scheduling or retry logic. The orchestrator fires the sink; the
//!   sink trusts the orchestrator's policy and only writes when asked.
//! - HTTP. v0.1 ships only local-filesystem sinks. When remote sinks
//!   land in v0.4+, the `SinkCtx` grows a `http: HttpClient` field as
//!   an additive change.
//! - Auth. Same reasoning — added when a sink ships that needs it.

pub mod adapter;
pub mod ctx;
pub mod mock;

pub use adapter::SinkAdapter;
pub use ctx::SinkCtx;
pub use mock::{MockSink, MockWrite};

// Re-export the dayseam-core sink data types so crate consumers can do
// `use sinks_sdk::{SinkAdapter, SinkCapabilities, SinkKind, …}`
// without bouncing through two imports. This is a convenience, not a
// re-home: the canonical definitions live in `dayseam-core`.
pub use dayseam_core::{
    CapabilityConflict, Sink, SinkCapabilities, SinkConfig, SinkKind, WriteReceipt,
};
