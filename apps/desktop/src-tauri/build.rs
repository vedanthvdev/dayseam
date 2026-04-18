//! Hook app-defined Tauri commands into the permission system.
//!
//! Tauri 2 denies every command unless it is listed both in the
//! runtime `invoke_handler!` and in a capability's `permissions` array
//! as `allow-<command-name>`. Declaring the command set on the
//! [`tauri_build::AppManifest`] makes `tauri-build` autogenerate the
//! matching permission files under the crate's `OUT_DIR`, so adding a
//! new command in `src/ipc/commands.rs` only requires touching four
//! places:
//!
//!   1. the `#[tauri::command]` function,
//!   2. the command list below (production commands unconditional,
//!      dev-only commands behind the `dev-commands` feature gate),
//!   3. either `capabilities/default.json` (production) or the dev
//!      capability written by this script (dev-only), and
//!   4. `packages/ipc-types/src/index.ts`'s `Commands` map.
//!
//! Any of the four missing surfaces a loud error — either at compile
//! time (capability entry references an unknown permission) or at
//! runtime (webview call for a command not in the handler).
//!
//! The dev capability file is written programmatically on purpose so
//! it cannot be committed by accident. Whether it exists at build time
//! exactly tracks the `dev-commands` feature flag: a plain
//! `cargo build --release` produces neither dev commands nor a dev
//! capability grant.

use std::path::PathBuf;

const DEV_CAPABILITY_FILENAME: &str = "dev.json";
const DEV_CAPABILITY_BODY: &str = r#"{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "dev",
  "description": "Dev-only capability written by `build.rs` when the `dev-commands` Cargo feature is enabled. Grants the frontend test harness and the in-app dev tray access to `dev_emit_toast` and `dev_start_demo_run`. Absent from release builds.",
  "windows": ["main"],
  "permissions": [
    "allow-dev-emit-toast",
    "allow-dev-start-demo-run"
  ]
}
"#;

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo"),
    );
    let dev_capability = manifest_dir
        .join("capabilities")
        .join(DEV_CAPABILITY_FILENAME);

    if cfg!(feature = "dev-commands") {
        std::fs::write(&dev_capability, DEV_CAPABILITY_BODY)
            .expect("failed to write capabilities/dev.json");
    } else if dev_capability.exists() {
        std::fs::remove_file(&dev_capability)
            .expect("failed to remove stale capabilities/dev.json");
    }

    // `AppManifest::commands` takes a `&'static [&'static str]`, so we
    // can't build the list dynamically with a `Vec`. Two static slices
    // keep the dev-commands gate visible at a glance.
    #[cfg(feature = "dev-commands")]
    const COMMANDS: &[&str] = &[
        "settings_get",
        "settings_update",
        "logs_tail",
        "dev_emit_toast",
        "dev_start_demo_run",
    ];
    #[cfg(not(feature = "dev-commands"))]
    const COMMANDS: &[&str] = &["settings_get", "settings_update", "logs_tail"];

    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
