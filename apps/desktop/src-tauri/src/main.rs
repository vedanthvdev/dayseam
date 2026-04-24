#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use dayseam_db::LogRepo;
use dayseam_desktop::ipc::{atlassian, broadcast_forwarder, commands, github, scheduler};
use dayseam_desktop::{scheduler_task, startup, tracing_init};
use tauri::menu::{AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, Manager};

fn main() {
    // DAY-115 / SF-1. Install the `tracing` subscriber *before*
    // anything else can log. `supervised_spawn`'s panic-capture
    // `error!` call (DAY-113), the broadcast forwarder, every IPC
    // command, and every plugin all route through this dispatcher;
    // if it is not set they write to a null subscriber and the
    // supervisor looks like it swallowed panics silently — exactly
    // the F-10 regression DAY-113 was supposed to fix.
    tracing_init::init();

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
        // DAY-108 in-app updater. The plugin verifies every download
        // against `plugins.updater.pubkey` in `tauri.conf.json`
        // before swapping the `.app` bundle; the matching
        // `updater:allow-check` / `updater:allow-download-and-install`
        // permissions live in `capabilities/updater.json` so the
        // production surface can be audited in a single file.
        .plugin(tauri_plugin_updater::Builder::new().build())
        // DAY-108. Paired with the updater: `install()` on macOS
        // replaces the `.app` in place but does not relaunch the
        // running process, so `useUpdater` calls `relaunch()` from
        // `@tauri-apps/plugin-process` after install. Grants the
        // single `process:allow-relaunch` permission; `exit` stays
        // denied so a malicious page can't force-quit the app.
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let data_dir = startup::default_data_dir();
            let state = tauri::async_runtime::block_on(startup::build_app_state(&data_dir))
                .expect("build AppState");
            let pool = state.pool.clone();
            let app_bus = state.app_bus.clone();
            app.manage(state);

            let handle = app.handle().clone();
            let logs = LogRepo::new(pool);
            let _broadcast_task = broadcast_forwarder::spawn(handle.clone(), app_bus, logs);

            // DAY-130: spawn the scheduler background loop after
            // `app.manage(state)` so the task's `AppHandle::state()`
            // lookups find the live `AppState`. The first tick fires
            // immediately so a cold boot surfaces the catch-up
            // banner on first paint instead of popping up an hour
            // later.
            scheduler_task::spawn(handle);

            // DAY-119: install a native application menu. The macOS
            // build used to ship with Tauri's default window menu
            // (which has no app submenu at all), so the "Dayseam"
            // entry next to the Apple logo didn't exist and there was
            // nowhere to surface "Check for Updates…". Without an
            // explicit menu on macOS the standard Cmd+Q / Cmd+W /
            // clipboard shortcuts also rely on the webview's own
            // keybindings, which is fragile. Building the menu here
            // gives us (1) the OS-native "Dayseam" submenu, (2) a
            // custom "Check for Updates…" item that emits
            // `menu://check-for-updates` so the frontend's
            // `useUpdater.check()` can re-run, and (3) standard Edit /
            // Window submenus so copy-paste and window management
            // behave like every other Mac app.
            let check_updates =
                MenuItemBuilder::with_id("check_for_updates", "Check for Updates…").build(app)?;
            // DAY-130: Preferences… under the Dayseam submenu. The
            // item emits `menu://open-preferences` which the
            // frontend listens for and opens `PreferencesDialog`.
            let preferences = MenuItemBuilder::with_id("open_preferences", "Preferences…")
                .accelerator("Cmd+,")
                .build(app)?;
            let about_metadata = AboutMetadataBuilder::new()
                .name(Some("Dayseam"))
                .version(Some(env!("CARGO_PKG_VERSION")))
                .copyright(Some("Local-first work reporting"))
                .build();
            let app_submenu = SubmenuBuilder::new(app, "Dayseam")
                .about(Some(about_metadata))
                .item(&check_updates)
                .separator()
                .item(&preferences)
                .separator()
                .services()
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .quit()
                .build()?;
            let edit_submenu = SubmenuBuilder::new(app, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            // DAY-130 part 1: native *View > Theme* submenu. Moving
            // the Light/System/Dark control out of the always-visible
            // header reclaims screen real estate for a setting users
            // touch once and forget. The menu items carry stable ids
            // `view_theme_{light,system,dark}`; selecting one emits
            // `view:set-theme` with the matching payload so the
            // existing `ThemeProvider.setTheme` stays the single
            // source of truth for theme state.
            let theme_light = MenuItemBuilder::with_id("view_theme_light", "Light").build(app)?;
            let theme_system =
                MenuItemBuilder::with_id("view_theme_system", "System").build(app)?;
            let theme_dark = MenuItemBuilder::with_id("view_theme_dark", "Dark").build(app)?;
            let theme_submenu = SubmenuBuilder::new(app, "Theme")
                .item(&theme_light)
                .item(&theme_system)
                .item(&theme_dark)
                .build()?;
            let view_submenu = SubmenuBuilder::new(app, "View")
                .item(&theme_submenu)
                .build()?;
            let window_submenu = SubmenuBuilder::new(app, "Window")
                .minimize()
                .maximize()
                .separator()
                .close_window()
                .build()?;
            let menu = MenuBuilder::new(app)
                .items(&[&app_submenu, &edit_submenu, &view_submenu, &window_submenu])
                .build()?;
            app.set_menu(menu)?;

            let check_updates_id = check_updates.id().clone();
            let preferences_id = preferences.id().clone();
            let theme_light_id = theme_light.id().clone();
            let theme_system_id = theme_system.id().clone();
            let theme_dark_id = theme_dark.id().clone();
            app.on_menu_event(move |app_handle, event| {
                let id = event.id();
                if id == &check_updates_id {
                    // The frontend's `useUpdater` hook listens for
                    // this event and calls `check()` — which drives
                    // the same code path the automatic check on
                    // mount uses. We intentionally do NOT perform
                    // the update check in Rust here because the UI
                    // state machine (idle → checking → available →
                    // downloading → ready) lives entirely on the JS
                    // side; emitting keeps a single source of truth
                    // for updater status.
                    let _ = app_handle.emit("menu://check-for-updates", ());
                } else if id == &preferences_id {
                    let _ = app_handle.emit("menu://open-preferences", ());
                } else if id == &theme_light_id {
                    let _ = app_handle.emit("view:set-theme", "light");
                } else if id == &theme_system_id {
                    let _ = app_handle.emit("view:set-theme", "system");
                } else if id == &theme_dark_id {
                    let _ = app_handle.emit("view:set-theme", "dark");
                }
            });

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
        github::github_validate_credentials,
        github::github_sources_add,
        github::github_sources_reconnect,
        scheduler::scheduler_get_config,
        scheduler::scheduler_set_config,
        scheduler::scheduler_run_catch_up,
        scheduler::scheduler_skip_catch_up,
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
        github::github_validate_credentials,
        github::github_sources_add,
        github::github_sources_reconnect,
        scheduler::scheduler_get_config,
        scheduler::scheduler_set_config,
        scheduler::scheduler_run_catch_up,
        scheduler::scheduler_skip_catch_up,
    ]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running the Dayseam desktop app");
}
