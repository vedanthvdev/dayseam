//! User-adjustable application settings.
//!
//! [`Settings`] is the canonical shape the frontend reads and the Rust
//! core persists. [`SettingsPatch`] is the partial-update counterpart
//! used by the `settings_update` IPC command — every field is
//! optional, and only `Some` fields are applied. This keeps settings
//! round-trips explicit about *what* changed, so the backend never has
//! to guess whether a missing field means "leave alone" or "reset to
//! default".
//!
//! The shape is deliberately minimal for v0.1 — only the handful of
//! preferences the app actually needs before any connector lands. New
//! preferences are added by extending both structs (always via `Option`
//! on `SettingsPatch`) plus the migration in
//! [`Settings::with_patch`]. Every addition bumps
//! [`Settings::CONFIG_VERSION`] so a stored-settings rehydrator can
//! detect legacy shapes and migrate them forward.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Preferred visual theme. Mirrors the frontend `Theme` union — kept
/// server-side too so a hypothetical "reset to default" action has a
/// single source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}

impl Default for ThemePreference {
    fn default() -> Self {
        Self::System
    }
}

/// User-adjustable application settings. Persisted by the `settings`
/// repo under the single key `"app"` and surfaced to the frontend by
/// [`crate`]'s consumers via IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Settings {
    /// Monotonic schema version. Written by the Rust side on every
    /// save so a future migration can detect legacy shapes.
    pub config_version: u32,
    /// Visual theme preference.
    pub theme: ThemePreference,
    /// When true, the log drawer shows `Debug`-level rows. Off by
    /// default — Phase 1 still captures them into SQLite, the toggle
    /// only affects visibility.
    pub verbose_logs: bool,
    /// When true, closing the main window hides the app but keeps the
    /// process (and therefore the scheduler) running in the Dock so
    /// the 6pm report fires even if the user quit the window at 9am.
    /// When false, closing the last window quits the app — the
    /// pre-DAY-149 behaviour. Defaults to `true` for new installs and
    /// for migrated v1 rows so the scheduler promise on the marketing
    /// site holds without the user having to enable anything. Read
    /// cheaply from the window-close handler via an atomic mirror on
    /// [`crate::state::AppState`] in `dayseam-desktop`; that mirror
    /// is the authoritative value the handler reads, and this field
    /// is the persisted source of truth it was seeded from.
    #[serde(default = "default_keep_running_when_window_closed")]
    pub keep_running_when_window_closed: bool,
}

fn default_keep_running_when_window_closed() -> bool {
    true
}

impl Settings {
    /// Current schema version of the persisted settings blob. Bump
    /// whenever a field is added or reshaped so a future migration can
    /// tell legacy rows from current ones.
    ///
    /// Version history:
    ///
    /// * `1` — initial shape (`theme`, `verbose_logs`).
    /// * `2` — DAY-149: added
    ///   [`Settings::keep_running_when_window_closed`]. Legacy v1
    ///   JSON rehydrates with the new field defaulted to `true` via
    ///   [`default_keep_running_when_window_closed`] — no destructive
    ///   migration is required, and the next `with_patch` stamps
    ///   `config_version = 2` so the row stops looking legacy on
    ///   subsequent reads.
    pub const CONFIG_VERSION: u32 = 2;

    /// Apply every `Some(_)` field of `patch`, leave `None` fields
    /// untouched, and stamp `config_version` back to
    /// [`Self::CONFIG_VERSION`] so the stored shape always reflects
    /// the current schema.
    #[must_use]
    pub fn with_patch(mut self, patch: SettingsPatch) -> Self {
        if let Some(theme) = patch.theme {
            self.theme = theme;
        }
        if let Some(verbose) = patch.verbose_logs {
            self.verbose_logs = verbose;
        }
        if let Some(keep_running) = patch.keep_running_when_window_closed {
            self.keep_running_when_window_closed = keep_running;
        }
        self.config_version = Self::CONFIG_VERSION;
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            config_version: Self::CONFIG_VERSION,
            theme: ThemePreference::default(),
            verbose_logs: false,
            keep_running_when_window_closed: default_keep_running_when_window_closed(),
        }
    }
}

