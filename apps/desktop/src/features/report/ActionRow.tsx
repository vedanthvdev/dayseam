// The real "generate a report" action row — date picker, source
// multi-select, and the generate/cancel primary button. Replaces
// Phase-1's `ActionBar`, which rendered disabled-placeholder inputs
// so the wireframe was legible before any IPC existed.
//
// The row is a controlled component driven by the parent
// `useReport()` hook: this component owns the user-input state
// (`date`, `selectedSourceIds`) and fires the actions; it does not
// own the run state (`status`, `runId`, `progress`, …). That split
// keeps the hook the single source of truth for in-flight runs and
// lets the streaming preview and this row stay decoupled.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { Source } from "@dayseam/ipc-types";
import type { ReportStatus } from "../../ipc";
import { useSources } from "../../ipc";

export interface ActionRowProps {
  /** Current status from `useReport()`. Controls whether we show
   *  "Generate" or "Cancel" and whether the inputs are disabled. */
  status: ReportStatus;
  /** Called when the user confirms they want to start a new run.
   *  The parent is responsible for wiring this to `useReport.generate`. */
  onGenerate: (date: string, sourceIds: string[]) => void;
  /** Called when the user clicks "Cancel" on an in-flight run. */
  onCancel: () => void;
}

// DAY-119: ActionRow used to render the last `ProgressEvent.message`
// inline next to the date picker. `StreamingPreview` also renders that
// same message directly beneath its progress bar, so users saw the
// scanning folder twice in a row during a live sync (below the date
// field + below the loading bar). The preview is the canonical home
// — it is the surface the progress bar anchors on and it is already
// `aria-live="polite"`. The action row now focuses on input state
// (date picker, source chips, Generate/Cancel) and leaves live
// narration to the preview. The former `action-row-progress-message`
// testid is gone on purpose.

/** YYYY-MM-DD for the user's local today. The `<input type="date">`
 *  element formats in local tz; using the ISO UTC date would cause a
 *  near-midnight user in UTC-05:00 to see "yesterday" selected. */
function localTodayIso(): string {
  const now = new Date();
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function isRunning(status: ReportStatus): boolean {
  return status === "starting" || status === "running";
}

export function ActionRow({
  status,
  onGenerate,
  onCancel,
}: ActionRowProps) {
  const { sources, loading: sourcesLoading } = useSources();
  const [date, setDate] = useState<string>(() => localTodayIso());
  const [selected, setSelected] = useState<Set<string>>(() => new Set());

  // Auto-select every configured source the first time the list
  // arrives — matches the dominant user intent ("generate for
  // everything I've wired up"). Subsequent source additions are not
  // auto-selected so a power user who's curated their selection
  // doesn't have it silently widened.
  const [hasSeenInitialSources, setHasSeenInitialSources] = useState(false);
  useEffect(() => {
    if (hasSeenInitialSources) return;
    if (sourcesLoading) return;
    setSelected(new Set(sources.map((s) => s.id)));
    setHasSeenInitialSources(true);
  }, [sources, sourcesLoading, hasSeenInitialSources]);

  const running = isRunning(status);
  const disabled = running || sources.length === 0;

  const toggleSource = useCallback((id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const selectedIds = useMemo(() => Array.from(selected), [selected]);

  const canGenerate = !disabled && selectedIds.length > 0 && Boolean(date);

  const handleGenerate = useCallback(() => {
    if (!canGenerate) return;
    onGenerate(date, selectedIds);
  }, [canGenerate, onGenerate, date, selectedIds]);

  return (
    <section
      aria-label="Report actions"
      className="flex flex-wrap items-center gap-3 border-b border-neutral-200 bg-neutral-50/50 px-6 py-3 dark:border-neutral-800 dark:bg-neutral-900/40"
    >
      <label className="flex items-center gap-2 text-sm text-neutral-700 dark:text-neutral-200">
        <span>Date</span>
        <input
          type="date"
          value={date}
          onChange={(event) => setDate(event.target.value)}
          disabled={running}
          aria-disabled={running ? "true" : undefined}
          aria-label="Report date"
          data-testid="action-row-date"
          className="rounded border border-neutral-300 bg-white px-2 py-1 text-sm text-neutral-800 disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200"
        />
      </label>

      <fieldset
        className="flex flex-wrap items-center gap-1"
        aria-label="Sources included in the report"
      >
        {sources.length === 0 && !sourcesLoading ? (
          <span className="text-xs text-neutral-500 dark:text-neutral-400">
            Add a source above to enable Generate.
          </span>
        ) : null}
        {sources.map((source: Source) => {
          const isOn = selected.has(source.id);
          return (
            <label
              key={source.id}
              className={`inline-flex cursor-pointer items-center gap-1 rounded border px-2 py-0.5 text-xs ${
                isOn
                  ? "border-neutral-700 bg-neutral-900 text-white dark:border-neutral-300 dark:bg-neutral-100 dark:text-neutral-900"
                  : "border-neutral-300 text-neutral-600 hover:bg-neutral-100 dark:border-neutral-700 dark:text-neutral-300 dark:hover:bg-neutral-800"
              }`}
              title={isOn ? "Included" : "Excluded (click to include)"}
            >
              <input
                type="checkbox"
                checked={isOn}
                disabled={running}
                onChange={() => toggleSource(source.id)}
                className="sr-only"
                data-testid={`action-row-source-${source.id}`}
              />
              <span>{source.label}</span>
            </label>
          );
        })}
      </fieldset>

      {running ? (
        <button
          type="button"
          onClick={onCancel}
          data-testid="action-row-cancel"
          className="ml-auto rounded border border-red-300 bg-red-50 px-3 py-1.5 text-sm font-medium text-red-800 hover:bg-red-100 dark:border-red-800 dark:bg-red-950 dark:text-red-200 dark:hover:bg-red-900"
        >
          Cancel
        </button>
      ) : (
        <button
          type="button"
          onClick={handleGenerate}
          disabled={!canGenerate}
          data-testid="action-row-generate"
          className="ml-auto rounded bg-neutral-900 px-3 py-1.5 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
        >
          Generate report
        </button>
      )}
    </section>
  );
}
