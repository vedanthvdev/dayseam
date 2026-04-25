// DAY-130. The single surface for every user-facing preference:
//
// - *View* — theme (Light / System / Dark). Reuses the existing
//   `ThemeToggle` styling so the radio group visually matches
//   every other dialog control.
// - *Scheduler* — master toggle, weekday chips, `target_time` +
//   `earliest_start` time inputs, catch-up days, and the sink
//   selector. Filtered to `MarkdownFile` sinks because that is the
//   only `safe_for_unattended` sink kind shipped today; the gate is
//   re-enforced server-side in `run_scheduled_action` so a
//   misconfigured sink still cannot fire.
//
// The dialog binds to a *local* copy of `ScheduleConfig` so the
// user can edit fields without touching SQLite on every keystroke.
// `Save` dispatches `scheduler_set_config` through the
// `useScheduler` hook; `Cancel` drops the draft and re-reads the
// persisted value on the next open.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { ScheduleConfig } from "@dayseam/ipc-types";
import { Dialog, DialogButton } from "../../components/Dialog";
import { useSettings, useSinks } from "../../ipc";
import { ThemeToggle } from "../../components/ThemeToggle";
import { useScheduler } from "../scheduler/useScheduler";

export interface PreferencesDialogProps {
  open: boolean;
  onClose: () => void;
}

/** Chrono's `Weekday` serde form is the three-letter English short
 *  name, so the frontend values must match bit-for-bit or the
 *  round-trip through `scheduler_set_config` rejects the payload. */
const WEEKDAYS: readonly { key: string; label: string }[] = [
  { key: "Mon", label: "Mon" },
  { key: "Tue", label: "Tue" },
  { key: "Wed", label: "Wed" },
  { key: "Thu", label: "Thu" },
  { key: "Fri", label: "Fri" },
  { key: "Sat", label: "Sat" },
  { key: "Sun", label: "Sun" },
];

/** Chrono serialises `NaiveTime` as `HH:MM:SS`; the native
 *  `<input type="time">` element emits `HH:MM`. Both directions run
 *  through these two shims so the config JSON always carries the
 *  three-segment form the Rust deserializer expects. */
function timeToInput(t: string): string {
  // Accepts "HH:MM" or "HH:MM:SS" and returns "HH:MM".
  return t.length >= 5 ? t.slice(0, 5) : t;
}

function inputToTime(v: string): string {
  if (/^\d{2}:\d{2}$/.test(v)) return `${v}:00`;
  return v;
}

