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
/// The canonical dev capability JSON. Lives on disk at
/// `capabilities.dev.template.json` (outside the `capabilities/`
/// directory so tauri-build's `capabilities/**/*` scanner never sees
/// the template itself) and is embedded here at compile time. The
/// integration test in `tests/capabilities.rs` parses the exact same
/// bytes, which makes the parity check independent of whether the
/// on-disk `capabilities/dev.json` happens to be materialised when
/// the test binary runs.
const DEV_CAPABILITY_BODY: &str = include_str!("capabilities.dev.template.json");

fn main() {
    // Features of the *package being built* are not passed through to
    // the build-script's own compilation, so `cfg!(feature = "...")`
    // evaluates against the build-script crate's (empty) feature set
    // and is always `false`. Cargo does expose the package's active
    // features as `CARGO_FEATURE_<UPPER_SNAKE>` environment variables,
    // which is the sanctioned way to read them from build.rs.
    // See: https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts
    let dev_commands = std::env::var_os("CARGO_FEATURE_DEV_COMMANDS").is_some();
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DEV_COMMANDS");
    println!("cargo:rerun-if-changed=capabilities.dev.template.json");

    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo"),
    );
    let dev_capability = manifest_dir
        .join("capabilities")
        .join(DEV_CAPABILITY_FILENAME);

    // Remove any stale dev capability eagerly so a release build can
    // never accidentally ship a grant left over from a prior dev
    // build. Writing the fresh file (when the feature is on) is
    // deferred until *after* `tauri_build::try_build` runs, because
    // tauri-build scans `capabilities/**/*` at the start of its own
    // pipeline and scrubs files whose identifiers it didn't resolve
    // against the command manifest we pass in.
    if !dev_commands && dev_capability.exists() {
        std::fs::remove_file(&dev_capability)
            .expect("failed to remove stale capabilities/dev.json");
    }

    // `AppManifest::commands` wants a `&'static [&'static str]`, so
    // we pick between two static slices at runtime. Keeping two
    // literal arrays — rather than assembling one dynamically — makes
    // the dev-commands gate visible at a glance when diffing build.rs.
    const PROD_COMMANDS: &[&str] = &[
        "settings_get",
        "settings_update",
        "logs_tail",
        "persons_get_self",
        "persons_update_self",
        "sources_list",
        "sources_add",
        "sources_update",
        "sources_delete",
        "sources_healthcheck",
        "identities_list_for",
        "identities_upsert",
        "identities_delete",
        "local_repos_list",
        "local_repos_set_private",
        "sinks_list",
        "sinks_add",
        "report_generate",
        "report_cancel",
        "report_get",
        "report_save",
        "retention_sweep_now",
        "activity_events_get",
        "shell_open",
        "gitlab_validate_pat",
        "atlassian_validate_credentials",
        "atlassian_sources_add",
        "atlassian_sources_reconnect",
        "github_validate_credentials",
        "github_sources_add",
        "github_sources_reconnect",
    ];
    const DEV_COMMANDS: &[&str] = &[
        "settings_get",
        "settings_update",
        "logs_tail",
        "persons_get_self",
        "persons_update_self",
        "sources_list",
        "sources_add",
        "sources_update",
        "sources_delete",
        "sources_healthcheck",
        "identities_list_for",
        "identities_upsert",
        "identities_delete",
        "local_repos_list",
        "local_repos_set_private",
        "sinks_list",
        "sinks_add",
        "report_generate",
        "report_cancel",
        "report_get",
        "report_save",
        "retention_sweep_now",
        "activity_events_get",
        "shell_open",
        "gitlab_validate_pat",
        "atlassian_validate_credentials",
        "atlassian_sources_add",
        "atlassian_sources_reconnect",
        "github_validate_credentials",
        "github_sources_add",
        "github_sources_reconnect",
        "dev_emit_toast",
        "dev_start_demo_run",
    ];

    let commands: &'static [&'static str] = if dev_commands {
        DEV_COMMANDS
    } else {
        PROD_COMMANDS
    };
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(commands));
    tauri_build::try_build(attributes).expect("tauri-build failed");

    // Written after `try_build` intentionally — see the note above the
    // stale-removal block. The file is loaded at app startup by Tauri's
    // runtime capability resolver, which walks `capabilities/*.json` at
    // launch, so the build-time rescan order is fine.
    if dev_commands {
        std::fs::write(&dev_capability, DEV_CAPABILITY_BODY)
            .expect("failed to write capabilities/dev.json");
    }
}
