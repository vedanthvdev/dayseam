// Full-screen first-run experience.
//
// Takes over the entire viewport while the setup checklist is
// incomplete — the plan (§Task 7, invariant #1) treats this as a hard
// gate so the user never sees a broken "no sources connected, no
// report to generate" main layout on a fresh install.
//
// The content is intentionally minimal: welcome copy on the left, the
// live checklist on the right. The checklist state is hoisted in by
// the caller so it can decide the gate and the empty state off the
// *same* hook instance and avoid a double-fetch.

import type { UseSetupChecklistState } from "./useSetupChecklist";
import { SetupSidebar } from "./SetupSidebar";

interface FirstRunEmptyStateProps {
  checklist: UseSetupChecklistState;
}

export function FirstRunEmptyState({ checklist }: FirstRunEmptyStateProps) {
  const remaining = checklist.items.filter((i) => !i.done).length;

  return (
    <main
      role="main"
      data-testid="first-run-empty-state"
      className="flex min-h-screen flex-col items-center justify-center bg-white px-8 py-16 text-neutral-900 dark:bg-neutral-950 dark:text-neutral-100"
    >
      <div className="grid w-full max-w-3xl grid-cols-1 gap-8 md:grid-cols-[minmax(0,1fr)_minmax(0,1.25fr)]">
        <section className="flex flex-col justify-center gap-3">
          <span className="text-xs font-medium uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
            Welcome to Dayseam
          </span>
          <h1 className="text-2xl font-semibold text-neutral-900 dark:text-neutral-50">
            Let&rsquo;s set up your workspace
          </h1>
          <p className="text-sm text-neutral-600 dark:text-neutral-400">
            Four quick steps and you&rsquo;re ready to generate your first
            end-of-day report, all locally. Nothing leaves this machine.
          </p>
          {checklist.loading ? (
            <span
              className="text-xs text-neutral-400 dark:text-neutral-500"
              aria-live="polite"
            >
              Loading setup status&hellip;
            </span>
          ) : (
            <span
              className="text-xs text-neutral-500 dark:text-neutral-400"
              aria-live="polite"
              data-testid="first-run-progress"
            >
              {remaining === 0
                ? "All set \u2014 one moment\u2026"
                : `${remaining} step${remaining === 1 ? "" : "s"} remaining`}
            </span>
          )}
        </section>

        <section>
          <SetupSidebar checklist={checklist} />
        </section>
      </div>
    </main>
  );
}