/// Partial update shape for [`Settings`]. Every field is optional so
/// the frontend can send only what the user changed; omitted fields
/// are explicitly "leave alone", not "reset to default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemePreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbose_logs: Option<bool>,
    /// DAY-149. When `Some(_)`, flips whether closing the main window
    /// hides the app (scheduler keeps running) or quits the process
    /// outright. The Preferences dialog surfaces this as a single
    /// checkbox; the IPC layer mirrors the resolved value onto an
    /// atomic on `AppState` so the window-close handler can read it
    /// synchronously without re-hitting SQLite on every close event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_running_when_window_closed: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_system_theme_quiet_logs_and_keep_running() {
        let s = Settings::default();
        assert_eq!(s.theme, ThemePreference::System);
        assert!(!s.verbose_logs);
        assert!(
            s.keep_running_when_window_closed,
            "DAY-149: default must be ON so the scheduler promise holds out of the box"
        );
        assert_eq!(s.config_version, Settings::CONFIG_VERSION);
    }

    #[test]
    fn patch_applies_only_provided_fields() {
        let base = Settings {
            config_version: Settings::CONFIG_VERSION,
            theme: ThemePreference::Light,
            verbose_logs: false,
            keep_running_when_window_closed: true,
        };
        let patched = base.clone().with_patch(SettingsPatch {
            theme: Some(ThemePreference::Dark),
            verbose_logs: None,
            keep_running_when_window_closed: None,
        });
        assert_eq!(patched.theme, ThemePreference::Dark);
        assert!(!patched.verbose_logs);
        assert!(
            patched.keep_running_when_window_closed,
            "None patch field must leave keep_running untouched"
        );
    }

    #[test]
    fn patch_can_flip_keep_running_off() {
        let base = Settings::default();
        let patched = base.with_patch(SettingsPatch {
            keep_running_when_window_closed: Some(false),
            ..SettingsPatch::default()
        });
        assert!(
            !patched.keep_running_when_window_closed,
            "DAY-149: explicit Some(false) must override the default-true"
        );
    }

    #[test]
    fn empty_patch_is_identity() {
        let base = Settings::default();
        let patched = base.clone().with_patch(SettingsPatch::default());
        assert_eq!(base, patched);
    }

    #[test]
    fn patch_always_stamps_current_config_version() {
        let stale = Settings {
            config_version: 0,
            theme: ThemePreference::System,
            verbose_logs: true,
            keep_running_when_window_closed: true,
        };
        let patched = stale.with_patch(SettingsPatch::default());
        assert_eq!(patched.config_version, Settings::CONFIG_VERSION);
    }

    #[test]
    fn v1_json_rehydrates_with_keep_running_defaulted_to_true() {
        // DAY-149: rows written by v0.7.0 and earlier don't carry
        // `keep_running_when_window_closed`. `serde(default)` on the
        // new field must treat the missing key as "true" so the
        // first launch after upgrading does not surprise users by
        // flipping to close-quits-the-app behaviour. The next write
        // stamps `config_version = 2`, at which point the row is no
        // longer legacy.
        let legacy = r#"{
            "config_version": 1,
            "theme": "system",
            "verbose_logs": false
        }"#;
        let parsed: Settings = serde_json::from_str(legacy).expect("legacy v1 shape parses");
        assert_eq!(parsed.config_version, 1);
        assert!(
            parsed.keep_running_when_window_closed,
            "v1 → v2 migration must default to the scheduler-friendly behaviour"
        );

        let restamped = parsed.with_patch(SettingsPatch::default());
        assert_eq!(
            restamped.config_version,
            Settings::CONFIG_VERSION,
            "touching a v1 row stamps the current schema version"
        );
    }
}
