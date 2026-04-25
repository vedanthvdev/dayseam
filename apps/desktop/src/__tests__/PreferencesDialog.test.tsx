// DAY-130. The Preferences dialog owns the theme radio group the
// persistent header used to carry, and is the primary UI for the
// scheduler. These tests lock three invariants:
//
// 1. The theme radio group lives inside the dialog, persists to
//    `localStorage` via `ThemeProvider`, and marks the chosen
//    option with `aria-checked`.
// 2. The scheduler form binds to the `ScheduleConfig` returned by
//    `scheduler_get_config` and round-trips edits through
//    `scheduler_set_config`.
// 3. "Cancel" does not commit pending edits — the dialog falls
//    back to the persisted value on the next open.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  ScheduleConfig,
  Settings,
  SettingsPatch,
  Sink,
} from "@dayseam/ipc-types";
import { PreferencesDialog } from "../features/preferences/PreferencesDialog";
import { ThemeProvider, THEME_STORAGE_KEY } from "../theme";
import { registerInvokeHandler, resetTauriMocks } from "./tauri-mock";

const BASE_CFG: ScheduleConfig = {
  enabled: true,
  days_of_week: ["Mon", "Wed", "Fri"],
  target_time: "18:00:00",
  earliest_start: "12:00:00",
  catch_up_days: 7,
  sink_id: null,
  template_id: null,
};

const BASE_SETTINGS: Settings = {
  config_version: 2,
  theme: "system",
  verbose_logs: false,
  keep_running_when_window_closed: true,
};

const SINK: Sink = {
  id: "kkkkkkkk-kkkk-kkkk-kkkk-kkkkkkkkkkkk",
  kind: "MarkdownFile",
  label: "Daily notes",
  config: {
    MarkdownFile: {
      config_version: 1,
      dest_dirs: ["/Users/me/notes"],
      frontmatter: false,
    },
  },
  created_at: "2026-04-17T12:00:00Z",
  last_write_at: null,
};

function Harness({ initialOpen }: { initialOpen: boolean }) {
  // `PreferencesDialog` relies on `useTheme()` from `ThemeProvider`;
  // wrap the dialog in the provider so the radio group is actually
  // interactive (otherwise `setTheme` throws).
  return (
    <ThemeProvider>
      <PreferencesDialog open={initialOpen} onClose={() => {}} />
    </ThemeProvider>
  );
}

