//! IPC layer: Tauri command surface plus the two forwarders that carry
//! per-run streams and app-wide broadcasts out to the frontend.

pub mod atlassian;
pub mod broadcast_forwarder;
pub mod commands;
pub mod github;
pub mod run_forwarder;
pub mod secret;
