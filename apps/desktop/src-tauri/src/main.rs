#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use dayseam_db::LogRepo;
use dayseam_desktop::ipc::{atlassian, broadcast_forwarder, commands};
use dayseam_desktop::startup;
use tauri::Manager;

fn main() {
    // One multi-threaded Tokio runtime powers the whole app: the
    // database pool, the broadcast forwarder, and per-run forwarders
    // all share it. `tauri::async_runtime` wraps the same machinery
    // so there's no second reactor to keep in sync.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let _guard = runtime.enter();

    let builder = tauri::Builder::default()
        // Registers the native file/directory chooser. The only
        // permission we grant on it is `dialog:allow-open` (see
        // `capabilities/default.json`); save-pickers, message boxes,
        // and confirm dialogs stay denied so the plugin surface can't
        // grow by accident.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = startup::default_data_dir();
            let state = tauri::async_runtime::block_on(startup::build_app_state(&data_dir))
                .expect("build AppState");
            let pool = state.pool.clone();
            let app_bus = state.app_bus.clone();
            app.manage(state);

            let handle = app.handle().clone();
            let logs = LogRepo::new(pool);
            let _broadcast_task = broadcast_forwarder::spawn(handle, app_bus, logs);
            Ok(())
        });

    // Release builds compile the dev commands out entirely so the
    // binary ships with a minimal IPC surface. Keep this list in
    // lockstep with `COMMANDS` in `build.rs` and with
    // `capabilities/default.json` — every command mentioned here
    // must appear in both, or Tauri 2 denies the call at runtime.
    #[cfg(feature = "dev-commands")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::settings_get,
        commands::settings_update,
        commands::logs_tail,
        commands::persons_get_self,
        commands::persons_update_self,
        commands::sources_list,
        commands::sources_add,
        commands::sources_update,
        commands::sources_delete,
        commands::sources_healthcheck,
        commands::identities_list_for,
        commands::identities_upsert,
        commands::identities_delete,
        commands::local_repos_list,
        commands::local_repos_set_private,
        commands::sinks_list,
        commands::sinks_add,
        commands::report_generate,
        commands::report_cancel,
        commands::report_get,
        commands::report_save,
        commands::retention_sweep_now,
        commands::activity_events_get,
        commands::shell_open,
        commands::gitlab_validate_pat,
        atlassian::atlassian_validate_credentials,
        atlassian::atlassian_sources_add,
        atlassian::atlassian_sources_reconnect,
        commands::dev_emit_toast,
        commands::dev_start_demo_run,
    ]);

    #[cfg(not(feature = "dev-commands"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::settings_get,
        commands::settings_update,
        commands::logs_tail,
        commands::persons_get_self,
        commands::persons_update_self,
        commands::sources_list,
        commands::sources_add,
        commands::sources_update,
        commands::sources_delete,
        commands::sources_healthcheck,
        commands::identities_list_for,
        commands::identities_upsert,
        commands::identities_delete,
        commands::local_repos_list,
        commands::local_repos_set_private,
        commands::sinks_list,
        commands::sinks_add,
        commands::report_generate,
        commands::report_cancel,
        commands::report_get,
        commands::report_save,
        commands::retention_sweep_now,
        commands::activity_events_get,
        commands::shell_open,
        commands::gitlab_validate_pat,
        atlassian::atlassian_validate_credentials,
        atlassian::atlassian_sources_add,
        atlassian::atlassian_sources_reconnect,
    ]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running the Dayseam desktop app");
}