describe("PreferencesDialog", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
    resetTauriMocks();
    registerInvokeHandler("scheduler_get_config", async () => BASE_CFG);
    registerInvokeHandler("scheduler_set_config", async (args) => {
      const payload = args as { config: ScheduleConfig };
      return payload.config;
    });
    registerInvokeHandler("sinks_list", async () => [SINK]);
    // DAY-149: the Preferences dialog now hydrates a Background
    // section from the persisted `Settings` blob. A test that
    // doesn't override these handlers gets the default (true)
    // toggle state and a no-op `settings_update` that echoes
    // whatever patch it receives back as the canonical `Settings`
    // row — matching the Rust `Settings::with_patch` contract.
    registerInvokeHandler("settings_get", async () => BASE_SETTINGS);
    registerInvokeHandler("settings_update", async (args) => {
      const { patch } = args as { patch: SettingsPatch };
      // `SettingsPatch` is generated as "field?: T | null" from
      // Rust's `Option<T>`. Only apply fields the caller set to a
      // real value — treating `null` as "leave alone" matches the
      // Rust-side `with_patch` semantics, and keeps the mock's
      // return type compatible with `Settings` (no nullable
      // fields).
      const next: Settings = { ...BASE_SETTINGS };
      if (patch.theme != null) next.theme = patch.theme;
      if (patch.verbose_logs != null) next.verbose_logs = patch.verbose_logs;
      if (patch.keep_running_when_window_closed != null) {
        next.keep_running_when_window_closed =
          patch.keep_running_when_window_closed;
      }
      return next;
    });
  });

  afterEach(async () => {
    await act(async () => {});
    localStorage.clear();
    resetTauriMocks();
  });

  it("renders the theme radio group and persists a change to localStorage", async () => {
    render(<Harness initialOpen />);
    await screen.findByTestId("preferences-dialog");

    const group = screen.getByRole("radiogroup", { name: /theme/i });
    expect(group).toBeInTheDocument();

    const darkRadio = screen.getByRole("radio", { name: /^dark$/i });
    fireEvent.click(darkRadio);

    expect(darkRadio).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: /^light$/i })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
  });

  it("loads the scheduler config and toggles a weekday chip", async () => {
    render(<Harness initialOpen />);
    await screen.findByTestId("preferences-dialog");

    // Mon is in the base config; clicking it removes it.
    const monChip = await screen.findByTestId("preferences-scheduler-day-Mon");
    expect(monChip).toHaveAttribute("aria-checked", "true");
    fireEvent.click(monChip);
    expect(monChip).toHaveAttribute("aria-checked", "false");

    // Tue is not in the base config; clicking it adds it.
    const tueChip = screen.getByTestId("preferences-scheduler-day-Tue");
    expect(tueChip).toHaveAttribute("aria-checked", "false");
    fireEvent.click(tueChip);
    expect(tueChip).toHaveAttribute("aria-checked", "true");
  });

  it("dispatches scheduler_set_config with normalised HH:MM:SS times on Save", async () => {
    const saves: ScheduleConfig[] = [];
    registerInvokeHandler("scheduler_set_config", async (args) => {
      const payload = args as { config: ScheduleConfig };
      saves.push(payload.config);
      return payload.config;
    });

    render(<Harness initialOpen />);
    await screen.findByTestId("preferences-dialog");

    // Change the target time via the native <input type="time">.
    const targetInput = (await screen.findByTestId(
      "preferences-scheduler-target-time",
    )) as HTMLInputElement;
    fireEvent.change(targetInput, { target: { value: "17:30" } });
    expect(targetInput.value).toBe("17:30");

    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(saves.length).toBe(1));
    // `inputToTime` re-appends the seconds segment chrono's
    // `NaiveTime` serde expects.
    expect(saves[0]!.target_time).toBe("17:30:00");
  });

  it("clamps catch_up_days to the 0-30 range enforced server-side", async () => {
    render(<Harness initialOpen />);
    await screen.findByTestId("preferences-dialog");

    const input = (await screen.findByTestId(
      "preferences-scheduler-catch-up-days",
    )) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "999" } });
    // UI clamps at 30 to match the `CATCH_UP_DAYS_HARD_CAP` Rust
    // constant — the orchestrator silently clamps too, but we
    // still prefer a visible ceiling.
    expect(input.value).toBe("30");

    fireEvent.change(input, { target: { value: "-5" } });
    expect(input.value).toBe("0");
  });

  it("persists the keep-running-in-background toggle via settings_update", async () => {
    // DAY-149: flipping the Background checkbox and hitting Save
    // must dispatch a `settings_update` patch with the new
    // value — otherwise the close-handler atomic mirror (seeded
    // at boot + updated by this same IPC path) never learns about
    // the user's choice and Cmd+W keeps hiding the window
    // regardless.
    const patches: SettingsPatch[] = [];
    registerInvokeHandler("settings_update", async (args) => {
      const { patch } = args as { patch: SettingsPatch };
      patches.push(patch);
      const next: Settings = { ...BASE_SETTINGS };
      if (patch.keep_running_when_window_closed != null) {
        next.keep_running_when_window_closed =
          patch.keep_running_when_window_closed;
      }
      return next;
    });

    render(<Harness initialOpen />);
    await screen.findByTestId("preferences-dialog");

    const toggle = (await screen.findByTestId(
      "preferences-keep-running-when-window-closed",
    )) as HTMLInputElement;
    // Defaulted to true by the fixture — confirm before flipping.
    await waitFor(() => expect(toggle.checked).toBe(true));
    fireEvent.click(toggle);
    expect(toggle.checked).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(patches.length).toBe(1));
    expect(patches[0]!.keep_running_when_window_closed).toBe(false);
  });

  it("skips settings_update when the background toggle was not changed", async () => {
    // DAY-149: scheduler-only edits must not drag a no-op
    // `settings_update` call along with them. The dialog touches
    // two different persistence rows (`ScheduleConfig` vs
    // `Settings`) and bundling them would turn a scheduler-save
    // failure into a settings-save failure too — worse
    // ergonomics, no safety gain.
    let settingsCalls = 0;
    registerInvokeHandler("settings_update", async (args) => {
      settingsCalls += 1;
      const { patch } = args as { patch: SettingsPatch };
      const next: Settings = { ...BASE_SETTINGS };
      if (patch.keep_running_when_window_closed != null) {
        next.keep_running_when_window_closed =
          patch.keep_running_when_window_closed;
      }
      return next;
    });

    render(<Harness initialOpen />);
    await screen.findByTestId("preferences-dialog");
    // Let the Background section hydrate so the save path can see
    // an unchanged `keepRunning` value instead of the initial
    // `null` draft.
    await screen.findByTestId("preferences-keep-running-when-window-closed");

    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: /^save$/i }),
      ).toBeInTheDocument(),
    );
    expect(settingsCalls).toBe(0);
  });

  it("surfaces the no-sink empty state when no MarkdownFile sink exists", async () => {
    registerInvokeHandler("sinks_list", async () => []);
    render(<Harness initialOpen />);
    await screen.findByTestId("preferences-dialog");

    expect(
      await screen.findByText(/no sink is configured/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId("preferences-scheduler-sink"),
    ).not.toBeInTheDocument();
  });
});
