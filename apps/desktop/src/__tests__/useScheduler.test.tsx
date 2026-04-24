// DAY-130. The hook that binds the Preferences form and the
// catch-up banner to the Rust side. These tests lock:
//
// 1. The initial `scheduler_get_config` call populates `config`.
// 2. `save(next)` round-trips through `scheduler_set_config` and
//    updates `config` to the backend's canonical echo.
// 3. An inbound `scheduler:catch-up-suggested` event is sorted and
//    stashed on `pendingCatchUp`; Run / Skip call the right IPC and
//    clear the banner state.

import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ScheduleConfig } from "@dayseam/ipc-types";
import {
  SCHEDULER_CATCH_UP_EVENT,
  useScheduler,
} from "../features/scheduler/useScheduler";
import {
  emitEvent,
  mockInvoke,
  registerInvokeHandler,
  resetTauriMocks,
} from "./tauri-mock";

const BASE_CFG: ScheduleConfig = {
  enabled: false,
  days_of_week: [],
  target_time: "18:00:00",
  earliest_start: "12:00:00",
  catch_up_days: 7,
  sink_id: null,
  template_id: null,
};

describe("useScheduler", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerInvokeHandler("scheduler_get_config", async () => BASE_CFG);
    registerInvokeHandler(
      "scheduler_set_config",
      async (args) => (args as { config: ScheduleConfig }).config,
    );
    registerInvokeHandler("scheduler_run_catch_up", async () => []);
    registerInvokeHandler("scheduler_skip_catch_up", async () => null);
  });

  afterEach(() => {
    resetTauriMocks();
  });

  it("loads the stored config on mount", async () => {
    const { result } = renderHook(() => useScheduler());
    await waitFor(() => expect(result.current.config).not.toBeNull());
    expect(result.current.config).toEqual(BASE_CFG);
    expect(result.current.error).toBeNull();
    expect(result.current.saving).toBe(false);
  });

  it("round-trips save() through scheduler_set_config", async () => {
    const { result } = renderHook(() => useScheduler());
    await waitFor(() => expect(result.current.config).not.toBeNull());

    const next: ScheduleConfig = {
      ...BASE_CFG,
      enabled: true,
      days_of_week: ["Mon", "Tue"],
    };
    await act(async () => {
      await result.current.save(next);
    });
    expect(result.current.config).toEqual(next);
    expect(mockInvoke).toHaveBeenCalledWith("scheduler_set_config", {
      config: next,
    });
  });

  it("captures catch-up events (sorted oldest-first) and wires Run to the right IPC", async () => {
    const { result } = renderHook(() => useScheduler());
    await waitFor(() => expect(result.current.config).not.toBeNull());

    // Payload arrives newest-first; the hook must sort oldest-first
    // so the banner copy reads correctly.
    await act(async () => {
      emitEvent(SCHEDULER_CATCH_UP_EVENT, ["2026-04-21", "2026-04-19"]);
    });
    await waitFor(() =>
      expect(result.current.pendingCatchUp).toEqual([
        "2026-04-19",
        "2026-04-21",
      ]),
    );

    await act(async () => {
      await result.current.runCatchUp();
    });
    expect(mockInvoke).toHaveBeenCalledWith("scheduler_run_catch_up", {
      dates: ["2026-04-19", "2026-04-21"],
    });
    expect(result.current.pendingCatchUp).toEqual([]);
  });

  it("skipCatchUp dispatches scheduler_skip_catch_up and clears the banner", async () => {
    const { result } = renderHook(() => useScheduler());
    await waitFor(() => expect(result.current.config).not.toBeNull());

    await act(async () => {
      emitEvent(SCHEDULER_CATCH_UP_EVENT, ["2026-04-19"]);
    });
    await waitFor(() => expect(result.current.pendingCatchUp).toEqual(["2026-04-19"]));

    await act(async () => {
      await result.current.skipCatchUp();
    });
    expect(mockInvoke).toHaveBeenCalledWith("scheduler_skip_catch_up", {
      dates: ["2026-04-19"],
    });
    expect(result.current.pendingCatchUp).toEqual([]);
  });

  it("ignores non-array payloads so a malformed event can't brick the banner", async () => {
    const { result } = renderHook(() => useScheduler());
    await waitFor(() => expect(result.current.config).not.toBeNull());

    await act(async () => {
      emitEvent(SCHEDULER_CATCH_UP_EVENT, { dates: ["2026-04-19"] });
    });
    // A future Rust refactor that wraps the payload in an object
    // should trip this guard rather than silently render.
    expect(result.current.pendingCatchUp).toEqual([]);
  });
});
