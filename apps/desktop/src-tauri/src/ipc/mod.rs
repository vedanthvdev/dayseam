//! IPC layer: Tauri command surface plus the two forwarders that carry
//! per-run streams and app-wide broadcasts out to the frontend.

pub mod broadcast_forwarder;
pub mod commands;
pub mod run_forwarder;
pub mod secret;
