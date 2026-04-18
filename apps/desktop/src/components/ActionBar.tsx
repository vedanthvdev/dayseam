const DISABLED_HINT = "Available once sources are connected (Phase 2).";

/**
 * Action row — the date picker and the "Generate report" primary
 * action. Both are present but visibly disabled in Phase 1 so the
 * wireframe is legible without implying the app is broken.
 *
 * The inputs are real `<input>` / `<button>` elements (not fake divs)
 * so screen readers and keyboard users correctly observe them as
 * disabled. Phase 2 swaps in the real date-range picker and wires the
 * Generate button to the per-run IPC command.
 */
export function ActionBar() {
  return (
    <section
      aria-label="Report actions"
      className="flex flex-wrap items-center gap-3 border-b border-neutral-200 bg-neutral-50/50 px-6 py-3 dark:border-neutral-800 dark:bg-neutral-900/40"
    >
      <label className="flex items-center gap-2 text-sm text-neutral-700 dark:text-neutral-200">
        <span>Date</span>
        <input
          type="date"
          disabled
          aria-disabled="true"
          title={DISABLED_HINT}
          className="rounded border border-neutral-300 bg-white px-2 py-1 text-sm text-neutral-500 disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-400"
        />
      </label>

      <button
        type="button"
        disabled
        aria-disabled="true"
        title={DISABLED_HINT}
        className="ml-auto rounded bg-neutral-900 px-3 py-1.5 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
      >
        Generate report
      </button>
    </section>
  );
}
