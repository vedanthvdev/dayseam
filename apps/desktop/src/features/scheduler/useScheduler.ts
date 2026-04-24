// DAY-130. Single React binding for everything the scheduler UI
// needs: read + write the persisted `ScheduleConfig`, surface the
// cold-start catch-up banner, and wire the banner's Run / Skip
// buttons back to the IPC surface.
//
// The hook keeps a *local* copy of `ScheduleConfig` so the
// Preferences form can bind directly to its fields without
// round-tripping to SQLite on every keystroke; the committed value
// only lands once `save(config)` is called. Banner state lives next
// to it so any surface can render the prompt without re-wiring the
// event listener.
//
// The `scheduler:catch-up-suggested` event is emitted by the Rust
// background tick (and by the cold-start scan inside
// `scheduler_task::spawn`) with a payload of ISO `YYYY-MM-DD`
// strings. We keep the payload as strings rather than `Date`
// objects so the same value can round-trip back into the
// `scheduler_run_catch_up` / `scheduler_skip_catch_up` IPCs without
// TZ guessing.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { ScheduleConfig } from "@dayseam/ipc-types";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "../../ipc";

/** Event name the Rust side emits whenever it plans a `CatchUp`
 *  action. Shared by `SchedulerCatchUpBanner` so banner mounts and
 *  hook state stay pinned to the same string. */
export const SCHEDULER_CATCH_UP_EVENT = "scheduler:catch-up-suggested";

/** Event name the native *Dayseam > Preferences…* menu item emits.
 *  Exported so `App` can listen and open the dialog — keeps the
 *  scheduler + preferences plumbing co-located with the other
 *  scheduler strings. */
export const OPEN_PREFERENCES_EVENT = "menu://open-preferences";

/** Payload the Rust tick emits with the list of missed dates. The
 *  backend emits a bare `string[]` (see `scheduler_task::tick`); we
 *  keep the type alias explicit so a future edit that wraps the
 *  payload in an object is a compile error here rather than a
 *  silent banner-never-fires regression. */
type CatchUpEventPayload = string[];

export interface UseSchedulerState {
  /** `null` while the first `scheduler_get_config` call is in
   *  flight. Every successful load replaces this atomically so the
   *  Preferences form can bind to the same object reference. */
  config: ScheduleConfig | null;
  /** Reflects an in-flight `scheduler_set_config` so the Save
   *  button can show a spinner. */
  saving: boolean;
  /** Surfaces the most recent load or save failure for inline
   *  error rendering. Cleared on the next successful call. */
  error: string | null;
  /** ISO `YYYY-MM-DD` strings for catch-up candidates. Empty when
   *  no banner should show. Ordered oldest-first to match the
   *  planner's own ordering. */
  pendingCatchUp: string[];
  /** Commit a new config to disk. Mutates `config` on success so
   *  the dialog reflects the stored shape (not the user's local
   *  edits). */
  save: (next: ScheduleConfig) => Promise<void>;
  /** Run the pending catch-up batch and clear the banner. */
  runCatchUp: () => Promise<void>;
  /** Dismiss the banner for this session. */
  skipCatchUp: () => Promise<void>;
}

export function useScheduler(): UseSchedulerState {
  const [config, setConfig] = useState<ScheduleConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingCatchUp, setPendingCatchUp] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    invoke("scheduler_get_config", {})
      .then((cfg) => {
        if (!cancelled) setConfig(cfg);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : JSON.stringify(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<CatchUpEventPayload>(SCHEDULER_CATCH_UP_EVENT, (event) => {
      const dates = event.payload;
      if (!Array.isArray(dates)) return;
      // The planner already emits oldest-first but we don't take
      // that on faith — if a future Rust edit reverses the order
      // the banner copy ("from Mon 14 …") would flip nonsensically.
      const sorted = [...dates].filter((d) => typeof d === "string").sort();
      setPendingCatchUp(sorted);
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {
        // No Tauri bridge in the test harness; the banner simply
        // never shows, which matches the production "no missed
        // days" path.
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const save = useCallback(async (next: ScheduleConfig) => {
    setSaving(true);
    setError(null);
    try {
      const stored = await invoke("scheduler_set_config", { config: next });
      setConfig(stored);
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
      throw err;
    } finally {
      setSaving(false);
    }
  }, []);

  const runCatchUp = useCallback(async () => {
    const dates = pendingCatchUp;
    if (dates.length === 0) return;
    setError(null);
    try {
      await invoke("scheduler_run_catch_up", { dates });
      setPendingCatchUp([]);
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
      throw err;
    }
  }, [pendingCatchUp]);

  const skipCatchUp = useCallback(async () => {
    const dates = pendingCatchUp;
    if (dates.length === 0) return;
    setError(null);
    try {
      await invoke("scheduler_skip_catch_up", { dates });
      setPendingCatchUp([]);
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
      throw err;
    }
  }, [pendingCatchUp]);

  return useMemo(
    () => ({
      config,
      saving,
      error,
      pendingCatchUp,
      save,
      runCatchUp,
      skipCatchUp,
    }),
    [config, saving, error, pendingCatchUp, save, runCatchUp, skipCatchUp],
  );
}
