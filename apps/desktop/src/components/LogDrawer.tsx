import { useEffect, useMemo, useState } from "react";
import type { LogEntry, LogEvent, LogLevel, RunId } from "@dayseam/ipc-types";
import { useLogsTail } from "../ipc";

const LEVELS: readonly LogLevel[] = ["Debug", "Info", "Warn", "Error"];

const LEVEL_CLASSES: Record<LogLevel, string> = {
  Debug: "bg-neutral-200 text-neutral-700 dark:bg-neutral-800 dark:text-neutral-300",
  Info: "bg-sky-200 text-sky-800 dark:bg-sky-900 dark:text-sky-100",
  Warn: "bg-amber-200 text-amber-800 dark:bg-amber-900 dark:text-amber-100",
  Error: "bg-red-200 text-red-800 dark:bg-red-900 dark:text-red-100",
};

function formatTimestamp(ts: string): string {
  try {
    const d = new Date(ts);
    if (Number.isNaN(d.getTime())) return ts;
    return d.toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return ts;
  }
}

export interface LogDrawerProps {
  open: boolean;
  onClose: () => void;
  /** The in-flight run whose logs can be isolated via the "This run
   *  only" toggle. `null` disables the toggle. The filter is
   *  client-side because the persisted `LogEntry` row doesn't carry a
   *  `run_id` — we cross-reference against the streamed `LogEvent`s
   *  by timestamp and message. */
  currentRunId?: RunId | null;
  /** Live-streamed log events for `currentRunId`. Passed in from
   *  `useReport()` so the drawer doesn't need its own per-run
   *  listener. An empty array is fine and means "no run active yet
   *  or no events yet." */
  liveLogs?: LogEvent[];
}

type RunFilter = "all" | "current";

/** Build the set of `(timestamp, message)` composite keys that belong
 *  to the current run. Timestamp matching is exact on the ISO-8601
 *  string because the Rust side emits the same `DateTime<Utc>` into
 *  both the live stream and the persisted row; ties on identical
 *  timestamp + message would already be indistinguishable in the
 *  drawer regardless. */
function currentRunKeys(liveLogs: LogEvent[]): Set<string> {
  const keys = new Set<string>();
  for (const ev of liveLogs) {
    keys.add(`${ev.emitted_at}|${ev.message}`);
  }
  return keys;
}

export function LogDrawer({
  open,
  onClose,
  currentRunId = null,
  liveLogs = [],
}: LogDrawerProps) {
  const { entries, loading, error, refresh } = useLogsTail({ autoLoad: open });
  const [activeLevels, setActiveLevels] = useState<Set<LogLevel>>(
    () => new Set(LEVELS),
  );
  const [runFilter, setRunFilter] = useState<RunFilter>("all");

  useEffect(() => {
    if (open) void refresh();
  }, [open, refresh]);

  // A new run resets the filter to "all" so the drawer doesn't
  // silently stay stuck on the previous run's keys.
  useEffect(() => {
    setRunFilter("all");
  }, [currentRunId]);

  const runKeys = useMemo(() => currentRunKeys(liveLogs), [liveLogs]);

  const visibleEntries = useMemo(() => {
    return entries.filter((e) => {
      if (!activeLevels.has(e.level)) return false;
      if (runFilter === "current") {
        if (!currentRunId) return false;
        return runKeys.has(`${e.timestamp}|${e.message}`);
      }
      return true;
    });
  }, [entries, activeLevels, runFilter, runKeys, currentRunId]);

  if (!open) return null;

  const toggleLevel = (level: LogLevel) => {
    setActiveLevels((prev) => {
      const next = new Set(prev);
      if (next.has(level)) next.delete(level);
      else next.add(level);
      return next;
    });
  };

  return (
    <aside
      role="dialog"
      aria-label="Log drawer"
      aria-modal="false"
      className="fixed inset-y-0 right-0 z-40 flex w-[420px] max-w-full flex-col border-l border-neutral-200 bg-white shadow-xl dark:border-neutral-800 dark:bg-neutral-950"
    >
      <header className="flex items-center justify-between border-b border-neutral-200 px-4 py-3 dark:border-neutral-800">
        <div className="flex flex-col gap-0.5">
          <h2 className="text-sm font-semibold text-neutral-900 dark:text-neutral-50">
            Activity log
          </h2>
          <p className="text-xs text-neutral-500 dark:text-neutral-400">
            Local-only; retained for troubleshooting.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => void refresh()}
            disabled={loading}
            className="rounded border border-neutral-300 px-2 py-1 text-xs text-neutral-700 hover:bg-neutral-50 disabled:opacity-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-900"
            title="Refresh"
          >
            {loading ? "Refreshing…" : "Refresh"}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close log drawer"
            title="Close (⌘L)"
            className="rounded border border-neutral-300 px-2 py-1 text-xs text-neutral-700 hover:bg-neutral-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-900"
          >
            Close
          </button>
        </div>
      </header>

      <div
        role="group"
        aria-label="Log level filters"
        className="flex flex-wrap items-center gap-1 border-b border-neutral-200 px-4 py-2 dark:border-neutral-800"
      >
        {LEVELS.map((level) => (
          <button
            key={level}
            type="button"
            role="checkbox"
            aria-checked={activeLevels.has(level)}
            onClick={() => toggleLevel(level)}
            className={`rounded px-2 py-0.5 text-[11px] uppercase tracking-wide transition ${
              activeLevels.has(level)
                ? LEVEL_CLASSES[level]
                : "bg-transparent text-neutral-400 line-through dark:text-neutral-600"
            }`}
          >
            {level}
          </button>
        ))}
        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            role="checkbox"
            aria-checked={runFilter === "current"}
            disabled={!currentRunId}
            onClick={() =>
              setRunFilter((prev) => (prev === "current" ? "all" : "current"))
            }
            data-testid="log-drawer-run-filter"
            className={`rounded px-2 py-0.5 text-[11px] uppercase tracking-wide transition disabled:opacity-40 ${
              runFilter === "current"
                ? "bg-sky-200 text-sky-800 dark:bg-sky-900 dark:text-sky-100"
                : "border border-neutral-300 text-neutral-500 dark:border-neutral-700 dark:text-neutral-400"
            }`}
            title={
              currentRunId
                ? "Show only entries tied to the current run"
                : "No active run"
            }
          >
            This run
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto px-4 py-3 font-mono text-xs">
        {error ? (
          <p className="text-red-600 dark:text-red-400">
            Failed to load logs: {error}
          </p>
        ) : visibleEntries.length === 0 ? (
          <p className="text-neutral-500 dark:text-neutral-400">
            {entries.length === 0
              ? "No entries yet."
              : "No entries match the current filters."}
          </p>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {visibleEntries.map((entry, idx) => (
              <LogRow key={`${entry.timestamp}-${idx}`} entry={entry} />
            ))}
          </ul>
        )}
      </div>
    </aside>
  );
}

function LogRow({ entry }: { entry: LogEntry }) {
  return (
    <li className="flex items-start gap-2">
      <span className="shrink-0 text-neutral-500 dark:text-neutral-400">
        {formatTimestamp(entry.timestamp)}
      </span>
      <span
        className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] uppercase tracking-wide ${LEVEL_CLASSES[entry.level]}`}
      >
        {entry.level}
      </span>
      <span className="break-words text-neutral-800 dark:text-neutral-200">
        {entry.message}
      </span>
    </li>
  );
}
