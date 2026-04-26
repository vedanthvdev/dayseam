#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use dayseam_db::LogRepo;
use dayseam_desktop::ipc::{
    atlassian, broadcast_forwarder, commands, github, oauth, outlook, scheduler,
};
use dayseam_desktop::state::AppState;
use dayseam_desktop::{scheduler_task, startup, tracing_init};
use tauri::menu::{AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

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
        // DAY-149. Intercept the main window's close event so the
        // user's `Cmd+W` / red-traffic-light click keeps the Tauri
        // process (and therefore the `scheduler_task` background
        // loop that promises a 6pm report even if the window was
        // closed at 9am) alive. The close policy is read cheaply
        // from an `AtomicBool` mirrored onto `AppState` by
        // `settings_update` and seeded at boot by
        // `startup::build_app_state`, so the handler never has to
        // touch SQLite on the Tauri main thread. Auxiliary windows
        // (e.g. a future detached preferences window) are not
        // intercepted here — only the `main` label is — so closing
        // a child panel still closes that panel without affecting
        // the app lifecycle.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                let keep_running = window
                    .app_handle()
                    .state::<AppState>()
                    .should_keep_running_when_window_closed();
                if keep_running {
                    // `prevent_close` has to come before `hide` so
                    // Tauri doesn't tear down the webview between
                    // the hide call and the next event loop tick —
                    // the ordering is what lets the window re-appear
                    // instantly when `RunEvent::Reopen` (Dock click
                    // on macOS) fires.
                    api.prevent_close();
                    let _ = window.hide();
                    tracing::info!(
                        "window close intercepted: app staying in background for scheduler"
                    );
                }
                // When `keep_running` is false, fall through — Tauri
                // closes the window and, because this is the only
                // application window, the process exits naturally.
                // That matches the pre-DAY-149 behaviour an opt-out
                // user explicitly asked for.
            }
        })
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

            // DAY-149: build the menu-bar tray icon so a user who
            // hid the main window via Cmd+W has an always-visible
            // affordance to bring it back (or quit the app
            // entirely). The tray is the only place the
            // hide-on-close behaviour becomes discoverable: without
            // it, a user who hid the window and then wants to
            // re-open has to either click the Dock icon (which we
            // handle via `RunEvent::Reopen` below) or know about
            // the running process. Building the tray is
            // best-effort — on platforms where the feature is
            // unavailable (headless CI, some Linux sessions without
            // an AppIndicator host) `build` returns an error; we
            // log and continue rather than fail boot, because the
            // close-on-hide behaviour itself still works and the
            // user can always relaunch from Launchpad.
            let tray_show = MenuItemBuilder::with_id("tray_show", "Show Dayseam").build(app)?;
            let tray_quit = MenuItemBuilder::with_id("tray_quit", "Quit Dayseam").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .items(&[&tray_show])
                .separator()
                .items(&[&tray_quit])
                .build()?;
            let tray_show_id = tray_show.id().clone();
            let tray_quit_id = tray_quit.id().clone();
            // DAY-170: ship a coloured, background-less tray PNG
            // instead of the full app-bundle icon. The bundle icon
            // is the rounded-charcoal "Convergence" square sized for
            // the Dock; rendering it in the menu bar drops a tiny
            // opaque box into the user's menu row that fights every
            // other item in the bar. The dedicated tray PNG is just
            // the five coloured strands + the dashed seam against a
            // transparent background, rasterised from
            // `assets/brand/dayseam-tray.svg` at 32x32 and 64x64 via
            // `scripts/rasterise-tray-icon.py`. The @2x variant
            // would be picked via `.icon_as_template(false)` + the
            // OS's usual retina handling, but `include_image!` only
            // embeds one raster at a time; Tauri will auto-use the
            // @2x sibling at runtime when the file sits next to the
            // base PNG with the `@2x` suffix.
            let tray_icon = tauri::include_image!("./icons/tray-icon.png");
            let tray_build = TrayIconBuilder::with_id("dayseam-main-tray")
                .icon(tray_icon)
                // Explicitly *not* a template image on macOS: the
                // mark is intentionally full-colour (brand strands),
                // so treating it as a single-ink template would
                // flatten every strand to the system foreground and
                // destroy the whole point of shipping a coloured
                // tray variant.
                .icon_as_template(false)
                .tooltip("Dayseam")
                .menu(&tray_menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app_handle, event| {
                    let id = event.id();
                    if id == &tray_show_id {
                        show_main_window(app_handle);
                    } else if id == &tray_quit_id {
                        // `app.exit(0)` starts a graceful shutdown:
                        // Tauri emits `RunEvent::ExitRequested` to
                        // any registered handler, closes windows,
                        // and then the process exits once every
                        // window is gone. The scheduler task is
                        // currently untracked on shutdown — that's
                        // a known limitation captured in the
                        // ARCHITECTURE.md background-execution
                        // section as a Tier C follow-up.
                        app_handle.exit(0);
                    }
                })
                .build(app);
            if let Err(err) = tray_build {
                tracing::warn!(
                    %err,
                    "tray icon unavailable; hide-on-close still works but no menu-bar affordance"
                );
            }

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
        oauth::oauth_begin_login,
        oauth::oauth_cancel_login,
        oauth::oauth_session_status,
        outlook::outlook_validate_credentials,
        outlook::outlook_sources_add,
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
        oauth::oauth_begin_login,
        oauth::oauth_cancel_login,
        oauth::oauth_session_status,
        outlook::outlook_validate_credentials,
        outlook::outlook_sources_add,
    ]);

    // DAY-149: switch from the one-shot `.run(context)` convenience
    // to `.build().run(callback)` so we can observe
    // `RunEvent::Reopen` — macOS's "user clicked the Dock icon of
    // a running app with no visible windows" signal. Without the
    // callback a user who hid the window via Cmd+W and then tries
    // to re-open by clicking the Dock icon would find the icon
    // bouncing but no window appearing, because macOS has no
    // default behaviour for reopen on a hidden app. The handler
    // shows and focuses the main window, exactly matching what a
    // well-behaved native macOS app does.
    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building the Dayseam desktop app");
    app.run(|app_handle, event| {
        if let RunEvent::Reopen {
            has_visible_windows,
            ..
        } = event
        {
            if !has_visible_windows {
                show_main_window(app_handle);
            }
        }
    });
}

/// DAY-149: shared helper used by both the tray "Show Dayseam" menu
/// item and the macOS `RunEvent::Reopen` dispatch. Guarantees the
/// main window ends up visible *and* focused — `show()` alone is
/// enough on a cold Dock click but doesn't re-take focus when the
/// window was merely hidden (not minimised), so both calls are needed
/// to give the user a consistent "click here, get a window" feel.
/// Silent on missing-window because the only path that hits this
/// helper is one where the window was created at startup; a missing
/// `"main"` label means something catastrophic happened at boot and
/// the earlier startup paths will have already surfaced it.
fn show_main_window<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        tracing::warn!("show_main_window: no 'main' webview window registered");
    }
}
