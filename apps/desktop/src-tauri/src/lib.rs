//! Dayseam desktop app library root.
//!
//! The desktop crate is split into a thin `main.rs` binary and a
//! testable library (this file) so integration tests can exercise the
//! IPC plumbing without booting an actual Tauri runtime.

pub mod ipc;
pub mod scheduler_task;
pub mod startup;
pub mod state;
pub mod tracing_init;

pub use state::{AppState, RunHandle, RunRegistry};
