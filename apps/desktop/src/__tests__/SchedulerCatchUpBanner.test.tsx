// DAY-130. `SchedulerCatchUpBanner` renders off the
// `UseSchedulerState` shape; here we hand it a hand-rolled stub so
// the component's behaviour (hide-when-empty, labels, Run / Skip
// wiring) is tested in isolation from the real hook.

import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SchedulerCatchUpBanner } from "../features/scheduler/SchedulerCatchUpBanner";
import type { UseSchedulerState } from "../features/scheduler/useScheduler";
import { resetTauriMocks } from "./tauri-mock";

function stubState(
  overrides: Partial<UseSchedulerState> = {},
): UseSchedulerState {
  return {
    config: null,
    saving: false,
    error: null,
    pendingCatchUp: [],
    save: vi.fn(async () => {}),
    runCatchUp: vi.fn(async () => {}),
    skipCatchUp: vi.fn(async () => {}),
    ...overrides,
  };
}

describe("SchedulerCatchUpBanner", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  afterEach(() => {
    resetTauriMocks();
  });

  it("renders nothing when the pending-catch-up list is empty", () => {
    render(<SchedulerCatchUpBanner scheduler={stubState()} />);
    expect(
      screen.queryByTestId("scheduler-catch-up-banner"),
    ).not.toBeInTheDocument();
  });

  it("shows a singular label when only one day is missed", () => {
    render(
      <SchedulerCatchUpBanner
        scheduler={stubState({ pendingCatchUp: ["2026-04-19"] })}
      />,
    );
    expect(screen.getByTestId("scheduler-catch-up-banner")).toHaveTextContent(
      /catch up 1 missed report/i,
    );
    expect(screen.getByTestId("scheduler-catch-up-banner")).toHaveTextContent(
      /2026-04-19/,
    );
  });

  it("shows a ranged label when multiple days are missed", () => {
    render(
      <SchedulerCatchUpBanner
        scheduler={stubState({
          pendingCatchUp: ["2026-04-17", "2026-04-19", "2026-04-21"],
        })}
      />,
    );
    const banner = screen.getByTestId("scheduler-catch-up-banner");
    expect(banner).toHaveTextContent(/catch up 3 missed reports/i);
    expect(banner).toHaveTextContent(/2026-04-17/);
    expect(banner).toHaveTextContent(/2026-04-21/);
  });

  it("wires the Run button to runCatchUp()", () => {
    const runCatchUp = vi.fn(async () => {});
    render(
      <SchedulerCatchUpBanner
        scheduler={stubState({
          pendingCatchUp: ["2026-04-19"],
          runCatchUp,
        })}
      />,
    );
    fireEvent.click(screen.getByTestId("scheduler-catch-up-run"));
    expect(runCatchUp).toHaveBeenCalledTimes(1);
  });

  it("wires the Skip button to skipCatchUp()", () => {
    const skipCatchUp = vi.fn(async () => {});
    render(
      <SchedulerCatchUpBanner
        scheduler={stubState({
          pendingCatchUp: ["2026-04-19"],
          skipCatchUp,
        })}
      />,
    );
    fireEvent.click(screen.getByTestId("scheduler-catch-up-skip"));
    expect(skipCatchUp).toHaveBeenCalledTimes(1);
  });
});
