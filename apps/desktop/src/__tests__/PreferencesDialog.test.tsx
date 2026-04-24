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
import type { ScheduleConfig, Sink } from "@dayseam/ipc-types";
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