export function PreferencesDialog({ open, onClose }: PreferencesDialogProps) {
  const scheduler = useScheduler();
  const { sinks, loading: sinksLoading } = useSinks();
  const {
    settings,
    loading: settingsLoading,
    error: settingsError,
    save: saveSettings,
  } = useSettings();

  // Local draft so the form doesn't mutate the persisted config on
  // every change. Reset to the stored value every time the dialog
  // opens so a cancelled edit leaves no trace.
  const [draft, setDraft] = useState<ScheduleConfig | null>(null);
  // DAY-149: local draft of the "keep the app running when the
  // main window closes" toggle. Loaded from the persisted
  // `Settings` blob on open, written back via `settings_update` on
  // Save. Kept in its own state — rather than merged into the
  // scheduler draft — because the underlying row is `Settings`,
  // not `ScheduleConfig`, and we don't want a scheduler-save
  // failure to also abandon a background-mode toggle change (or
  // vice versa).
  const [keepRunning, setKeepRunning] = useState<boolean | null>(null);
  const [savingKeepRunning, setSavingKeepRunning] = useState(false);

  useEffect(() => {
    if (open) setDraft(scheduler.config);
  }, [open, scheduler.config]);

  useEffect(() => {
    if (open && settings) {
      setKeepRunning(settings.keep_running_when_window_closed);
    }
  }, [open, settings]);

  const eligibleSinks = useMemo(
    () =>
      // Today the only `safe_for_unattended` sink kind is
      // `MarkdownFile`. The backend still re-checks the capability
      // bit before every write, so this UI filter is a
      // user-experience shortcut — it hides sinks the scheduler
      // would refuse to write to anyway.
      sinks.filter((s) => s.kind === "MarkdownFile"),
    [sinks],
  );

  const handleToggleDay = useCallback((day: string) => {
    setDraft((prev) => {
      if (!prev) return prev;
      const has = prev.days_of_week.includes(day);
      const next = has
        ? prev.days_of_week.filter((d) => d !== day)
        : [...prev.days_of_week, day];
      return { ...prev, days_of_week: next };
    });
  }, []);

  const handleSave = useCallback(async () => {
    if (!draft) return;
    try {
      // DAY-149: save the `Settings` patch *first*. If persisting
      // the background-mode toggle fails the user still gets a
      // "settings failed" error surfaced through `useSettings`'s
      // error channel, and the scheduler config is not touched
      // yet. Saving scheduler second means a successful
      // background-mode save can't be stranded by a scheduler
      // write failure either — `useScheduler.save` throws, we
      // skip `onClose`, and the dialog stays open for the retry.
      if (
        keepRunning !== null &&
        settings !== null &&
        keepRunning !== settings.keep_running_when_window_closed
      ) {
        setSavingKeepRunning(true);
        try {
          await saveSettings({ keep_running_when_window_closed: keepRunning });
        } finally {
          setSavingKeepRunning(false);
        }
      }
      await scheduler.save(draft);
      onClose();
    } catch {
      // Error is surfaced via `scheduler.error` or `settingsError`;
      // keep the dialog open so the user can retry or cancel.
    }
  }, [draft, scheduler, onClose, keepRunning, settings, saveSettings]);

  const canSave =
    draft !== null &&
    // `catch_up_days` is `u32` in Rust; refuse negatives and
    // anything larger than the 30-day hard cap to match the
    // backend's silent clamp.
    draft.catch_up_days >= 0 &&
    draft.catch_up_days <= 30 &&
    // A time picker that reports an empty string is still a
    // "pending" input on Firefox; block save until the user
    // actually picks a value.
    draft.target_time.length > 0 &&
    draft.earliest_start.length > 0 &&
    !scheduler.saving &&
    !savingKeepRunning;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Preferences"
      description="Appearance and scheduler settings. Changes apply immediately after Save."
      size="lg"
      testId="preferences-dialog"
      footer={
        <>
          <DialogButton kind="secondary" onClick={onClose}>
            Cancel
          </DialogButton>
          <DialogButton
            kind="primary"
            disabled={!canSave}
            onClick={() => void handleSave()}
          >
            {scheduler.saving ? "Saving…" : "Save"}
          </DialogButton>
        </>
      }
    >
      <div className="flex flex-col gap-6">
        <section aria-labelledby="preferences-view-heading" className="flex flex-col gap-2">
          <h3
            id="preferences-view-heading"
            className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400"
          >
            View
          </h3>
          {/* DAY-130 F-1: a `<label>` wrapping a radiogroup with
              multiple buttons breaks the W3C accname algorithm —
              only the last radio ends up with an accessible name.
              Use a plain `<div>` so each option keeps its own
              "Light" / "System" / "Dark" name. */}
          <div className="flex items-center justify-between gap-3">
            <span
              id="preferences-view-theme-label"
              className="text-sm text-neutral-800 dark:text-neutral-200"
            >
              Theme
            </span>
            <ThemeToggle />
          </div>
          <p className="text-[11px] text-neutral-500 dark:text-neutral-400">
            Also available from the native <em>View &gt; Theme</em> menu.
          </p>
        </section>

        {/* DAY-149: Background-execution section. Lives above
            Scheduler because the value it controls is a precondition
            for the scheduler promise ("my daily report fires at 6pm
            even if I closed the window at 9am") — a user who
            disables this here should see the scheduler section
            immediately below to understand the knock-on effect. */}
        <section
          aria-labelledby="preferences-background-heading"
          className="flex flex-col gap-2 border-t border-neutral-200 pt-4 dark:border-neutral-800"
        >
          <h3
            id="preferences-background-heading"
            className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400"
          >
            Background
          </h3>
          {settingsLoading && keepRunning === null ? (
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              Loading background settings…
            </p>
          ) : (
            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                checked={keepRunning ?? true}
                onChange={(e) => setKeepRunning(e.target.checked)}
                data-testid="preferences-keep-running-when-window-closed"
                className="mt-0.5"
              />
              <span className="flex flex-col gap-0.5">
                <span className="text-neutral-800 dark:text-neutral-200">
                  Keep Dayseam running in the background when I close the window
                </span>
                <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
                  Leaves the app in the Dock and menu bar so scheduled reports
                  still run. Quit Dayseam from the menu bar icon or the app
                  menu when you really want to exit. Turn this off to have
                  closing the window quit the app instead.
                </span>
              </span>
            </label>
          )}
          {settingsError ? (
            <p
              role="alert"
              className="rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
            >
              {settingsError}
            </p>
          ) : null}
        </section>

        <section
          aria-labelledby="preferences-scheduler-heading"
          className="flex flex-col gap-3 border-t border-neutral-200 pt-4 dark:border-neutral-800"
        >
          <h3
            id="preferences-scheduler-heading"
            className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400"
          >
            Scheduler
          </h3>

          {!draft ? (
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              Loading scheduler configuration…
            </p>
          ) : (
            <>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={draft.enabled}
                  onChange={(e) =>
                    setDraft({ ...draft, enabled: e.target.checked })
                  }
                  data-testid="preferences-scheduler-enabled"
                />
                <span className="text-neutral-800 dark:text-neutral-200">
                  Enable automatic daily reports
                </span>
              </label>

              <div
                aria-labelledby="preferences-scheduler-days-label"
                className="flex flex-col gap-1"
              >
                <span
                  id="preferences-scheduler-days-label"
                  className="text-[11px] text-neutral-600 dark:text-neutral-400"
                >
                  Days of the week
                </span>
                <div
                  role="group"
                  aria-labelledby="preferences-scheduler-days-label"
                  className="flex flex-wrap gap-1"
                >
                  {WEEKDAYS.map((day) => {
                    const isOn = draft.days_of_week.includes(day.key);
                    return (
                      <button
                        key={day.key}
                        type="button"
                        role="checkbox"
                        aria-checked={isOn}
                        disabled={!draft.enabled}
                        onClick={() => handleToggleDay(day.key)}
                        data-testid={`preferences-scheduler-day-${day.key}`}
                        className={
                          "inline-flex items-center rounded border px-2 py-0.5 text-xs transition disabled:cursor-not-allowed disabled:opacity-40 " +
                          (isOn
                            ? "border-neutral-700 bg-neutral-900 text-white dark:border-neutral-300 dark:bg-neutral-100 dark:text-neutral-900"
                            : "border-neutral-300 text-neutral-600 hover:bg-neutral-100 dark:border-neutral-700 dark:text-neutral-300 dark:hover:bg-neutral-800")
                        }
                      >
                        {day.label}
                      </button>
                    );
                  })}
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <label className="flex flex-col gap-1">
                  <span className="text-[11px] text-neutral-600 dark:text-neutral-400">
                    Target time
                  </span>
                  <input
                    type="time"
                    value={timeToInput(draft.target_time)}
                    disabled={!draft.enabled}
                    onChange={(e) =>
                      setDraft({
                        ...draft,
                        target_time: inputToTime(e.target.value),
                      })
                    }
                    data-testid="preferences-scheduler-target-time"
                    className="rounded border border-neutral-300 bg-white px-2 py-0.5 text-xs text-neutral-800 disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200"
                  />
                </label>
                <label className="flex flex-col gap-1">
                  <span className="text-[11px] text-neutral-600 dark:text-neutral-400">
                    Earliest start
                  </span>
                  <input
                    type="time"
                    value={timeToInput(draft.earliest_start)}
                    disabled={!draft.enabled}
                    onChange={(e) =>
                      setDraft({
                        ...draft,
                        earliest_start: inputToTime(e.target.value),
                      })
                    }
                    data-testid="preferences-scheduler-earliest-start"
                    className="rounded border border-neutral-300 bg-white px-2 py-0.5 text-xs text-neutral-800 disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200"
                  />
                </label>
              </div>

              <label className="flex flex-col gap-1">
                <span className="text-[11px] text-neutral-600 dark:text-neutral-400">
                  Catch up last N days on next open (0 disables)
                </span>
                <input
                  type="number"
                  min={0}
                  max={30}
                  value={draft.catch_up_days}
                  disabled={!draft.enabled}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      catch_up_days: Math.max(
                        0,
                        Math.min(30, Number.parseInt(e.target.value, 10) || 0),
                      ),
                    })
                  }
                  data-testid="preferences-scheduler-catch-up-days"
                  className="w-24 rounded border border-neutral-300 bg-white px-2 py-0.5 text-xs text-neutral-800 disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200"
                />
              </label>

              <label className="flex flex-col gap-1">
                <span className="text-[11px] text-neutral-600 dark:text-neutral-400">
                  Sink
                </span>
                {sinksLoading ? (
                  <span className="text-xs text-neutral-500 dark:text-neutral-400">
                    Loading sinks…
                  </span>
                ) : eligibleSinks.length === 0 ? (
                  <p className="rounded border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-900 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200">
                    No sink is configured that supports unattended writes.
                    Add a markdown sink from the <em>Sinks</em> dialog, then
                    pick it here.
                  </p>
                ) : (
                  <select
                    value={draft.sink_id ?? ""}
                    disabled={!draft.enabled}
                    onChange={(e) =>
                      setDraft({
                        ...draft,
                        sink_id: e.target.value.length === 0 ? null : e.target.value,
                      })
                    }
                    data-testid="preferences-scheduler-sink"
                    className="rounded border border-neutral-300 bg-white px-2 py-0.5 text-xs text-neutral-800 disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200"
                  >
                    <option value="">— Pick a sink —</option>
                    {eligibleSinks.map((sink) => (
                      <option key={sink.id} value={sink.id}>
                        {sink.label}
                      </option>
                    ))}
                  </select>
                )}
              </label>
            </>
          )}

          {scheduler.error ? (
            <p
              role="alert"
              className="rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
            >
              {scheduler.error}
            </p>
          ) : null}
        </section>
      </div>
    </Dialog>
  );
}
