// DAY-130 catch-up banner. Non-blocking strip that mounts above
// the merged `SourcesSidebar` report row (previously `ActionRow`;
// see DAY-170) when the Rust scheduler's cold-start scan (or an
// hourly tick) emits `scheduler:catch-up-suggested` with a list of
// missed dates. "Run" dispatches `scheduler_run_catch_up`; "Skip"
// dispatches `scheduler_skip_catch_up` (session-scoped — the banner
// re-surfaces on the next app open if the dates are still
// unsatisfied).
//
// The banner deliberately stays lightweight: it does not own the
// catch-up state machine (that's `useScheduler`) and it does not
// run the batch itself. All it contributes is the Run / Skip
// affordance and the headline copy — which keeps the UI easy to
// swap out for a richer surface later without re-plumbing the IPC.

import { useCallback, useState } from "react";
import type { UseSchedulerState } from "./useScheduler";

export interface SchedulerCatchUpBannerProps {
  scheduler: UseSchedulerState;
}

export function SchedulerCatchUpBanner({
  scheduler,
}: SchedulerCatchUpBannerProps) {
  const { pendingCatchUp, runCatchUp, skipCatchUp } = scheduler;
  const [busy, setBusy] = useState<"run" | "skip" | null>(null);

  const onRun = useCallback(async () => {
    setBusy("run");
    try {
      await runCatchUp();
    } catch {
      // Surfaced via `scheduler.error` already; reset the local
      // busy flag so the user can retry.
    } finally {
      setBusy(null);
    }
  }, [runCatchUp]);

  const onSkip = useCallback(async () => {
    setBusy("skip");
    try {
      await skipCatchUp();
    } catch {
      // As above — retain banner so user can retry.
    } finally {
      setBusy(null);
    }
  }, [skipCatchUp]);

  if (pendingCatchUp.length === 0) return null;

  const n = pendingCatchUp.length;
  const summary =
    n === 1
      ? `Catch up 1 missed report (${pendingCatchUp[0]})?`
      : `Catch up ${n} missed reports (${pendingCatchUp[0]} → ${pendingCatchUp[n - 1]})?`;

  return (
    <div
      role="region"
      aria-label="Scheduler catch-up"
      data-testid="scheduler-catch-up-banner"
      className="flex items-center justify-between gap-3 border-b border-amber-200 bg-amber-50 px-6 py-2 text-xs text-amber-900 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200"
    >
      <span>{summary}</span>
      <div className="flex items-center gap-2">
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void onSkip()}
          data-testid="scheduler-catch-up-skip"
          className="rounded border border-amber-400 bg-white px-2 py-0.5 text-xs font-medium text-amber-900 hover:bg-amber-100 disabled:opacity-50 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-100 dark:hover:bg-amber-900/40"
        >
          {busy === "skip" ? "Skipping…" : "Skip"}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void onRun()}
          data-testid="scheduler-catch-up-run"
          className="rounded bg-amber-600 px-2 py-0.5 text-xs font-medium text-white hover:bg-amber-700 disabled:opacity-60"
        >
          {busy === "run" ? "Running…" : "Run"}
        </button>
      </div>
    </div>
  );
}
